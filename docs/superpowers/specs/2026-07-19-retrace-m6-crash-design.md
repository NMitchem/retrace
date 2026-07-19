# retrace M6 — record a real crash, reverse-continue to the corrupting write

**Design spec — 2026-07-19.** The first post-M5 milestone. M5 closed with write watchpoints and
reverse-continue-to-last-writer merged (`49f9e7d`, gate 121/0/0 incl. fast-follow,
[M5 design](2026-07-18-retrace-m5-watchpoints-design.md)). The debugger can move cheaply in both
directions, stop on instruction addresses, and answer "who wrote this byte?" — but only for guests
that run to a clean `exit`. It cannot yet do the one thing every reverse debugger exists to do in
practice: **record a program that crashes, then run backwards from the crash to the bug.** M6 makes
guest synchronous faults (the SIGSEGV class) first-class recorded, replayed, seekable stops, makes
the software watch path sound on MMU-on guests (the M5 VA→IPA deferral), and proves the end-to-end
story on a planted-bug dynamically-linked C program: record the crash, `watch` the corrupted
pointer, `reverse-continue`, land on the out-of-bounds store that corrupted it.

M6 is the first milestone of the "practically useful" arc settled in brainstorming: arm64
self-built programs (never arm64e — all Apple-shipped binaries are arm64e; Homebrew/self-compiled
are arm64), a C → Rust → brew-jq breadth ladder as follow-on milestones, open-sourcing after. This
spec covers only the crash core.

## The problem, precisely

Today a guest fault kills the run with an error. Both record and replay treat the two abort funnels
like this:

1. **Stage-1 EL0 faults** (wild pointer, NULL deref, jump to garbage) are delivered to the guest's
   EL1 trampoline (VBAR), which HVCs out; `run()`'s inner match on `ec_of(ESR_EL1)` has no arm for
   them, so they surface as the generic `Stop::Other { esr: esr1 }` diagnosis bucket
   (`retrace-box/src/lib.rs:1371-1372`), which the dispatch then tries to page-in
   (`page_in_cache` / `commit_reserved_page`, `retrace-core/src/lib.rs:357-358`) and, failing
   both, aborts the run with an "unexpected stop" error.
2. **Stage-2 aborts** (outer `Ec::DataAbort`, `retrace-box/src/lib.rs:1389-1392`) are the
   below-the-trace demand paths — shared-cache page-in and reserved-page commit — and, when
   unclaimed, the fail-loud wild-store error (`asm/wildstore.s`).

So a crashing guest is indistinguishable from a retrace bug, nothing about the crash enters the
trace, and there is no position "at the crash" to seek to or debug from. Separately, the M5
software watch check intersects an armed watch **VA** against a recorded write's **IPA**
(`watch_ranges` / `syscall_watch_hit`, `retrace-box/src/lib.rs:267-268`) — exact only for
identity-mapped static guests; on an MMU-on guest it silently compares different address spaces,
which undermines exactly the "who wrote this" answer the crash demo depends on.

M6 fixes both: stage-1 EL0 data/instruction aborts become `Stop::Fault`, recorded as a terminal
`Event::Crash` and byte-verified on replay; the software watch check translates armed VAs through
the guest's own page tables before intersecting.

## Verified facts (this repo — read directly, HEAD `49f9e7d`)

- **The two abort funnels are already structurally distinct.** Guest EL0 exceptions arrive via the
  EL1 trampoline as the outer `Ec::Hvc` arm's inner match on `ec_of(ESR_EL1)`
  (`retrace-box/src/lib.rs:1349-1372`); stage-2 aborts arrive as the **outer** `Ec::DataAbort` arm
  with `last_far = e.virtual_address` (an IPA) (`:1389-1392`). Crashes are inner; demand paging is
  outer. No disambiguation logic is needed — the seam already exists. The same inner structure
  exists in the single-step path `run_one_for_step` (`:1425`, generic fallthrough `:1447-1448`).
- **`retrace-arch` already decodes data aborts.** `0x24 | 0x25 => Ec::DataAbort`
  (`retrace-arch/src/lib.rs:35`). Instruction aborts (`0x20 | 0x21`) currently fall to
  `Ec::Other` — one enum variant + one match line to add. Note `ec_of` folds the lower-EL and
  same-EL forms together; the fault arm must additionally require the lower-EL form (EC bit 0
  clear): a *same-EL* abort means the trampoline itself faulted — a retrace bug that must stay in
  the fail-loud `Stop::Other` path.
- **`Stop` is small and additive.** `pub enum Stop { Syscall {..}, Other { esr }, Step }`
  (`retrace-box/src/lib.rs:272`). A new variant is compiler-enforced at every dispatch site.
- **The trace format has headroom and a bump procedure.** `Event` is
  `Snapshot / Syscall / Sched / Exit` (`retrace-trace/src/lib.rs:14-19`; `Sched` is unused
  threads future-proofing), `TRACE_MAGIC = "RT\x00\x03"` (`:21`, last bumped M2-mach). Changing
  `Event`'s shape is a declared format break — bump to `0x04`. On exit, record appends
  `Event::Exit` then the final full-memory snapshot (`retrace-core/src/lib.rs:93`); `Crash`
  mirrors that order.
- **Replay's terminal path is the template.** `advance()` handles `Some(Event::Exit { code })`
  with a final-memory compare and returns `Advance::Exited(ReplayReport)`
  (`retrace-core/src/lib.rs:456-466`); `Advance` is
  `Event / Exited / Break / Watch / WatchSyscall` (`:409`). Terminal-position handling exists in
  `checkpointed_seek` (`:935`) and plain `replay()` (`:976`).
- **The M5 watch machinery is one funnel with one comparison to fix.** Armed ranges and the
  first-overlap hit live in `Box_` (`watch_ranges` / `syscall_watch_hit`,
  `retrace-box/src/lib.rs:267-268`); the intersection runs in `apply_and_return`'s write loop.
  The hardware path (DBGWVR/DBGWCR, 4 slots, `HW_WATCHPOINT_SLOTS` `:126`) compares **VAs in the
  guest's own translation regime** and needs no change.
- **No read-walk helper exists yet.** The stage-1 geometry is fixed and documented — 47-bit VA,
  T0SZ=17, 3-level 16 KiB-granule walk TTBR0→L1→L2→L3 (`retrace-box/src/lib.rs:409`); the box owns
  the table-building state (`l2_host`, `next_l3`) but has no VA→IPA *reader*. Naturally-aligned
  ≤8-byte watch ranges never cross a 16 KiB page, so one translation per armed range suffices.
- **The stage-2 fail-loud negative is load-bearing and must not weaken.** `asm/wildstore.s` stores
  to an unbacked, unreserved IPA and its test asserts `commit_reserved_page` refuses and the run
  stays fatal. M6 deliberately does **not** reclassify unclaimed stage-2 aborts as crashes (see
  Scope).
- **The fixture pattern is proven.** `build.rs` compiles `c/hello_dyn.c` with the normal toolchain
  (`crates/retrace-guest/build.rs:147-156`); path constants in `retrace-guest/src/lib.rs:86-108`.
  `hello_dyn` is write-only — no existing dynamic guest performs a `read()` into a known buffer,
  so the VA→IPA syscall-write proof needs the new fixture to do one.
- **Debug CLI surfaces to extend.** `cmd_continue` (`crates/retrace/src/debug.rs:326`),
  `cmd_reverse_continue` (`:455`, scan hits ordered by `(N,K)`), `resolve_hit_k` (`:140`),
  `cmd_where` (`:266`). All `Advance` consumers are compiler-visible match sites
  (`:353-434`, `:469-477`; plus `advance_to_landmark` `retrace-core/src/lib.rs:693` and
  `replay()` `:976`).

## The mechanism

### M6-trace — the format break, first

`Event::Crash { pc: u64, esr: u64, far: u64 }`, a terminal event parallel to `Exit`, followed by
the same final full-memory snapshot. `TRACE_MAGIC` bumps `0x03 → 0x04` in the same task, at the
start of the milestone, so the format breaks exactly once. No test fixture stores traces on disk
(house convention: every test records fresh), so the bump is self-contained.

### M6-arch — instruction-abort decode

`Ec::InstrAbort`, `0x20 | 0x21 => Ec::InstrAbort`. Two lines plus a decode unit test.

### M6-fault — `Stop::Fault` (retrace-box)

New variant `Stop::Fault { pc: u64, esr: u64, far: u64 }`. In **both** inner matches (`run()`'s
`Ec::Hvc` arm and `run_one_for_step`'s), before the generic fallthrough:

- `Ec::DataAbort | Ec::InstrAbort` **and** EC bit 0 clear (lower-EL form) → capture
  `far = FAR_EL1`, `pc = ELR_EL1` (the vCPU's PC at exit is the trampoline; the faulting EL0 PC is
  in `ELR_EL1`), `esr = esr1`, return `Stop::Fault`.
- Same-EL forms (`0x25`/`0x21`) keep falling through to `Stop::Other` — a trampoline fault is a
  retrace bug, fail-loud.

The outer stage-2 arm is untouched: cache page-in, reserved-page commit, and the wildstore fatal
path behave exactly as today.

### M6-dispatch — record & replay, symmetry rule 1 (retrace-core)

**Record** (`record_box`): `Stop::Fault { pc, esr, far }` → append `Event::Crash { pc, esr, far }`
+ final snapshot, close the trace. Recording a crash is a *successful recording* — not an `Err` —
and `RecordSummary` gains an outcome (`Exit { code }` vs `Crash { pc, esr, far }`) which the CLI
prints.

**Replay** (`ReplaySession::advance`): on `Stop::Fault`, the next recorded event must be `Crash`
and the triple `(pc, esr, far)` must byte-match — that comparison **is** the divergence check
(recorded crash but clean replay exit, replay fault at a different triple, or replay fault while
events remain are all loud `Divergence` errors). Then the final-memory compare runs exactly as in
the `Exit` path. A new compiler-enforced variant `Advance::Crashed(ReplayReport)` surfaces it
(`ReplayReport` gains the same outcome field); every existing `Advance` match site gains an arm.
`checkpointed_seek` treats `Crash` as end-of-trace exactly like `Exit`: the crash is an ordinary,
seekable, terminal `(N, K)` position. Plain `retrace replay` of a crash trace succeeds and reports
the verified crash.

The fault is a pure function of guest state, so it happens at the same instruction with the same
triple on both runs — determinism-by-construction, no new record/replay asymmetry, no new posture.

### M6-vaipa — sound software watch on MMU-on guests (retrace-box)

- `Box_` gains `mmu_on: bool` (set by the constructors: `load` false, `load_dynamic` true — cheap,
  deterministic, no sysreg read per check) and a read-only walker
  `va_to_ipa(&self, va: u64) -> Option<u64>`: MMU off → identity; MMU on → 3-level read of the
  guest's own tables; unmapped at any level → `None`.
- `apply_and_return`'s intersection translates each armed watch VA at check time (per event, so
  later remaps are naturally honored) and intersects the resulting IPA range against the recorded
  write's IPA range. `None` (armed VA unmapped) → no match, which is sound: an unmapped VA cannot
  be the destination of an applied write.
- Detection remains observation-only (the M5 invariant): the copy is never skipped or altered;
  translation changes only *which hits are reported*, never what executes or what enters the trace.
  On record and plain replay-with-no-watches the added cost stays an is-empty check.
- The hardware DBGW path is untouched — its comparators already operate on guest VAs.

### M6-fixture — `crashy.c` (retrace-guest)

`c/crashy.c`, compiled exactly like `hello_dyn` (same `build.rs` clang recipe, new `CRASHY` path
constant). Program shape, all deterministic:

1. Early: `read()` a few bytes from a fixture file into a **global buffer** — this is the VA→IPA
   syscall-write target (a kernel write into a watchable global of an MMU-on guest).
2. A global struct places `long buf[N]` directly before `long *ptr` (initialized to a valid
   address). A loop with a planted off-by-one writes a recognizable garbage constant through
   `buf[N]`, corrupting `ptr` — the ordinary guest store the demo must find.
3. Later, a store through `*ptr` faults: the garbage constant is chosen unmapped-in-stage-1 (any
   unmapped VA faults deterministically; the constant just makes transcripts readable).

Tests discover all addresses and coordinates from the freshly-recorded trace at test time — never
hardcoded (house convention).

### M6-cli — debug & record surfaces (retrace)

- `record` / `record-dyn` print the crash outcome (`guest crashed: pc=… far=… esr=…`) on success.
- `retrace debug`: the crash is the last seekable position; `where` there prints the crash line;
  `continue` reaching it prints crash-and-stop (`Advance::Crashed` arm); `reverse-continue`
  treats it as the scan terminal exactly like `Exited`. The demo flow is then entirely existing
  machinery: `watch <&ptr> 8` + `reverse-continue` → the hardware comparator catches the
  out-of-bounds store (a plain guest store) → `resolve_hit_k` lands on its exact `(N, K)`.

## Correctness invariant

A guest synchronous fault is deterministic: identical guest state ⇒ identical `(pc, esr, far)` on
record and replay, so the recorded triple is byte-comparable and every mismatch is a loud
`Divergence` — the crash *extends* the oracle rather than weakening it. All M6 changes are
additive widenings: no path that works today takes a different route (the fault arms fire only on
ECs that are fatal errors today; the VA→IPA translation only re-addresses a comparison that was
silently wrong on MMU-on guests). Watch detection remains observation-only.

## Scope

**In:** stage-1 EL0 data/instruction aborts as recorded, replayed, seekable crashes; the VA→IPA
software-watch fix; `crashy.c` + the headline gate; CLI surfaces above.

**Out (explicit):**

- **Signal delivery.** The guest's `sigaction` handlers never execute; a fault is terminal (rr's
  default for fatal signals). `sigaction`/`sigaltstack` *calls* keep recording as ordinary
  forwarded syscalls.
- **Unclaimed stage-2 aborts stay fatal errors** (wildstore semantics unchanged). Consequence,
  documented as a known limitation: a use-after-free store into a deallocated carveout hole
  manifests as stage-2 and still kills the run loudly instead of recording a crash. Promoting it
  would let genuine retrace IPA bugs masquerade as guest crashes; revisit only with a
  reservations-aware classifier.
- arm64e guests; threads (`Sched` stays unused); `rwatch`/`awatch`; watch ranges > 8 bytes;
  old→new value printing; the C/Rust/jq breadth rungs (M7+); all open-sourcing work.

## Exit criterion

The headline gate `crashy_e2e` (new integration test, **born `#[ignore]`d** per honest-gate
discipline, un-ignored only on a genuine double pass): `record-dyn` of `CRASHY` reports a crash
outcome; plain `replay` verifies the trace bit-for-bit including the crash triple (double-replayed);
a scripted `retrace debug` session seeks to the crash, `where` reports it, arms `watch` on the
corrupted pointer, `reverse-continue`s, and the reported hit is the out-of-bounds store's `(N, K)`
— asserted against the trace-discovered corrupting-store PC, golden transcript byte-compared.
`just gate` stays 0 failed / 0 ignored at close, README gains an M6 Status section re-documenting
the next boundary.

## Testing

TDD throughout, in dependency order:

1. **retrace-arch unit:** `0x20 | 0x21 => Ec::InstrAbort` decode.
2. **retrace-trace unit:** `Crash` roundtrip; `0x04` magic rejects an `0x03` trace; torn tail
   ending in a partial `Crash`/snapshot is dropped by `open_checked`.
3. **Box-level fault test:** an MMU-on guest takes a stage-1 fault → `Stop::Fault` with the
   expected `(pc, esr, far)`; both data-abort and instruction-abort flavors. Harness choice is
   Open question 1.
4. **Classification regressions:** the existing wildstore fatal test keeps passing unchanged
   (stage-2 unclaimed ≠ crash); reserved-page first-touch still demand-commits invisibly.
5. **Record/replay e2e:** `crashy` records with a crash outcome, replays bit-for-bit, double
   replay; a divergence test that re-writes the recorded `Crash` event with a perturbed triple
   *via `Writer`* (valid CRC — a raw byte flip would fail the record CRC before the compare) →
   loud `Divergence`.
6. **VA→IPA:** walker unit test against the fixed guest layout (known VA→IPA pairs incl. an
   unmapped VA → `None`; MMU-off identity); session-level: `watch` on `crashy`'s global read
   buffer fires `Advance::WatchSyscall` for the `read()` — the M5-deferral proof on a real
   MMU-on guest.
7. **Debug CLI golden transcripts:** `where` at the crash; `continue` into `Crashed`;
   the full demo script (watch + reverse-continue → corrupting store).
8. **Headline gate** per Exit criterion.

## Risk register

- **R1 — inner-delivery ISS determinism.** EL0 aborts ride the same proven trampoline as syscalls
  and FPAC, but no *data abort* has crossed it yet; `ESR_EL1` ISS bits (DFSC etc.) are expected
  architecturally deterministic for an identical fault. The first RED box test observes the real
  triple before anything depends on it; any residual variance would surface as a loud divergence,
  never silent corruption. Fallback if a truly nondeterministic ISS bit exists: mask it out of
  both the event and the compare, documented.
- **R2 — fixture fragility.** dyld layout drift could someday map the garbage VA. Mitigation: the
  constant is chosen far from every fixed region and tests assert the *crash outcome*, discovering
  addresses from the trace, so drift breaks tests loudly, not silently.
- **R3 — `ELR_EL1` fidelity.** The trampoline must not clobber `ELR_EL1` before the HVC exit; it
  already preserves it for syscall returns (replay feeds recorded returns back through the same
  path). Verified by the box test's PC assertion against the fixture's faulting instruction.
- **R4 — walk-vs-remap staleness.** The VA→IPA walk happens at check time inside the event being
  applied, reading the tables as they are at that instant — later remaps re-translate naturally.
  No caching in M6 (three guest-memory reads per armed range per syscall event is noise).

## Components

- `retrace-arch`: `Ec::InstrAbort` (+decode test).
- `retrace-trace`: `Event::Crash`, magic `0x04` (+roundtrip/reject/torn tests).
- `retrace-box`: `Stop::Fault` + two inner-match arms; `mmu_on`; `va_to_ipa`; translated
  intersection in `apply_and_return`.
- `retrace-core`: record crash path + `RecordSummary` outcome; `advance()` crash verify +
  `Advance::Crashed` + `ReplayReport` outcome; `checkpointed_seek` terminal handling.
- `retrace` (CLI): record outcome print; `debug.rs` arms for `Crashed`; `where` crash display;
  `crashy_e2e` + golden transcripts.
- `retrace-guest`: `c/crashy.c` + `CRASHY` const + fixture file.

## Open questions for implementation planning

1. **Lightest box-level harness for a stage-1 fault test.** Options: reuse the `load_dynamic`
   path with `crashy` (heavier but zero new infrastructure) vs. a minimal MMU-on static guest if
   the existing static loader can run with translation enabled. Planner should check how
   `strip47`'s harness runs and pick the cheaper one.
2. **Outcome type shape.** One shared `Outcome { Exit { code }, Crash { pc, esr, far } }` used by
   both `RecordSummary` and `ReplayReport`, or per-struct fields. Prefer shared unless it drags a
   dependency the wrong way.
3. **Transcript wording** for the crash lines (`where`/`continue`/record print) — fix exact
   strings at golden-test time.
