# retrace — a record/replay reverse debugger for Apple Silicon

**Design spec — 2026-07-05**

## What this is

`retrace` records the execution of a native ARM64 command-line program on Apple
Silicon macOS and replays it **bit-for-bit deterministically** — same registers,
same memory, same syscall results, every replay, forever. On top of that recording
it exposes an LLDB-compatible session where `reverse-continue`, `reverse-step`, and
`reverse-next` work: you land on the exact instruction that produced a bad value and
walk *backward* to where it came from.

The target artifact is the first **working, open-source-licensed, debugger-integrated,
simulation-verified** record/replay reverse debugger for macOS. The headline demo is
reverse-stepping through **real CPython** (`python3` running a real script) after a
crash.

This is a scoped tool, not a general one. See [Scope](#scope-what-v1-does-and-does-not-record).

## Why it's hard (and why it doesn't exist yet)

To replay a program identically you must (1) capture every source of nondeterminism the
kernel or hardware injects, (2) make thread scheduling deterministic, and (3) be able to
name an exact execution position so you can stop "one instruction before the crash" and
step backward.

On Linux, `rr` gets (3) nearly free from a hardware retired-conditional-branch counter and
gets (1)/(2) from `ptrace` (with `sysemu`), `seccomp-bpf`, and `/proc/<pid>/mem`. **Apple
Silicon macOS provides none of these at usable fidelity:**

- No user-accessible PMU: there is no retired-instruction counter at any level — not via
  the PMU (reads crash; arming replay interrupts can kernel-panic XNU) and not via
  Hypervisor.framework (its `hv_sys_reg_t` enum contains zero PMU registers; its only
  execution meter is `hv_vcpu_get_exec_time`, which is *time*, not instructions).
- macOS `ptrace` is a stub of Linux's (no `sysemu`); there is no `seccomp`, no `procfs`.
- Mach exception ports + `task_for_pid` are entitlement/SIP-gated; hardened-runtime
  binaries resist `DYLD_INSERT_LIBRARIES`.

Both independent expert sources we consulted (the author of `rr.soft`; the authors of
Warpspeed) conclude that a *general*, `rr`-fidelity record/replay debugger for macOS is
"very difficult" and that the only viable path is a **scoped system that owns execution
end-to-end** — a userspace-controlled scheduler where "deliver event at instruction N"
needs no kernel interception. This design is that system.

## Verification status (2026-07-05)

The load-bearing claims below were verified on the target OS before committing to this
architecture (Apple Silicon, macOS 26.4.1 build 25E253, non-root, SIP enabled). Working
probes are in `spikes/`.

**Empirically confirmed on this machine:**

- **The entitlement is free.** Without it, `hv_vm_create` → `HV_DENIED`; ad-hoc signed with
  `com.apple.security.hypervisor` (no Apple account, no provisioning profile, non-root, SIP
  on) → `HV_SUCCESS`. The "a normal dev can ship this" claim holds.
- **No instruction counter exists** — `ID_AA64DFR0_EL1` reports `PMUVer=0x0`; the SDK
  headers contain zero PMU sysregs; only `hv_vcpu_get_exec_time` (time, not instructions).
  This confirms software single-step is *mandatory*, not a choice.
- **Stage-2 RWX** of a plain RW `MAP_ANON` buffer (no `MAP_JIT`) → `HV_SUCCESS`.
- **6 HW breakpoints / 4 watchpoints**; default IPA 36 bits (64 GiB).
- **The core loop works end-to-end:** a guest ran real instructions natively and its `HVC`
  trapped to the VMM with a decodable `ESR_EL2` (EC=0x16). This is the whole trap-and-forward
  architecture in miniature.

**Confirmed against the Apple SDK headers:** all debug/single-step/interrupt/vtimer APIs;
`MDSCR_EL1`/`DBGBVR0`/`HCR_EL2` sysregs; `hv_vm_config_set_ipa_granule` + `HV_IPA_GRANULE_4KB`
(the macOS 26 4 KiB-granule claim); exactly four exit reasons.

**Confirmed against primary sources:** `kallsyms/warpspeed` and `kallsyms/appbox` both carry
**no license** (`license: null`, no LICENSE file — all rights reserved; clean-room only);
`Impalabs/applevisor` is **Apache-2.0** (safe to depend on); the `rr.soft` magic counter
address `0x70001000` falls inside macOS arm64's 4 GiB `__PAGEZERO` (the deferred SoftPMU
needs address relocation).

**Not yet proven (engineering-effort risks, not platform walls) — deferred to M0/M1:**
EL0-`SVC`→`VBAR_EL1` trampoline path (spikes issue `HVC` from EL1 directly); dyld-shared-
cache loading on Tahoe (risk #1); memory-diff fidelity across the real syscall surface. None
of these is a capability gate — the platform allows everything the design needs; the open
questions are all "how much work," not "is it possible."

## Prior art (and our relationship to it)

- **Warpspeed** (Nick Gregory / Pete Markowsky, REcon 2023; repo `kallsyms/warpspeed`) —
  the only prior R/R debugger to target macOS. Uses the same substrate we chose (box a
  userspace Mach-O in a Hypervisor.framework VM, trap-and-forward syscalls, memory-diff
  recording). It is explicitly WIP and **never finished** thread-switch replay, external-
  signal replay, instruction-exact breakpoints, or any debugger UI, and its authors state
  it is "by no means production ready or even super useful." **The repo carries NO
  LICENSE** — it is "all rights reserved." We treat it as **read-only prior art for ideas
  and API sequences only. We never copy its code.** Our implementation is clean-room and
  licensed MIT OR Apache-2.0.
- **AppBox** (`kallsyms/appbox`) — the VM-management layer extracted from Warpspeed;
  implements "just enough to load an arm64 dyld shared cache as of macOS Tahoe (26.x)" with
  a 1:1 host↔guest mapping. Same license caveat. Study only.
- **applevisor** (Impalabs; `docs.rs/applevisor`, permissively licensed) — a complete Rust
  binding to Hypervisor.framework (VM/vCPU lifecycle, general/SIMD/system-register get/set,
  debug-trap enables, RWX memory map/protect, exit structs). Candidate dependency.
- **hyperpom** (Impalabs, GPL-3.0) — Apple-Silicon AArch64 userland fuzzer; its page-table
  setup is the reference for guest MMU construction. Study; do not vendor (GPL).
- **rr / rr.soft** — conceptual template (single-core scheduling, recorded async events).
  `rr.soft`'s software-counter technique (patch conditional branches to increment a counter
  and trap at a target) is our fallback for instruction counting in long syscall-free
  windows. Its MIT-licensed core and Apache-2.0 plugin are reusable references.

**Our novel contributions over all prior art:** (a) finishing the async-event replay and
instruction-exact positioning that Warpspeed left as TODO; (b) a first-class **determinism
oracle** — replay divergence as a seed-reproducible, named failure — and a **seeded fault
simulator** driving record→replay, none of which exists in any prior macOS R/R work;
(c) an LLDB-integrated reverse-debugging front end; (d) an open license.

## Architecture

The target runs at **EL0 inside a single-vCPU Hypervisor.framework VM with no guest OS**.
The VMM (our process) runs at EL2. Every syscall, mach-trap, and nondeterministic system-
register access traps out to the VMM, which records it. Five components, each with one job:

### 1. `retrace-box` — the VMM and loader

- Creates the VM (`hv_vm_create`) under the freely **ad-hoc-signable**
  `com.apple.security.hypervisor` entitlement — no Apple approval, root, SIP change, or
  kext. (`codesign -s - --entitlements`.) The shipped binary is Developer-ID-signed +
  notarized for Gatekeeper; the entitlement adds no extra distribution hurdle.
- Maps guest memory with `hv_vm_map` at **1:1 host↔guest virtual addresses** (AppBox's key
  simplification) so forwarded-syscall pointers cross the boundary unchanged. Uses macOS 26
  `hv_vm_config_set_ipa_granule(HV_IPA_GRANULE_4KB)` where 4 KiB granularity is needed;
  otherwise the default 16 KiB granule. Stage-2 RWX is permitted (no W^X on guest physical
  memory); the backing host buffer needs neither `PROT_EXEC` nor `MAP_JIT`.
- Loads the Mach-O, dyld, and the **dyld shared cache** (the single hardest part of loading
  a Mac binary — AppBox proves it is tractable on Tahoe), sets up stack, commpage, and TLS.
- Installs a **minimal guest EL1 vector table** (`VBAR_EL1`) whose only function is to
  convert a guest `SVC` into an `HVC` (which exits to the VMM — `EC_AA64_HVC` = 0x16). This
  is unavoidable: an EL0 `SVC` is architecturally taken to the guest's own EL1, never to
  EL2, and classic HVF exposes no knob to reroute it. The shim is ~a dozen instructions,
  not a kernel. Guest starts at EL1 (`CPSR = 0x3c5`) with a reset trampoline redirecting to
  EL0.
- Runs the vCPU loop: `hv_vcpu_run` → on `HV_EXIT_REASON_EXCEPTION` decode
  `ec = (ESR_EL2 >> 26) & 0x3f` → dispatch (HVC/syscall, `EC_SYSTEMREGISTERTRAP` 0x18 for
  trapped `MRS/MSR`, `EC_SOFTWARESTEP` 0x32, `EC_BREAKPOINT` 0x30, `EC_WATCHPOINT` 0x34,
  `EC_DATAABORT` 0x24/0x25). Handles `HV_EXIT_REASON_VTIMER_ACTIVATED` and spontaneous
  `HV_EXIT_REASON_CANCELED` exits as control-plane events, never as recordable inputs.

### 2. `retrace-record` — the trap handler / recorder

- On each syscall/mach-trap HVC exit: forward to the host kernel and capture the result via
  **memory-diff + pointer-chasing** (Warpspeed's technique — we do not model syscall
  semantics). Snapshot memory around pointer-valued arguments before the call; diff after;
  log the delta plus the return registers. Special-case only the handful that change the
  memory map or return fresh capabilities: `mmap`/`munmap`/`mprotect`, fd-returning, and
  mach-port-returning calls.
- Neutralizes the one untrappable input, `CNTVCT_EL0`: since we own guest memory (RWX), we
  **rewrite the guest's `mrs xN, cntvct_el0` read sites** to trap (replace with `HVC` or a
  branch to a recorder stub), record each observed value, and substitute the recorded value
  on replay. (`cntpct_el0` and the EL0 physical-timer regs already trap via
  `EC_SYSTEMREGISTERTRAP` and are recorded directly.) Any hardware RNG instruction
  (`rndr`/`rndrrs`) is handled the same way.
- Freezes the commpage to recorded contents during replay.
- Owns the **single-vCPU scheduler**: exactly one guest thread holds the run-token at a
  time; context switches happen only at recorded points (syscalls, blocking, and logged
  preemptions driven by our own instruction budget — never by host-scheduling-dependent
  vtimer/CANCELED exits). The schedule (which thread ran, for how many instructions) is
  written to the trace. Switching threads swaps vCPU register state.
- Streams events to `retrace-trace`.

### 3. `retrace-trace` — the on-disk format

- Append-only, checksummed, seekable event log (segmented; recovery-aware — the same WAL
  discipline as charpente-core::log). Records: syscall/mach results (as diffs), signals,
  the schedule, and `CNTVCT`/RNG observations.
- Periodic **full-VM snapshots** (memory + registers) so replay and reverse-execution do
  not restart from t=0. Memory snapshots exploit cheap host-page aliasing and
  `hv_vm_protect`-based dirty tracking (region protect ≈ hundreds of ns, near
  size-independent). Registers via `hv_vcpu_get_*`.
- Records the **host chip model and macOS build** in the trace header (cross-chip
  reproducibility is not guaranteed — see risks).

### 4. `retrace-replay` — the deterministic core

- Restarts the boxed program from a snapshot and re-runs it, but the trap handler now
  **feeds recorded results instead of executing syscalls** — it applies the recorded memory-
  diff and return registers *without re-entering the host kernel* (programs receive data
  only two ways: reading memory or a syscall result; both are reproduced from the log).
- Re-injects signals and preemptions at the **exact recorded instruction** (see
  [Positioning](#instruction-exact-positioning)).
- Enforces the recorded schedule via the run-token.
- Drives the **divergence checker**: at every snapshot boundary (and on demand) compares
  live replay state against the recorded snapshot and halts at the *first* diverging byte,
  printing the position, pc, diverging bytes, seed, and a one-command repro.

### 5. `retrace-debug` — the debugger seam

- A gdb/lldb-remote server so people use a debugger they already know. Forward operations
  are ordinary. Reverse operations are implemented as **"restore nearest earlier snapshot,
  replay forward to the target position,"** landing exactly via HW breakpoints (query
  `ID_AA64DFR0_EL1` for slot count — M1 has 6 breakpoints / 4 watchpoints) plus single-step
  for the residual.

## Instruction-exact positioning

There is no retired-instruction counter on Apple Silicon. An execution position is
therefore `(nearest landmark, instruction-offset-from-landmark, pc, registers)`, where a
landmark is a snapshot or a syscall boundary.

- **Coarse:** run at native speed to the nearest landmark (snapshots are taken at intervals;
  syscall boundaries are natural landmarks).
- **Fine (instruction-exact):** from the landmark, **single-step** via `MDSCR_EL1.SS` +
  `hv_vcpu_set_trap_debug_exceptions(true)` — each instruction raises `EC_SOFTWARESTEP` to
  the VMM — counting instructions until the target offset. This is reliable and hardware-
  assisted, and it is the piece Warpspeed specified but never built.
- **Optimization (deferred, not v1-critical):** for long syscall-free windows where per-
  instruction VMEXIT single-stepping is too slow, patch guest conditional branches to
  increment an in-guest counter and `brk` at a target (the `rr.soft` SoftPMU). Because we
  control guest RWX this is available, but it carries the `rr.soft` correctness landmine:
  **never patch a conditional branch between an exclusive load (`ldxr` family) and its
  `ret`** — it breaks LL/SC atomic sequences. Single-step is the baseline; the SoftPMU is a
  speed optimization added only if measurements demand it.

## Async signal / preemption replay (the crown jewel)

Recorded as `(landmark, instruction-offset, siginfo, register state)`. On replay: run to the
landmark at native speed, single-step the residual offset, then inject at exactly that
instruction — precise interrupt placement is constructible via single-step +
`hv_vcpu_set_pending_interrupt` before the next `hv_vcpu_run` (pending interrupts auto-clear
per run, so re-assert before each entry). **This milestone is what makes `retrace` a working
tool rather than a syscall recorder, and it is exactly what no prior macOS R/R project has
shipped.**

## The determinism oracle (the methodology that makes this verifiable)

Record/replay carries a free, absolute correctness oracle: **replay must reproduce the
recording bit-for-bit at every landmark.** The entire system is developed sim-first around
this, in the charpente tradition:

- **Divergence checker** (component 4): replay-vs-snapshot comparison that fails at the
  first diverging byte with position + pc + bytes + seed + one-command repro. A
  contract-violation-as-product-surface.
- **Seeded scenario simulator:** drives synthetic guest programs (threads racing on shared
  memory, signal storms, syscall-heavy loops, `mmap` churn) through record→replay under a
  seeded scheduler, asserting **zero divergence over N fresh seeds**. Every bug found pins
  its seed as a permanent regression. The run-token scheduler *is* the seeded deterministic
  scheduler, so the simulator and the production scheduler are the **same code** (the
  executor same-shape rule).
- **Negative-space assertions** (kept on in release): no syscall ever really executes during
  replay; the commpage and `CNTVCT` reads are frozen to recorded values during replay; no
  conditional branch between `ldxr` and `ret` is ever patched (if the SoftPMU is enabled);
  snapshot bytes match at every landmark; the instruction counter is monotonic.

## Nondeterminism surface (bounded, because everything traps to the box)

| Source | Handling |
|---|---|
| Syscall / mach-trap results | Memory-diff + pointer-chasing; special-case map-changing and capability-returning calls |
| `CNTVCT_EL0` (untrappable) | Rewrite read sites to record-and-substitute |
| `CNTPCT_EL0`, EL0 phys-timer regs | Already trap (`EC_SYSTEMREGISTERTRAP`); record directly |
| Hardware RNG (`rndr`/`rndrrs`) | Rewrite read sites to record-and-substitute |
| Commpage | Snapshot into trace; freeze on replay |
| Signals / async preemptions | `(landmark, offset, siginfo, regs)`; single-step-inject on replay |
| Thread scheduling | Run-token; schedule recorded |
| `HV_EXIT_REASON_CANCELED` / vtimer exits | Control-plane only; never a recordable input |

## Scope (what v1 does and does not record)

**In scope:** native ARM64 command-line programs, servers, test runners, and language
runtimes that reach the kernel via `svc` — including hardened-runtime binaries (the box
loads the Mach-O itself, so `DYLD_INSERT_LIBRARIES` restrictions are irrelevant). The
success bar is recording and reverse-debugging **real interpreters** (`python3`, `node`,
`git`) on real workloads.

**Out of scope for v1:** GUI applications (WindowServer / graphical Mach-IPC recording);
AArch32 (absent on Apple Silicon); SVE/SME-dependent code (largely absent and version-
dependent across chips); nested virtualization. MMIO with ISV=0 syndromes would need a
custom instruction decoder but a boxed userspace process does little to no MMIO, so this is
low-risk.

## Milestones (dependency-ordered)

> **2026-07-05 update:** the original M1 (below) is split — the memory-diff recording engine
> is now **M1** (see `2026-07-05-retrace-m1-memory-diff-recorder-design.md`), and the dyld/
> DSC/MMU/PAC loader is **M2**. Everything after shifts by one (positioning → M3, signals →
> M4, debugger → M5, interpreter → M6). The two are orthogonal risks and were bundled here
> only for brevity; the dependency ordering is otherwise unchanged. M2 itself later grew two
> loader-revealed sub-milestones — **M2-cache** (shared-cache re-signing, landed 2026-07-07,
> `2026-07-07-retrace-m2-cache-resign-design.md`) and **M2-mach** (mach-IPC kernel-RPC
> servicing, `2026-07-07-retrace-m2-mach-design.md`) — without renumbering M3–M6.

- **M0 — Box & trace spine.** VMM loop; load a trivial static Mach-O; EL1 `SVC→HVC`
  trampoline; 1:1 mapping; first snapshot; checksummed trace format. *Exit:* record + replay
  an `/bin/echo`-class program with **zero divergence over N fresh seeds.**
- **M1 — Syscall record/replay + dyld shared cache.** Memory-diff engine; special-cased
  map/fd/port syscalls; load a dynamically-linked binary. *Exit:* record + replay a
  file-I/O program that replays identically after its input files are deleted.
- **M2 — Instruction-exact positioning.** Single-step counting from landmarks; interval
  snapshots; HW breakpoints. *Exit:* stop at an exact instruction position deterministically
  across record and replay.
- **M3 — Async signal + thread-switch replay.** Deliver recorded signals/preemptions at the
  exact instruction; deterministic run-token scheduling of multithreaded targets. *Exit:* a
  SIGSEGV/timer-callback program **and** a threaded data race each replay identically to the
  instruction. **This milestone surpasses Warpspeed.**
- **M4 — Debugger seam.** gdb/lldb-remote server; `reverse-continue`/`reverse-step`/
  `reverse-next` via nearest-snapshot-then-replay-forward. *Exit:* reverse-step through a
  real crash in LLDB.
- **M5 — Real interpreter.** Harden syscall/mach coverage, `CNTVCT` instrumentation, and
  commpage handling until Homebrew `python3` records and reverse-debugs a real script.
  *Exit:* the viral demo.

## Risk register

1. **dyld-shared-cache loading fidelity** across macOS point releases (AppBox proves Tahoe;
   the loader is brittle to OS updates). *Mitigation:* pin the macOS build in the trace;
   treat the DSC loader as a first-class maintained component; assert-and-fail loudly on an
   unrecognized cache layout rather than silently mis-loading.
2. **A syscall/mach result the diff engine cannot capture** (opaque kernel-side state, e.g.
   some mach-port right semantics). *Mitigation:* the divergence checker catches it at a
   named landmark immediately; special-case as found. This is the main schedule risk and the
   long tail of the project.
3. **`CNTVCT_EL0` reads via an unexpected path** that read-site rewriting misses.
   *Mitigation:* the divergence checker flags the resulting divergence; widen the rewrite
   pass. It fails loudly, never silently.
4. **Cross-chip / cross-OS trace portability** (feature registers, TSO differences). A trace
   recorded on an M4 may not replay on an M1. *Mitigation:* record the host chip + macOS
   build; v1 supports replay on the same chip class; pin a feature-register profile via
   `hv_vcpu_set_sys_reg`; document the limitation.
5. **Single-step positioning overhead** on long syscall-free windows. *Mitigation:* interval
   snapshots bound window length; the SoftPMU optimization is available if measurements
   demand it.
6. **Warpspeed/AppBox are unlicensed.** *Mitigation:* clean-room implementation only; never
   copy their code; our own MIT OR Apache-2.0.

## Non-goals / explicitly deferred

- GUI-app recording; multi-process / follow-fork recording; network-partition realism.
- Shareable traces across machines (v1 is same-chip-class).
- The SoftPMU is deferred behind measurement; single-step is the v1 positioning baseline.

## Dependencies (justified)

- **Hypervisor.framework** (system) — the substrate.
- **`applevisor`** (permissive) — Rust HVF binding; candidate, pending a license and API
  audit. We may instead maintain a thin in-tree binding to keep the dependency surface
  minimal and under the determinism discipline.
- Rust, proc-macro crates for the derive surface (matching charpente conventions).
- Nothing inside the deterministic boundary that does its own IO, threading, or time.

## Open questions for implementation planning

- Depend on `applevisor` vs. maintain a thin in-tree HVF binding? (Leaning in-tree for
  dependency minimalism and to keep the whole VMM under our assertion discipline.)
- Trace format details: segment size, snapshot interval policy, compression (defer
  compression to post-v1, per rr's lesson that an unoptimized log is fine to start).
- How much of the dyld-shared-cache loader must be written vs. can be driven through
  supported APIs on Tahoe.
