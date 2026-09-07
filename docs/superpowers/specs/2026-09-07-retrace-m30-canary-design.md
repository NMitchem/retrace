# M30-canary — give the guard band a signal it can lose

**Status:** design, 2026-09-07.
**Predecessors:** M27 (built the guard band), M28 (proved it can fire, and that it is attributable),
M29 (closed the `dest_buffer` table and leaned on the band throughout).

## Why this milestone

The M27 guard band is a one-directional detector. It samples `GUARD_BAND` bytes immediately past a
capped diff window before and after `host_svc`, and calls a difference proof of a kernel write:

```rust
pub fn overran_window(pre_guard: &[u8], post_guard: &[u8]) -> bool {
    !pre_guard.is_empty() && pre_guard != post_guard
}
```

A comparison can only report a change that happened. If the band held zeros and the kernel writes
zeros into it, `pre == post` and the overrun is invisible — not missed by accident, but undetectable
in principle, because no signal was ever placed there to lose. M27 measured exactly this on
`/bin/ps` and recorded it honestly. M28 hardened the tripwire without closing it; M29 did not touch
it; and it is the single reason the README must say the band is **"still not proof the class is
gone"** rather than the stronger claim three milestones of work would otherwise have earned.

This milestone puts a signal in the band so there is something to lose.

## Goals

1. Detect a kernel write past the diff window **regardless of the bytes written**, closing the
   zeros-over-zeros false negative.
2. Prove the closure with a **repo-owned positive control** that reproduces the `/bin/ps` false
   negative and cannot skip.
3. Measure the new detector's firing rate across the corpus **before** it is allowed to abort a
   recording.

## Non-goals

- **No `Event` or `TRACE_MAGIC` change.** Nothing here is recorded; no trace format moves.
- **No replay mirror.** `forward_and_diff` is record-side only, and an `assert!` mints no landmark.
- **Not the other owed items.** `proc_info`/`getattrlist`/`csops`'s flat window, `diff_memory`'s
  `.min(avail)`, the `readv` family's refusal, and `sysctlbyname`'s untested entry all stay owed.
- **Not a wider band.** `GUARD_BAND` stays 64. Widening trades one unmeasured constant for another;
  this milestone changes the *kind* of detector, not its size.

## Component 1 — the canary

### The problem, stated precisely

`overran_window` asks "did these bytes change?". That question has no answer when the kernel's write
is byte-identical to what was already there. The blind set is not only all-zeros: any band whose
post-image equals its pre-image is invisible, and zeros-over-zeros is simply its commonest instance,
because freshly-committed guest pages are zero and many kernel replies are zero-padded.

### The decision

Replace the question with "does the band still hold what I wrote?". Before the syscall, fill the
band with a known pattern; after it, compare against that pattern. A kernel write is then detectable
whatever it writes, unless it happens to reproduce the pattern exactly.

This writes into guest memory, which this project otherwise guards carefully. It is sound for the
reason `overran_window`'s own doc comment already gives: **the guest vCPU is halted across
`host_svc` and `clippy.toml` bans recorder threads**, so nothing but the kernel can observe or touch
those bytes in the interval. The band is restored before the vCPU resumes, so no guest can observe
it; nothing reaches the trace; and the whole mechanism is record-side, so no replay mirror is owed.

### Ordering, which is the load-bearing part

Five steps, in this order:

1. Capture every window's `pre` and `pre_band`, exactly as today.
2. **Compute each band's shrunk length, then fill it with the canary.**
3. `host_svc`.
4. Verify each canary; then restore every band from its `pre_band`.
5. Compare windows and push writes, exactly as today.

Step 2 moves `band_not_covered` from the post-pass to the pre-pass. It needs only the `(ipa, len)`
spans, all of which are known once step 1 finishes, so the move is mechanical — but it is
**required, not tidying**. Today's raw band is `GUARD_BAND.min(avail - win)` and may overlap another
argument's *window*. A canary written there would either contaminate that window's post-image, or —
if restored — **erase a genuine kernel write from the recording**. Shrinking first confines every
canary to bytes no window inspects, and both hazards disappear by construction.

Restoring in step 4 rather than step 5 matters for the same reason in reverse: windows must be
compared against true guest bytes, never against a canary.

### The pattern

Address-derived — byte at guest address `a` is a pure function of `a` — not a constant. Two reasons,
both load-bearing:

- **Anti-coincidence.** A kernel that memsets a constant cannot match a pattern that varies per
  byte. A single fixed fill would be defeated by the very case most likely to occur.
- **Overlap consistency.** Two *bands* may legitimately overlap each other (only band-vs-window
  overlap is excluded). Because the value depends only on the address, both writers agree on every
  shared byte, and verification is order-independent. A counter- or index-derived pattern would not
  have this property.

The function must be deterministic and cheap. Nothing nondeterministic may enter the recorder;
`clippy.toml`'s wall-clock ban is the standing expression of that rule.

**Pinned so the plan does not have to invent it:**

```rust
pub fn canary_byte(ipa: u64) -> u8 { (ipa as u8) ^ 0xA5 }
```

`0xA5` so that the two commonest accidental fills — all-zero and all-`0xFF` — are non-constant under
it and so `ipa = 0` does not produce a zero byte. Any function of `ipa` alone satisfies both reasons
above; this one is named here only to remove a choice the plan would otherwise make silently, and a
task that finds a concrete reason to change it may, provided it says why.

## Component 2 — the positive control

M27's band shipped with no positive control at all: the status log records that `let band = 0;`
passed the entire gate. M28 existed to fix that, and M29 twice found instruments that could not fire
through the channel reporting them. So the closure is not credible on reasoning alone.

A repo-owned guest reads from `/dev/zero` into a buffer with the diff window shrunk through the
existing `window_cap` seam — a field precisely so "a test can shrink it and hand the guard band a
REAL kernel overrun to detect". The kernel writes zeros past the window into a band that is already
zero. This reproduces `/bin/ps`'s false negative in a fixture that cannot skip, and the test must
demonstrate **both** halves:

- the **old** comparison is blind to it (`overran_window(pre, post)` is `false`), and
- the **canary** catches it.

A test that only shows the canary firing would not prove the false negative was ever closed, because
it would not establish that anything was blind in the first place.

`/dev/zero` is chosen over a synthetic write because the zeros come from the real host kernel through
the real forward path, which is what the detector guards.

## Component 3 — measure, then flip

Two phases, in this order, and the second is conditional on the first.

**Phase A** lands the canary as a gated diagnostic behind **`RETRACE_CANARY`** — its own variable,
not `RETRACE_TRACE`, which is also the per-trap firehose; M28's lesson is that a diagnostic must
reach a channel a reader can afford to open. Disturbances are tagged `[M30 CANARY]`. It is measured
across the 54-binary Apple sweep, `jq`, CPython, and the e2e guests.

Note what the gate does and does not cover. The canary is **filled and verified unconditionally** —
it has to be, or Phase B would flip on a measurement of a path production never takes. Only the
*reporting* is gated. So Phase A already changes what every recording does, and the sweep tally
being unmoved is the evidence that the change is inert.

Every measurement must be taken **through the channel that reports it**, with a positive control
proving that channel can carry a signal before any zero from it is believed. M29's Task 4 reported a
corpus-wide zero from a sweep whose stderr was discarded before the grep ran; the number was
structurally incapable of being anything else.

**Phase B** is decided by Phase A's number:

- **Zero disturbances** → flip the canary check to fail-loud, replacing `overran_window` at the call
  site, and the README's hedge comes off.
- **Any disturbance** → Phase B does **not** flip. Each occurrence is a kernel write past everything
  the diff inspected — a real finding, and running it down is the milestone. The hedge stays until
  it is understood.

A milestone that flips on an unmeasured assumption is the trap M27's own status log names: its band
landed as an `eprintln!` and became an `assert!` only after Task 2 measured zero firings.

## Testing

| What | Where | Proves |
|---|---|---|
| Canary pattern is address-derived and order-independent | `retrace-box` unit | Overlapping bands agree; a constant would not |
| Canary verification catches a single flipped byte | `retrace-box` unit | The predicate is a detector, tested apart from its plumbing |
| Old comparison is blind to zeros-over-zeros | `truncguard.rs` | The false negative is real, not assumed |
| Canary catches the same case | `truncguard.rs` | The closure works |
| `/dev/zero` guest, shrunk window, end to end | new e2e gate | The whole path, through the real kernel |
| Sweep tally unchanged by Phase A | measurement | A gated diagnostic perturbs nothing |

The predicate is extracted pure and tested directly, following `clamp_count`, `overran_window` and
`deref_len_fits`: the policy is reviewable, and testable, apart from the `unsafe` plumbing that feeds
it. M29's fast-follow is the cautionary case — a boundary inlined into a call site was exercised by
nothing, and the remedy its own status log prescribed could not have reached it.

## Risks

- **R1 — a canary lands inside a window.** Contaminates a post-image, or erases a kernel write on
  restore. *Mitigation:* shrinking before filling makes it impossible by construction, not by care.
  A test asserts no canary byte falls inside any window's span.
- **R2 — the kernel writes the pattern.** Astronomically unlikely for an address-derived pattern, but
  not impossible; the detector stays one-directional in principle. *Mitigation:* state it in the
  README rather than claiming proof. This narrows the hedge, it does not delete it.
- **R3 — the restore is skipped on the abort path.** If Phase B asserts before restoring, guest
  memory keeps the canary. Harmless, because the process is aborting, but it must be a deliberate
  documented choice rather than an oversight.
- **R4 — Phase A fires.** Not a risk to manage but a finding to chase; Component 3 already routes it.
- **R5 — cost on the record hot path.** Two extra 64-byte touches per mapped pointer argument per
  syscall, against window copies already up to 64 KiB. Expected to be noise; Phase A's sweep timing
  is the check, and M8's finding that per-syscall diff time is not free is the reason to look.

## Gate posture

No gate is parked and none is un-parked. The two `#[ignore]`d gates
(`stackoverflow_rust_e2e`, `cache_symbol_e2e`) are untouched. The gate figure moves by the tests
added here and is published only after the last commit that can change it — M28 published early and
left both documents stale by three.

## What stays owed after M30

`proc_info`(336), `getattrlist`/`fgetattrlist`(220/228) and `csops`(169/170) on a flat 64 KiB window;
`Box_::diff_memory`'s `.min(avail)` on the replay side, unpaid since M1's own branch review; the
`readv`/`recvmsg` family refused by value since M27; `sysctlbyname`(274) correct in the table but
exercised by no corpus. R2 above: even after this milestone, a band is not a proof of absence.
