# M29-clamptable — close the `dest_buffer` audit table

**Date:** 2026-09-06
**Status:** design, approved in chat
**Predecessor:** M28-bandproof (`docs/superpowers/specs/2026-09-06-retrace-m28-bandproof-design.md`)

## Why this milestone

M26, M27 and M28 each worked the same seam: the record-side memory-diff window, and the class of
bug where the kernel writes past it, the bytes land in guest memory on record, no `Event` records
them, and replay restores stale bytes. M26 found the class. M27 made it fail loud and freed
`/bin/ps`. M28 proved the tripwire can actually fire and made a firing attributable.

What all three left behind is not vague. The README names it in one paragraph
(`README.md:298-318`): a list of syscalls that still get a flat 64 KiB window, and one clamp that
"stays owed and unmeasured even for a covered syscall". M29 closes that paragraph.

This is a hardening milestone. It adds no capability and is expected to un-park nothing.

## Goals

1. Pay the owed `DerefU64` clamp — or, precisely, replace the gap with a **measured refusal**.
2. Add the four syscalls that have the shape `dest_buffer` already expresses.
3. Make M28's band-suppression count an observation the gate actually takes, rather than one a
   milestone claimed and could not have made.

## Non-goals

- **No multi-destination table.** Three of the four additions have a second destination; all three
  are self-bounding and already sit deep inside the flat window (see Component 2). Building a
  general multi-destination `dest_buffer` to model them would be YAGNI.
- **No change to M27's measured coverage false negative.** A 64-byte guard band of zeros still
  misses an overrun into zeros; M28 hardened the tripwire without closing that, and M29 does not
  touch it either. It stays in Known limits exactly as written.
- **No new guest capability**, no new rung, no `TRACE_MAGIC` bump. Nothing here changes `Event`'s
  shape or what a snapshot's bytes mean.

## Component 1 — the `DerefU64` gap: measure, then refuse

### The problem, stated precisely

`forward_and_diff` clamps the forwarded length for the `Reg` shape and deliberately does not for
`DerefU64` (`crates/retrace-box/src/lib.rs:3061-3076`). The existing comment calls this "owed and
unmeasured, a follow-up". It is not, however, the same operation as the clamp beside it, and the
difference is why it was deferred rather than forgotten:

| | `Reg` (read/pread) | `DerefU64` (sysctl's `oldlenp`) |
|---|---|---|
| Where the length lives | register `x{n}` | **guest memory** at `*(size_t*)x{n}` |
| Clamping writes to | `hargs[li]`, a forwarded register | **the guest's own memory** |
| Guest observes the clamp? | no — a short read is indistinguishable from a normal one | **yes** — it reads `*oldlenp` back |
| Effect on the call's outcome | fewer bytes read, correct return | a call that would have **succeeded natively returns `ENOMEM`** |

So clamping `DerefU64` is a fidelity change wearing a safety fix's clothes. Worse, the write would
land after the pre-image snapshot and before `host_svc`, so the diff would attribute retrace's own
write to the kernel and record it as a `Region`.

### The decision

**Refuse, do not clamp.** When `*oldlenp` exceeds the destination's backing, that is either a guest
bug or an unmodelled case; retrace stops loudly rather than letting the host kernel write past the
backing. Legitimate calls — every call where `*oldlenp <= avail` — forward completely untouched, so
fidelity is preserved exactly where it matters.

This is the discipline the repo already uses for the `readv`/`recvmsg` family ("refused by value,
fail-loud") and for `guest_workq_kernreturn`'s unenumerated opcodes.

### The two phases, in this order

**Phase A — measure.** Instrument the `DerefU64` arm to emit, for every occurrence, the syscall
number, `want` (`*oldlenp`), `avail` (`host_span`'s remaining bytes), and the destination backing's
`[ipa, ipa+len)` span. Count occurrences of `want > avail` over a corpus of **three** parts, named
so the number is reproducible: (a) the Apple sweep, all 54 binaries, via the script below; (b) the
repo's own heaviest `sysctl` users, `/bin/ps` and CPython, which is where M26/M27 found everything
they found; (c) `jq`, as a guest that reaches `main` through a different library path. Report the
count per part, not as one sum — a single number hides which part contributed it.

`read_u64(args[n])` on a NULL or unmapped `oldlenp` would be a guest bug, and it behaves here
exactly as it already does in `dest_len_bytes`, which reads the same pointer the same way. Phase A
changes nothing about that; it only adds the comparison.

The backing span is in the record on purpose: it is the discriminator for R1. If `want > avail`
because a legitimate buffer *spans two adjacent backings*, the neighbouring backing's start will
equal this one's end, and that is a different finding from a genuinely oversized request.

**M28's lesson binds here.** The count must come from a channel that can actually deliver it: a
human-run `RETRACE_BANDSHRINK`-style env-gated recorder run piped to `grep -c`, never a number
attributed to a test harness that pipes the recorder's stderr into a `String` a passing test never
prints. State how the number was taken, next to the number.

**Phase B — refuse, conditional on Phase A.** If Phase A counts **zero** `want > avail`
occurrences, land the assertion:

```rust
retrace_arch::DestLen::DerefU64(n) => {
    let want = self.read_u64(args[n]) as usize;
    // `oldp == NULL` is a legal "just tell me the size" sysctl — there is no destination to
    // bound, so there is nothing to check. `host_span` returning None is exactly that case.
    if let Some((_, avail)) = self.host_span(args[di]) {
        assert!(
            want <= avail,
            "unmodelled: syscall {} asked for {want} bytes at ipa {:#x} whose backing holds \
             only {avail} — forwarding it would let the host kernel write past the backing. \
             See the M29 spec, Component 1.",
            num as i64, args[di],
        );
    }
}
```

If Phase A counts **more than zero**, Phase B does *not* land the assert. The occurrences are the
finding; the milestone documents them and the clamp stays owed with a measurement attached instead
of none. That is a better outcome than M27's, which owed it with nothing.

`forward_and_diff` is record-side only and an `assert!` mints no landmark, so this owes **no replay
mirror** — the same reasoning that governs every other fail-loud assert in this function.

### The corpus, and why it gets scripted

The "47 of 54 Apple binaries" sweep has been ad-hoc since M22 — reconstructed by hand each time a
milestone needed it. M29 needs it twice (Phase A's measurement, and R3's check that the refusal
broke nothing that passed), so it gets a small `tools/apple-sweep.sh`: record-and-replay each
binary, print one line per binary, and a pass/fail tally. It takes the binary list as its own
committed content so the number is reproducible rather than remembered.

This is in scope because R3 cannot be checked reproducibly without it, and it pays off for every
later milestone that quotes the sweep number.

## Component 2 — four table additions

All four fit the existing `Option<(usize, DestLen)>` shape. Verified against the macOS 26.5 SDK
(`sys/syscall.h`, `sys/socket.h`, `sys/mount.h`):

| syscall | num | `dest_buffer` entry | in `fd_operands` today? |
|---|---|---|---|
| `getdirentries64` | 344 | `(1, Reg(2))` | **yes** (M25) — needs nothing |
| `getfsstat64` | 347 | `(0, Reg(1))` | n/a — takes no fd |
| `recvfrom` | 29 | `(1, Reg(2))` | **NO — must be added, `&[0]`** |
| `recvfrom_nocancel` | 403 | `(1, Reg(2))` | **NO — must be added, `&[0]`** |
| `sysctlbyname` | 274 | `(1, DerefU64(2))` | n/a — takes no fd |

Three of these need new `SYS_*` constants in `retrace-arch` (347, 29/403, 274); `SYS_GETDIRENTRIES64`
already exists.

**`sysctlbyname` (274) is a gap the README never named.** Its signature is
`sysctlbyname(const char *name, void *oldp, size_t *oldlenp, void *newp, size_t newlen)` — the
destination is `x1` and its length is `*(size_t*)x2`, byte-identical in shape to `sysctl`. It was
absent from the table *and* from the README's list of what is missing. One `DerefU64` arm now
covers both syscalls, so Component 1's refusal picks it up for free.

**`recvfrom` is missing from `fd_operands`, and `sendto` (133) is present.** A guest that receives
on a socket hands the host kernel an untranslated guest fd today. That is the M10 class — the same
asymmetry as M27's `pread_nocancel`, which was missing from `fd_operands`, the clamp and the window
at once. Adding it is cheap and can only correct behavior, but it is a behavior change and R2 names
it.

### Second destinations: unmodelled by decision, documented in the code

- `getdirentries64(fd, buf, bufsize, off_t *position)` also writes **8 bytes** at `*position`.
- `recvfrom(s, buf, len, flags, struct sockaddr *from, socklen_t *fromlen)` also writes `from` —
  but the kernel caps that write at the actual address size (`sockaddr_storage` is 128 bytes), not
  at `*fromlen`.

Both are far inside the flat 64 KiB window that every pointer argument already receives, and both
are self-bounding, so neither can produce the truncation class this table exists to prevent. The
table entries carry a comment saying so, naming the size — so a later reader can see the second
destination was considered and dismissed on a number, not overlooked.

## Component 3 — make the suppression count real

M28 gated `[M28 BANDSHRINK]` behind `RETRACE_TRACE`, which is also the full trap-trace firehose —
so turning the counter on to measure it means paying for tracing every dispatched trap.

Add a second, dedicated gate: the line prints when **either** `RETRACE_TRACE` or
`RETRACE_BANDSHRINK` is set. Then `crates/retrace/tests/sysbin_e2e.rs::ps_records_and_replays` sets
`RETRACE_BANDSHRINK=1` and asserts on the `rec.stderr` it already captures.

`crates/retrace/tests/util/mod.rs` has no env-passing helper today (`run` is
`Command::new(bin()).args(args).output()`), so this adds one — `record_dynamic_env(guest, env)` —
rather than mutating the test process's environment, which is process-global and `unsafe` under the
pinned 2024-edition toolchain.

The tag stays `[M28 BANDSHRINK]`, not renamed to M29: it names the milestone that created the
mechanism, and a rename would silently break any future grep written against M28's own status-log
reproduction command.

**The assertion is `> 0`, not `== 31`.** Pinning M28's exact measured number would make the gate
brittle against a different machine, mount count or OS point release, and a brittle failure there
teaches nothing. `> 0` proves the claim M28 actually needed and did not have: that the gate
exercises the suppression path and the count is observable from inside a test.

## Testing

| what | where | asserts |
|---|---|---|
| `dest_buffer` returns the four new entries with the right index and shape | `crates/retrace-arch` unit tests | exact `(usize, DestLen)` per syscall number, including both `recvfrom` spellings |
| `fd_operands` covers `recvfrom`/`recvfrom_nocancel` | `crates/retrace-arch` unit tests | `&[0]` for both |
| the window widens for a `Reg`-shape addition | `crates/retrace-box/tests/truncguard.rs` | `diff_window` for `getdirentries64` exceeds `window_cap` when the guest asks for more |
| the refusal fires when `*oldlenp` exceeds the backing (Phase B only) | `crates/retrace-box/tests/truncguard.rs` | `#[should_panic]` pinned to the syscall number, in the style M28 established for its positive control |
| the refusal does **not** fire on a legal NULL-`oldp` sysctl | `crates/retrace-box/tests/truncguard.rs` | completes normally |
| the suppression count is observable | `crates/retrace/tests/sysbin_e2e.rs` | `[M28 BANDSHRINK]` line count `> 0` in `rec.stderr` |
| nothing that passed stopped passing | `tools/apple-sweep.sh` | the *set* of passing binaries is unchanged, not merely the count — a binary dropping out while another joins would net to 47 and hide R4 |

The refusal test follows M28's positive-control discipline: pin the *firing site* in the
`should_panic` string, not just a fragment of the message, and choose a syscall the rest of the
machinery cannot widen out from under the test.

## Risks

**R1 — `avail` under-reports for a buffer spanning two backings.** `host_span` returns bytes
remaining in *one* backing. A legitimate guest buffer that spans two adjacent backings would look
oversized and trip the new refusal on a correct program. This is why Phase A logs the backing span
and why Phase B is conditional on Phase A finding zero. Note that the existing `Reg` clamp has the
identical blind spot but fails *quietly*, by truncating — the worse of the two failure modes, and
one this milestone does not fix.

**R2 — adding `recvfrom` to `fd_operands` changes behavior.** Its `x0` is translated where it was
not before. This should only correct things, but a guest that currently "works" through an
untranslated fd would change. The sweep is the check.

**R3 — four wider windows cost diff time.** M8 measured that window width costs per-syscall diff
time. `getfsstat64` on this machine is ~24 mounts × `sizeof(struct statfs64)`, and
`getdirentries64` buffers are typically 4-32 KiB, so the expected cost is small — but "expected" is
not "measured", and the sweep's wall-clock before and after is the cheap check.

**R4 — the refusal could abort an Apple binary that passes today.** If it does, that binary is
honestly parked with the reason on the test, which the honest-gate discipline counts as the
mechanism working rather than a regression.

## Gate posture

No new parked gate is expected. If R4 materialises, the park is honest and documented. At close,
both documents get touched as always: the README's "What works today" / "Known limits" edited in
place — specifically the `README.md:298-318` paragraph this milestone exists to shorten — and a new
append-only section added to `docs/status-log.md`.

## What stays owed after M29

Named rather than implied, so the next milestone inherits a list and not a feeling:

- M27's measured coverage false negative (a band of zeros misses an overrun into zeros).
- `diff_memory`'s `.min(avail)` on the replay side, unpaid since M1.
- The rest of the audit table: `proc_info` (336), `getattrlist`/`fgetattrlist` (220/228),
  `csops` (169/170) — each checked against the SDK and each still on the flat window.
- The `readv`/`recvmsg` family (120/27/540/411/401/480), still refused by value pending the
  `translate_mwl_regions` treatment.
- If Phase A finds occurrences, the `DerefU64` clamp itself — now owed *with* a measurement.
