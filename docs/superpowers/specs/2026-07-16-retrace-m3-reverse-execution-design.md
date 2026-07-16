# retrace M3 — reverse execution: position, single-step, scripted debug

**Design spec — 2026-07-16.** The first post-M2 milestone. M2 closed with the headline gate green
([M2-taskinfo](2026-07-15-retrace-m2-taskinfo-design.md)): `hello_dyn` records and replays
bit-for-bit through real `/usr/lib/dyld`, gate 78/0/**0** ignored. There is no known wall left on
that path, so M3 builds the capability retrace exists for: **positioning replay at an arbitrary
instant in a recording and stepping it backward** — the reverse-debugger core, shipped as a scripted
`retrace debug` subcommand. Original vision:
[the M0 design](2026-07-05-retrace-macos-record-replay-design.md).

## The idea: time is a coordinate; backward is forward

A moment in a recorded run is named by **P = (landmark N, step K)** — the state after the first `N`
trace events have been consumed and `K` further instructions have retired. `landmark` is exactly the
event index replay already tracks (`idx` in `replay`, the same number `Divergence.landmark` reports).
Because replay of a given trace is bit-exact, P is total and deterministic: **seeking the same P
twice yields byte-identical machine state.** That property is M3's oracle, the direct extension of
the divergence oracle.

Nothing ever executes backward. Every reverse operation computes an *earlier* coordinate and
re-seeks forward to it from the snapshot:

- `seek(N, K)` = restore snapshot → replay N events at native speed → single-step K instructions.
- `reverse-stepi` from (N, K) = seek (N, K−1); at K = 0, seek (N−1, len(window N−1)), the window
  length found by one forward counting pass.
- `reverse-continue` = one forward scan from the snapshot to the current P recording every
  breakpoint hit, then seek the last hit before P (clean "no earlier hit" if none).

**Engine choice (approved): re-replay + single-step, no checkpoints.** Each seek is O(run length) —
a full `hello_dyn` replay is ~242 events and sub-second, so this is fine at current guest scale.
Checkpoints are a pure acceleration with meaningful machinery (committed-page copies, invalidation)
and are deferred until a guest's replay time actually hurts. A host-side AArch64 interpreter for
stepping was rejected outright: it reimplements the ISA including PAC, violating the "never
reimplement Apple's PAC" rule. Hardware makes the choice unambiguous anyway — see the spike findings
below: **there is no PMU instruction counter in the HVF guest (PMUVer=0)**, so architectural
single-step is the only exact tick source on this platform.

M3 makes **zero trace-format changes**. Stepping lives below the trace (symmetry rule 2); nothing
about debugging enters a recording, no `TRACE_MAGIC` bump, and parallel work streams (hygiene batch,
guest breadth) have no format-coordination hazard with M3.

## Verified facts (this repo — read directly, HEAD `ca9c06d`)

- **Replay's resumable state is five locals.** `replay(trace_path) -> Result<ReplayReport, Divergence>`
  (`crates/retrace-core/src/lib.rs:371`) materializes all events up front via
  `Reader::open_checked`, rebuilds the guest with `Box_::restore(&mem, &regs)` (:386), and loops
  over `b.run()` with locals `b`, `events`, `idx` (event cursor, :389), `stdout` (:388),
  `guest_task_port` (:392). Only trace-bearing stops advance `idx`; `Stop::Other` cache page-ins /
  reservation commits `continue` without advancing (:608–616). `SYS_EXIT` verifies the `Exit` event
  + final `Snapshot` via `b.diff_memory` (:397–418).
- **`Stop` has two variants** — `Syscall { num, args: [u64;8] }` / `Other { esr }`
  (`crates/retrace-box/src/lib.rs:233`). `Box_::run()` (:1277) loops on `hv_vcpu_run`; the guest's
  EL1 vector table is a trampoline of `hvc #0` slots (`VBAR_EL1 = TRAMPOLINE_IPA`), so **every**
  EL0 exception surfaces at EL2 as `Ec::Hvc` and `run()` reads `ESR_EL1` for the true cause. The
  below-the-trace emulation arms — `try_emulate_timebase` (:407), `try_emulate_undef_mrs` (:439),
  `try_emulate_fpac_auth` (:471) — each resume at `ELR_EL1 + 4` with `CPSR = SPSR_EL1` and
  `continue` inside `run()`, invisible to callers.
- **The guest runs at EL0** (`CPSR = 0`, EL0t, at `load`:524 / `load_dynamic`:935); syscall resume
  goes through `set_x0_and_return` / `set_x0_err_and_return` (:1377/:1389), which set
  `PC = ELR_EL1`, `CPSR = SPSR_EL1` (± the carry bit).
- **hv-sys surface:** `Vcpu::set_trap_debug_exceptions` is already wrapped (`hv-sys/src/lib.rs:94`).
  `HV_SYS_REG_MDSCR_EL1` and the full `DBGBVR/DBGBCR/DBGWVR/DBGWCR` families exist in the raw
  bindings but have **no `sysreg::` constants**; `hv_vcpu_set_trap_debug_reg_accesses` has no
  wrapper. `retrace_arch::ec_of` already decodes `Ec::SoftStep` (0x32/0x33), `Ec::Breakpoint`
  (0x30/0x31), `Ec::Watchpoint` (0x34/0x35) — **nothing consumes them yet**; a step exception today
  would fall to `run()`'s `_ =>` arm as `Stop::Other`.
- **Prior spike findings** (`spikes/README.md:30–34`): `set_trap_debug_exceptions → HV_SUCCESS`;
  **6 hardware breakpoints, 4 watchpoints; PMUVer = 0x0 (no instruction counter)** — "instruction-
  exact positioning must be software single-step". What that spike did *not* prove: end-to-end
  delivery of an actual step exception from running EL0 guest code, or how it routes (see risks).
- **Inspection accessors already exist:** `read_guest(ipa, len)` (:1497, panics if unmapped),
  `is_mapped` (:1128), `dbg_regs()` (:1536), `dbg_backtrace` (:1551), `position()` = ELR_EL1
  (:1524), `pc()` (:1526). retrace's fixed layout maps guest VA identity to IPA (the layout
  constants serve as both), so debugger addresses are canonical guest VAs used directly as IPAs.
  There is deliberately **no** general poke-memory API and M3 adds none.
- **CLI + test harness:** `main.rs` is a hand-rolled `match a.get(1)` over
  `record`/`record-dyn`/`replay` (exit codes: 2 usage, 3 divergence, 4 record error); a `debug` arm
  follows the pattern. E2e tests spawn the CLI via `util::bin()`, which ad-hoc codesigns
  `CARGO_BIN_EXE_retrace` with `retrace.entitlements` first (`tests/util/mod.rs:12–21`).
- **Scale:** a green `hello_dyn` record is ~242 events (count varies per-trace — forwarded
  entropy/pid; irrelevant here because coordinates are *per-trace*: debug sessions and their oracle
  always replay one fixed recording).

## The mechanism

### M3-spike — `spikes/sstep.c` (platform proof before architecture)

Minimal EL0 guest (a few NOPs + a loop), retrace-shaped setup (EL1 trampoline of `hvc #0`,
MMU can stay off — stepping is MMU-independent). Prove empirically, findings into `spikes/README.md`:

1. **Step delivery:** arm `MDSCR_EL1.SS` (bit 0) + `CPSR.SS` (bit 21) +
   `set_trap_debug_exceptions(true)`; run; confirm exactly one instruction retires per
   `hv_vcpu_run` and observe **which route** the step exception takes — a direct EL2 debug exit
   (`Ec::SoftStep` in the exit syndrome) or the EL1 vector → trampoline → `hvc` path
   (`Ec::Hvc` with `ESR_EL1` EC = 0x32). Both are handleable; the arm in `run()` differs.
2. **Re-arm across an emulated trap:** step onto an instruction that traps (an MRS the VMM
   emulates), resume at `+4` with SS still armed, confirm the *next* instruction yields the step
   exception — validating the step-accounting rule below.
3. **Hardware breakpoint delivery (acceleration, optional):** program `DBGBVR0/DBGBCR0_EL1` +
   `MDSCR_EL1.MDE`, run at native speed, confirm a breakpoint exception at the target PC and its
   routing. Requires the raw `hv_vcpu_set_trap_debug_reg_accesses` / DBG* sysregs — spike may use
   `hv_sys::raw` directly. If this works, `run_to_pc` goes at native speed; if not, M3 falls back
   to step-scanning (slower, still correct). **SS is required for M3; BVR is only an acceleration.**

### M3-pos — `ReplaySession` (retrace-core refactor)

Promote replay's five locals into a struct with an explicit step interface:

```rust
pub struct ReplaySession { b: Box_, events: Vec<Event>, idx: usize,
                           stdout: Vec<u8>, guest_task_port: Option<u64> }
impl ReplaySession {
    pub fn open(trace: &Path) -> Result<Self, String>;        // read events, restore snapshot
    pub fn advance(&mut self) -> Result<Advance, Divergence>; // run to next trace-bearing stop, consume ONE event
    pub fn advance_to_landmark(&mut self, n: usize) -> Result<Advance, Divergence>;
    pub fn landmark(&self) -> usize;                          // = idx
    // inspection: regs()/pc()/read_mem() delegate to Box_ accessors
}
pub enum Advance { Event, Exited { report: ReplayReport } }
```

`replay()` is reimplemented as a thin loop over `ReplaySession` — **one engine serves the oracle and
the debugger**, so the debugger can never drift from what the oracle verifies. The entire existing
dispatch chain (mach_msg2, mmap family, carveouts, …) moves into `advance` unchanged. Behavior must
be byte-identical: the full 78-test gate is the regression harness for this refactor.

### M3-step — `step_insns` (retrace-box + retrace-core)

- hv-sys: add `sysreg::MDSCR_EL1` (and `DBG*` + a `set_trap_debug_reg_accesses` wrapper if the spike
  proves BVR); one-line constants over existing raw bindings.
- `Box_::step() -> Stop`: arm SS per the spike's proven recipe, run until exactly one instruction
  retires, disarm; a new `Stop::Step` variant reports it. Emulated-below-trace instructions
  (timebase MRS, undef MRS, FPAC strip) count as **exactly one step** — they advance PC by one
  instruction identically on both any two replays — and SS is re-armed across their `+4` resume.
  Cache/reservation faults (`Stop::Other` page-ins) retire nothing and count zero; they are
  deterministic in replay, so K stays exact. `run()` itself gains a fail-loud `Ec::SoftStep` arm
  (an unarmed step exception is a bug, not a `Stop::Other`).
- `ReplaySession::step_insns(k)` / `seek(trace, n, k)`: step K instructions after landmark N;
  stepping into the window-ending trap before K is exhausted is a clean error naming the window
  length (no silent clamp).

### M3-debug — `retrace debug <trace> --script '<cmds>'`

New `main.rs` arm; semicolon-separated commands, deterministic transcript on stdout; addresses only
(no symbolication):

`break <addr>` / `delete <addr>` · `continue` · `reverse-continue` · `stepi [n]` ·
`reverse-stepi [n]` · `regs` (via `dbg_regs`) · `x <addr> <len>` · `where` (prints `(N, K)` + pc).

`continue` runs to the next breakpoint hit or exit (hardware breakpoint if proven, else step-scan);
the reverse commands are the coordinate arithmetic + `seek` described above. Errors (unmapped
address, no earlier hit, seek past exit) print deterministically and continue the script.

## Scope

**In:** the four sub-milestones above; `Stop::Step`; hv-sys debug-reg constants/wrappers; the
headline gate `reverse_debug_e2e` (parked `#[ignore]`d from day one, honest-gate discipline
unchanged); README Status + memory at close.

**Out / named, not forgotten:** checkpoints (until a guest's replay time hurts); watchpoints;
symbolication; interactive REPL (a stdin loop over the same engine — trivial follow-on);
lldb/DAP integration; any record-side change; any guest-memory *write* API; reverse-next/finish
(need symbols/frames). Parallel tracks (hygiene batch: README/CLAUDE.md refresh, commpage topology,
xpcport asymmetry gate-test; guest breadth: a printf/stdio walk) are **separate specs** — M3 shares
no files with them beyond README Status.

## Exit criterion

`reverse_debug_e2e` un-`#[ignore]`d and green: record a fresh `hello_dyn`, then a scripted session —
break at a known green-path address (e.g. the `write(1,"hi\n")` syscall site, pc `0x1804af834` in the
M2 walk logs, or `main`'s entry discovered from the recording — the loader uses fixed slides, so code
addresses are stable per-trace), `continue` to it,
`reverse-stepi`, inspect `regs`/`x`, `continue` toward exit, `reverse-continue` back to the
breakpoint — asserting exact PCs at each stop **and the entire transcript byte-identical across two
independent sessions on the same trace** (the M3 determinism oracle). `just gate` green throughout
(78 baseline + new tests, honest ignore count at each sub-milestone), clippy clean. If a
sub-milestone hits a genuine platform wall (e.g. step delivery does not work as any spike variant),
park the gate at the documented boundary — no faked green.

## Testing

1. **Spike findings first** — no architecture lands before `sstep.c` results are in
   `spikes/README.md`.
2. **M3-pos determinism:** seek landmark N twice on one `hello_dyn` recording → byte-identical
   `dbg_regs` + full-memory compare; `replay()`-over-session keeps the 78-test gate green.
3. **M3-step determinism:** seek (N, K) twice → identical; a case where the K window crosses a
   timebase MRS (step accounting over emulation); seek past window end errors with the window
   length; `Ec::SoftStep` unarmed → fail-loud test.
4. **M3-debug:** golden-transcript unit tests per command on a fixed recording; the headline
   `reverse_debug_e2e` (double-session byte-compare) via the `util::bin()` codesign-spawn pattern;
   `--test-threads=1` discipline throughout.

## Risk register

1. **Step-exception routing is the trampoline path, not a direct EL2 exit.** Then `run()`/`step()`
   see `Ec::Hvc` with `ESR_EL1` EC 0x32 — handled by matching on the inner ESR exactly as SVC is
   today. *Mitigation:* the spike decides the arm; both routes are designed for.
2. **SS interacts badly with the below-the-trace emulations** (SS state not consumed by a trapped
   instruction, lost across `CPSR = SPSR` resume). *Mitigation:* the step engine owns SS arming at
   every resume; spike item 2 proves the re-arm; the M3-step gate crosses an emulated instruction.
3. **Huge trap windows make seeks slow** (dyld init runs millions of instructions between traps).
   Positioning cost is bounded by one window's step count plus native-speed replay to N. Acceptable
   at hello_dyn scale; hardware breakpoints (if proven) accelerate `continue`; checkpoints are the
   named future fix. *No silent cap:* seeks report progress, never truncate.
4. **The ReplaySession refactor regresses replay.** *Mitigation:* pure code motion with the oracle
   loop rebuilt on top; the full gate (including `hello_dyn_e2e`) must stay green in the same task,
   and the M2 walk-history in the e2e chronicle is untouched.
5. **6-breakpoint hardware limit** (spike-verified). Fine for M3's scripted sessions; step-scan is
   the unlimited fallback; document the limit in the debug subcommand's usage text.

## Components

- `spikes/sstep.c` + `spikes/README.md` — the platform proof.
- `crates/hv-sys/src/lib.rs` — `sysreg::MDSCR_EL1` (+ optional DBG*, `set_trap_debug_reg_accesses`
  wrapper).
- `crates/retrace-box/src/lib.rs` — `Stop::Step`, `Box_::step()`, the fail-loud `Ec::SoftStep` arm
  in `run()`.
- `crates/retrace-core/src/lib.rs` — `ReplaySession` (code motion of the replay dispatch),
  `replay()` over it, `seek`/`step_insns`.
- `crates/retrace/src/main.rs` — the `debug` subcommand + script parser/executor.
- `crates/retrace/tests/reverse_debug_e2e.rs` (parked `#[ignore]`d day one) + unit/determinism
  tests; README Status + memory at close.

## Open questions for implementation planning

1. Step-exception routing (direct EL2 vs trampoline) — decided by the spike, shapes the `step()`
   arm.
2. Whether hardware breakpoints are usable under HVF (spike item 3) — decides `continue`'s engine
   (native-speed BVR vs step-scan) but not its semantics.
3. Whether `Stop::Step` is a new public variant or `step()` returns a dedicated `StepStop` type —
   decide at implementation to keep `run()`'s record-path contract untouched.
