# retrace M8-stack — guest stack identity (`kern.usrstack64` / `RLIMIT_STACK`) + anon `MAP_FIXED`

**Design spec — 2026-07-31.** Clears the wall M7 re-parked at (rung 1, `hello_rust`, libstd's
stack-overflow guard-page install). The wall is **two independent defects on one code path**: retrace
tells the guest its stack lives at a *host* address (`sysctl(KERN_USRSTACK64)` is forwarded), and
retrace's anonymous `mmap` path silently ignores `addr`/`MAP_FIXED`. Neither is a pointer-signing or
PAC defect; both are one-value/one-flag bugs of the M2-cpuid and M2-tbi shape.

## The problem, precisely

`hello_rust` dies before `main` inside libstd's pre-`main` init:

```
thread 'main' panicked at library/std/src/sys/pal/unix/stack_overflow.rs:526:13:
failed to allocate a guard page: Undefined error: 0 (os error 0)
```

The panic drives Rust's abort path, raising a real `SIGABRT` that reaches the host `record-dyn`
process (exit 134).

## Verified facts (measured on this host, HEAD `48fe554`, 2026-07-31)

**The guest code is `install_main_guard_default`** (Rust 1.95.0 std, the pinned toolchain that built
the guest; `rustup component add rust-src` to read it). macOS reaches it through the `else` arm of the
`cfg!` ladder at `stack_overflow.rs:412-425` — this is the correct, expected path, not a `cfg` anomaly.
The failing check is `stack_overflow.rs:515-527`:

```rust
let result = mmap64(stackptr, page_size, PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANON | MAP_FIXED, -1, 0);
if result != stackptr || result == MAP_FAILED {
    panic!("failed to allocate a guard page: {}", io::Error::last_os_error());
}
```

`prot = RW` (not `PROT_NONE`) is deliberate and explained at `:509-513` — libstd maps RW, then
`mprotect`s to `PROT_NONE` at `:529`. The `result != stackptr` branch **does not set errno**, which is
the only explanation for `Undefined error: 0 (os error 0)`: the syscall *succeeded* at the wrong
address.

**The observed trap** (`RETRACE_TRACE=1 record-dyn hello_rust`):

```
[trap] num=197 (0xc5) pc=0x1804aea18 args=[0x16eca0000,0x4000,0x3,0x41012,0xffffffff,0x0]
[trap] num=4  (0x4)  → write(fd 2): "failed to allocate a guard page: Undefined error: 0 (os error 0)"
```

`flags = 0x41012` = `MAP_UNIX03|MAP_ANON|MAP_FIXED|MAP_PRIVATE`.

**The requested address is host-derived and differs every run** — measured across five recordings:
`0x16f4ec000`, `0x16ec7c000`, `0x16eb90000`, `0x16f3c0000`, `0x16c6f0000`. Host `kern.usrstack64`
sampled three times on the same machine: `0x16f8a8000`, `0x16f120000`, `0x16cfe0000` — same range,
same 16 KiB alignment, same variance.

**The carrier, isolated by instrumenting `forward_and_diff`'s writes** (temporary diagnostic, reverted):

```
[leak] num=202 (0xca) wrote 0x16f49c000 at ipa 0x1fc178
[sysctl] mib=[1, 59] oldp=0x1fc178          ← the exact oldp of that sysctl
```

`mib = [1, 59]` = `{CTL_KERN, KERN_USRSTACK64}`, confirmed against the SDK
(`sys/sysctl.h:276`, "LP64 user stack query"; `KERN_OSVERSION = 65` at `:282` corroborates the
numbering, and mib `[1,65]` appears in the same run). Note the *last* sysctl before the mmap is
`[6,7]` = `hw.pagesize` — a red herring; `KERN_USRSTACK64` is called earlier.

**The arithmetic closes exactly:**

```
kern.usrstack64 (host ASLR, forwarded)  = 0x16f49c000
RLIMIT_STACK    (host, forwarded)       = 0x007fc000   (8176 KiB; matches `ulimit -s`)
                                          ----------
libstd stackptr = usrstack − stacksize  = 0x16eca0000  ← precisely the requested mmap address
```

`getrlimit` is invoked as `0x1003` = `RLIMIT_STACK | _RLIMIT_POSIX_FLAG` (`sys/resource.h:446,458`).

**Defect 2 — anonymous `MAP_FIXED` is ignored.** `retrace-core/src/lib.rs:133` routes anonymous mmap
to `b.guest_mmap(args[1])`, passing **only the length**. `Box_::guest_mmap`
(`retrace-box/src/lib.rs:1291`) is a pure bump allocator returning `self.mmap_next`, which starts at
`MMAP_BASE = NANO_BAND_END = 0xA_0000_0000` and only increases. Since `0x16eca0000 < 0xA_0000_0000`,
it is **arithmetically impossible** for that call to return the requested address. The file-backed
path is correct — `map_mmap_region` (`:1312-1321`) honors `MAP_FIXED` at `:1316`. The anon path never
received the same treatment.

**`mprotect` is already serviced in-box** (`retrace-core/src/lib.rs:162`, `guest_mprotect`, returns 0,
not forwarded), so libstd's follow-up `mprotect(PROT_NONE)` at `stack_overflow.rs:529` needs no work.

## Why this is a correctness defect, not merely nondeterminism

M2-cpuid's spec (`2026-07-14-retrace-m2-cpuid-design.md:40-42`) already settled the general question:
forwarded syscalls whose outputs are recorded per-trace (`getentropy`, `proc_info`) **legitimately**
differ run-to-run, and that does not threaten replay determinism, which the divergence oracle enforces
per trace. That position stands and this spec does not disturb it.

The defect here is of a different kind: `KERN_USRSTACK64` is **semantically wrong**, not merely
nondeterministic. It hands the guest a *host* address to use as a *guest* address. It would be a bug
if it were perfectly deterministic — which is exactly M2-cpuid's own shape, where `TPIDR_EL0 = 0x30000`
was deterministic-but-wrong. This one is nondeterministic-and-wrong; the nondeterminism is the symptom
that made it visible, not the offense.

The invariant being restored: **the guest must be told the truth about its own address space.** A
value that names a location in retrace's host address space must never be handed to the guest as a
guest address.

## The mechanism

Three changes, all below or at the record/replay dispatch.

**A. Synthesize `sysctl({CTL_KERN, KERN_USRSTACK64})`.** Service mib `[1,59]` in the record dispatch:
write `DYN_STACK_TOP` (`0x0020_0000`) as a `u64` into the guest's `oldp`, update `*oldlenp`, return 0.
Never forwarded. Non-matching mibs keep forwarding unchanged.

**B. Synthesize `getrlimit(RLIMIT_STACK)`.** Return `rlim_cur = rlim_max = DYN_STACK_SIZE`
(`0x0004_0000`). The resource argument must be masked with `!_RLIMIT_POSIX_FLAG` — the guest passes
`0x1003`, not `3`. Other resources keep forwarding.

**C. Honor `addr`/`MAP_FIXED` in the anonymous mmap path.** Widen `guest_mmap(len)` to
`guest_mmap(addr, len, prot, flags)` and delegate placement to the existing `map_mmap_region`, which
already implements FIXED correctly. Because the guard page lands *inside* the live stack backing, the
FIXED case must first `unmap_overlapping` (the same helper `guest_vm_map`'s FIXED path already uses at
`retrace-box/src/lib.rs:1157`).

Together: `stackptr = 0x200000 − 0x40000 = 0x1C0000`, the true bottom of the guest stack, identical on
every run and on replay.

**Stack geometry is unchanged.** `DYN_STACK_TOP` / `DYN_STACK_SIZE` keep their current values; there
is no IPA-layout change. 256 KiB is already proven to carry dyld plus full libSystem init plus `main`
(that is the `hello_dyn` path). Enlarging the stack was considered and rejected: `BoxState.mem`
(`retrace-box/src/lib.rs:334`) is a full memory capture per checkpoint under a 256 MiB LRU budget, so
an 8 MiB stack would cost 8 MiB on *every* checkpoint and measurably erode M4's seek speedup for no
present benefit.

## Determinism posture

All three synthesized values derive from fixed box constants, so this takes the **standard symmetric
posture**: replay recomputes the identical bytes and byte-compares against the recording, and that
comparison *is* the divergence check (symmetry rule 1). This is M2-setport's posture, explicitly
**not** M2-xpcport's record-verbatim asymmetry — nothing here is host-derived once fixed.

Each record-side special case therefore requires a mirrored replay arm. An asymmetry surfaces as a
divergence, not as silent corruption.

## The address-space determinism oracle

The existing divergence oracle compares replay against a recording; it never compares one recording
against another, so it is structurally blind to a host address entering the trace. That blind spot is
why this defect survived seven milestones and 146 passing tests.

**The oracle:** record the same guest twice, then assert that every *address-shaped* field matches —
`mmap`/`mach_vm` return addresses, address-carrying syscall arguments, and every `Region.ipa`. Opaque
payload bytes are **not** compared.

The address-shaped restriction is load-bearing, not a convenience. A byte-identical oracle would fire
on values the project has deliberately and correctly accepted as per-trace nondeterministic:
M2-xpcport's minted bootstrap port name, M2-taskinfo's audit token, and M2-cpuid's `getentropy` /
`proc_info`. Comparing only addresses detects host-addresses-masquerading-as-guest-addresses — the
actual invariant — while leaving that settled ground untouched, and needs no allowlist to maintain.

## Correctness invariant

No value that names a location in retrace's host address space may be delivered to the guest as a
guest address. Guest-observable address-space facts (`kern.usrstack64`, `RLIMIT_STACK`) are derived
from the box's own layout constants, identically on record and replay.

## Scope

**In scope:** the three changes above; the address-space oracle and its test harness; a freestanding
asm guest fixture exercising all three; re-evaluating `hello_rust_e2e`.

**Out of scope:** signal delivery (`panic!`/`abort()` → `SIGABRT`), deferred since M6 and unchanged
here; threads (`Sched` stays unused); stack *size* (a deep-recursion guest hitting 240 KiB is a new
and honest wall); a general guest-truthful sysctl layer (YAGNI — one leaking oid is known, and the
oracle is the general guard); an arm64e dynamic guest; rung 2 (`brew jq`).

## Exit criterion

`just gate` green, clippy clean, and:

1. The record-twice oracle passes on the new fixture and on `hello_dyn`.
2. Anonymous `MAP_FIXED` returns the requested address.
3. The synthesized values equal `DYN_STACK_TOP` / `DYN_STACK_SIZE`, and recordings replay bit-for-bit.
4. `hello_rust_e2e` un-ignored **only on a genuine double pass**. If clearing the guard page exposes
   the next libstd init wall, the gate is re-parked with its `#[ignore]` reason rewritten to the new
   signature, per honest-gate discipline and M7's precedent. This milestone does **not** promise a
   green headline gate.

## Testing

The failing test comes first and deliberately does **not** depend on `hello_rust`: that run aborts via
`SIGABRT`, so its trace tail may be torn (`open_checked` drops it) and it is unfit as a fixture.

A new freestanding asm guest — `crates/retrace-guest/asm/usrstack.s`, following the existing
`machmsg.s` / `fileio.s` pattern — issues `sysctl(KERN_USRSTACK64)`, `getrlimit(RLIMIT_STACK)`, and an
anonymous `MAP_FIXED` mmap at a computed address, then stores the results where the test can read them.

- **Record-twice on `usrstack`: address-shaped fields identical.** Fails on HEAD (differs by host
  ASLR); passes after the fix.
- **Anonymous `MAP_FIXED` returns exactly the requested address.**
- **Synthesized values equal `DYN_STACK_TOP` / `DYN_STACK_SIZE`.**
- **The recording replays bit-for-bit** (symmetry rule 1 holds for all three new arms).
- **The oracle passes on `hello_dyn`.** This must be *verified, not assumed*. If it fails, that is a
  second real host-address leak and gets its own finding rather than an oracle weakened to accommodate
  it.

## Risk register

- **R1 — walls come in chains.** The guard page may not be the last pre-`main` wall; `sigaltstack` and
  signal delivery sit immediately downstream in libstd's init. M8 does one rung and re-parks honestly.
- **R2 — the FIXED guard page unmaps into the live stack backing.** `unmap_overlapping` must not
  disturb the `KernelArgs`/start-stack, which `build_start_stack` lays down at the stack *top*
  (`retrace-box/src/lib.rs:1091`). The guard page is at the bottom; the gap from SP must be verified,
  not assumed.
- **R3 — 240 KiB usable stack** after a 16 KiB guard. A future deep-recursion guest hits a size wall.
  That is a new, honest wall, and R3 is the note that it was chosen knowingly.
- **R4 — `mprotect(PROT_NONE)` is best-effort.** A guest that touches the guard page expecting SIGSEGV
  lands in M6 crash-recording territory; out of scope here.
- **R5 — the oracle may be too strict or too loose.** Too strict fails on legitimate per-trace
  nondeterminism (mitigated by comparing addresses only); too loose misses payload-carried host
  addresses. The `hello_dyn` check calibrates it against a known-good run.

## Components

- `crates/retrace-core/src/lib.rs` — record dispatch arms for `sysctl`/`getrlimit` (near the existing
  mmap arm at `:133` and the `mprotect` arm at `:162`); their mirrored replay arms (the existing
  mirrors live at `:589` for anon mmap and `:672` for `mprotect` — new arms go alongside); the widened
  anon-mmap call.
- `crates/retrace-box/src/lib.rs` — `guest_mmap` signature widened; FIXED placement via
  `map_mmap_region` + `unmap_overlapping`.
- `crates/retrace-arch/src/lib.rs` — `SYS_SYSCTL = 202`, `SYS_GETRLIMIT = 194`, `CTL_KERN`,
  `KERN_USRSTACK64`, `RLIMIT_STACK`, `_RLIMIT_POSIX_FLAG`.
- `crates/retrace-guest/asm/usrstack.s` + `build.rs` + the `USRSTACK` path constant.
- `crates/retrace/tests/` — the oracle harness and its tests.

## Open questions for implementation planning

1. Does `hello_dyn` itself call `KERN_USRSTACK64`? If so its recorded trace changes, and any golden
   fixture keyed to the old value needs updating. Determine empirically in task 1.
2. Where does the oracle's address-shaped projection live — a helper in `retrace-trace` (reusable, but
   widens that crate's surface) or in the test harness (contained)? Prefer the test harness unless a
   second consumer appears.
3. Does `guest_mmap`'s widened signature have non-test callers beyond the two dispatch sites? Confirm
   before changing it.
