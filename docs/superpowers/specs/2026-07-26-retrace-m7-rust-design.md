# retrace M7 — rung 1 of the breadth ladder: a real Rust binary

**Design spec — 2026-07-26.** The first post-M6 milestone. M6 closed with crash recording and
reverse-continue-to-the-corrupting-write merged (`b86fba3`, gate 136/0/0,
[M6 design](2026-07-19-retrace-m6-crash-design.md)). The debugger can now record a crashing program
and run backwards from the corpse to the bug — but every guest it has ever recorded through real
dyld was written *for this project*: `hello_dyn.c` and `crashy.c`, both hand-authored, both tiny,
both compiled by one `clang` invocation this repo controls.

M6's own spec named the next arc: **a C → Rust → brew-jq breadth ladder**, whose purpose is not new
debugger capability but finding out what breaks when the guest is no longer a fixture. M7 is rung 1:
a real Rust binary, built by the real Rust toolchain, with full `std`.

It also closes a gate-credibility gap that M6 exposed and did not itself have to face — see
"The problem, precisely".

## The problem, precisely

Two problems, and the second is the reason the first cannot be tested naively.

**1. A real Rust binary does not survive dyld today.** It dies after 240 traps without reaching
`main`, on an instruction abort whose branch target carries live PAC signature bits over a valid
shared-cache address. Details under "Verified facts".

**2. M6's crash path silently absorbs that failure as a successful recording.** This is the
important one. M6 established, deliberately and correctly, that a recorded crash is a *successful
recording* and a verified crash replay is a *successful replay* — the CLI exits `139` on both sides
and the trace is complete and self-consistent. So the obvious M7 gate —

> a real Rust binary records and replays bit-for-bit

— **passes today, on a guest that never ran a line of its own code.** Record produces a crash trace;
replay reproduces that crash trace byte-for-byte; the divergence oracle is silent because there is
genuinely no divergence. The oracle is answering "did these two runs agree?", which is not the
question "did this program run?".

This is the honest-gate principle applied one level up. Agreement between two runs is not evidence
that either run did anything, and a retrace limitation that manifests as a plausible-looking guest
crash is exactly the failure mode M6's spec warned about for stage-2 aborts ("promoting it would let
a genuine retrace IPA bug masquerade as a guest crash") — arriving here through the stage-1 door,
where M6's classification is *correct* and the absorption happens anyway.

Every breadth-ladder rung from here on inherits this hazard: the wider the guest, the more ways it
can die early while the record/replay oracle stays green. So M7 fixes the gate shape before it
relies on it.

## Verified facts (measured on this host, HEAD `b86fba3`, 2026-07-26)

**The guest, built by the real toolchain.** `rustc --target aarch64-apple-darwin` on a single-file
`fn main() { println!("hi from rust"); }`:

- **466,296 bytes** (vs `hello_dyn`'s ~16 KB).
- `MH_MAGIC_64`, `cputype ARM64`, `cpusubtype ALL` — plain arm64, not arm64e, matching the ladder's
  premise that self-built binaries are arm64.
- Flags `NOUNDEFS DYLDLINK TWOLEVEL PIE MH_HAS_TLV_DESCRIPTORS`. **`hello_dyn` has the same flags
  minus `MH_HAS_TLV_DESCRIPTORS`.**
- `LC_LOAD_DYLINKER` = `/usr/lib/dyld`; `otool -L` lists only `/usr/lib/libSystem.B.dylib`.
- **`hello_rust` has ZERO `LC_DYLD_CHAINED_FIXUPS` load commands; `hello_dyn` has one.** The Rust
  toolchain emits *classic* `LC_DYLD_INFO`-style rebase/bind opcodes. Every dynamic guest this repo
  has ever recorded used chained fixups, so the classic bind path in dyld has never been exercised.
- Runs correctly outside retrace: prints `hi from rust`.

**The failure.** `RETRACE_TRACE=1 cargo run -p retrace -- record-dyn hello_rust`:

- Exit **139** — a recorded crash outcome, i.e. M6 classified it as a guest crash.
- **240 dispatched traps** (exact `[trap]` count). For scale, the M2 milestone log records `hello_dyn`
  reaching ~242 traps as its wall-chain closed — not re-measured here, so treat the comparison as
  indicative: the loader gets essentially as deep before dying.
- **`hi from rust` never appears in the output** — 0 occurrences. `main` is never reached.
- `guest crashed: pc=0x67c0001800fc388 far=0x67c0001800fc388 esr=0x82000004`

**The crash, decoded.**

| Field | Value | Meaning |
| --- | --- | --- |
| `ESR` EC | `0x20` | Instruction abort, **lower EL** (so: inner funnel, a genuine `Stop::Fault`) |
| `ESR` IL | `1` | 32-bit instruction |
| `ESR` IFSC | `0b000100` | Translation fault, **level 0** |
| PC bits[46:0] | `0x1800fc388` | A valid shared-cache address (cache base `0x180000000` + `0xfc388`) |
| PC bits[63:47] | `0xcf8` | Garbage — the shape of live PAC signature bits |
| PC top byte | `0x6` | Ignored for translation under TBI, still reported in FAR |

`far == pc`, as expected for an instruction abort. The low 47 bits address real cache code; the
high bits are a signature that was never authenticated or stripped. **The guest branched through a
signed pointer as if it were raw.** Same family as the M2-bfam wall, at a new consumer.

**The immediate context.** The last traps before the fault are dyld patching the shared cache. The run
issues **47** `_kernelrpc_mach_vm_protect_trap` calls (svc `-14`) in total, **27** of them on
`0x1ec444000` size `0x24000` — including 8 with `prot=0x3` (RW) and 8 with `prot=0x1` (R) in an
alternating write-enable / write-disable pattern — interleaved with `_kernelrpc_mach_vm_map_trap`
(`-15`) and `_kernelrpc_mach_vm_deallocate_trap` (`-12`). The fault follows immediately after the
last such flip.

**And `mach_vm_protect` is serviced as a no-op success** (`retrace-core/src/lib.rs:239-242`:
"no-op success. Stage-2 stays RWX; stage-1 W^X is already correct"), recorded as a `Syscall` event
with `ret: 0` and no writes.

## Hypotheses to test (NOT facts — task 2 discriminates)

Ranked by the evidence above. The spec deliberately does not choose; the diagnosis does.

1. **Dropped cache patch.** dyld flips a cache region writable to patch it, retrace no-ops the
   protect, and dyld's subsequent write to a pointer slot is lost or lands somewhere inert. The
   consumer then branches through the slot's *unpatched* value — which retrace's cache re-signer had
   signed with the guest key — using a plain `br`/`blr` that performs no authentication. Strongest
   evidence: the protect flips target the cache and immediately precede the fault.
2. **Classic-bind slots must stay raw.** retrace's re-signer signs cache auth slots so that guest
   `braa`/`autda` authenticate correctly. If the classic `LC_DYLD_INFO` bind path writes and consumes
   raw pointers where the chained-fixup path uses authenticated ones, then re-signing a slot this
   binary consumes raw produces exactly the observed garbled branch. Strongest evidence: the
   zero-vs-one chained-fixups difference is the sharpest structural difference found.
3. **TLV descriptor thunk.** `MH_HAS_TLV_DESCRIPTORS` means dyld runs TLV setup, whose descriptors
   hold a function pointer called through a thunk. Weakest of the three — a real difference, but no
   evidence yet ties it to this branch site.

These are not mutually exclusive; 1 and 2 could be the same bug seen from two sides.

## The mechanism

Four components. Three are small and fully specified here; the fourth is deliberately not.

### M7-gate — the credibility fix, first

A "the guest actually ran" assertion class, so no breadth-ladder gate can be satisfied by a recorded
crash. Concretely a shared test helper asserting, for a rung guest:

- `Outcome::Exit { code: 0 }` — not merely a terminal outcome, and explicitly not `Outcome::Crash`
- the guest's expected stdout, exactly
- replay byte-identical to the recording, twice

M6 already made this checkable rather than aspirational: removing `exit_code` in favour of `Outcome`
means a crash cannot be mistaken for `code: 0` — several M6 tests were strengthened by exactly this
property. M7's helper makes that shape the *default* for rung guests instead of something each test
remembers to do, and a guard test pins the discrimination so a future rung cannot quietly regress to
asserting agreement alone.

This lands first because it is the instrument the rest of the milestone is judged by.

### M7-guest — `hello_rust`

`crates/retrace-guest/rs/hello_rust.rs` plus one `rustc` invocation in `build.rs`, mirroring the
existing one-`Command`-per-guest pattern, and a `HELLO_RUST` path const. `rustc` on a single file
takes no cargo lock, so there is no build recursion; the toolchain is already pinned (1.95.0,
`aarch64-apple-darwin`).

Full `std` and `println!` are the point — they drag in `std::rt` init, the stdout lock, and the
stack guard, none of which a hand-written C fixture exercises. Deliberately no `-C opt-level`
tuning and no `panic=abort`: the goal is what the toolchain emits by default.

### M7-trace — `Stop::Fault` in `RETRACE_TRACE`

`RETRACE_TRACE=1` filters to `Stop::Syscall` (`retrace-core/src/lib.rs:73-94`), so the one trap that
matters here is the one it cannot show — while `CLAUDE.md` advertises the flag as logging "every
dispatched trap". This is a parked M6 follow-up minor promoted onto M7's critical path, because the
diagnosis needs the branch site and the register state at the fault.

M6's crash park is the other half of the instrument: `retrace debug` can `continue` to this fault and
`where` it, which makes M6 its own successor's diagnostic tool.

### M7-wall — the PAC-garbled branch

**Mechanism deliberately unwritten.** Choosing one now would mean inventing it: the three hypotheses
above imply materially different fixes in different crates, and this repo's own practice (the M2
chain, `spikes/`) is to diagnose against the real OS and then make a deliberate route decision.

The route decision itself is the first thing the diagnosis must produce, because the repo's two
symmetry rules point at different places:

- **Rule 1** — a record-side special case needs a mirrored replay arm recomputing identical bytes,
  with replay's byte-compare serving as the divergence check. Correct if the fix must observe or
  record something.
- **Rule 2** — deterministic emulation belongs *below* the trace inside `Box_::run()`, shared by
  record and replay, where determinism is automatic. Correct if the fix is a pointer
  strip/re-sign, which is how both prior PAC walls (M2-cache re-signing, M2-bfam strip-on-FPAC) were
  resolved. This is the more likely home.

## Correctness invariant

Whatever the fix, it may not weaken these:

- **Determinism.** Nothing nondeterministic enters the trace. A re-signed or stripped pointer must be
  a pure function of (file bytes, fixed slide, fixed keys), identical on both runs — the standing
  posture for all cache-derived state.
- **Never reimplement Apple's PAC.** Signing and authentication run real `pac*`/`aut*` on the guest
  vCPU with fixed keys.
- **`mach_vm_protect`'s no-op must stay sound, or stop being a no-op deliberately.** If hypothesis 1
  holds, the fix changes a servicing decision that has been load-bearing since M2. Any change here is
  a route decision with its own record/replay symmetry consequences, not an incidental edit.
- **No M6 regression.** The crash path, both funnels, `wildstore.s`'s fatal stage-2 negative, and the
  136/0/0 gate all stand.

## Scope

**In:** the `hello_rust` guest; the "guest actually ran" assertion class and its guard test;
`Stop::Fault` in `RETRACE_TRACE`; diagnosis of the PAC-garbled branch and its fix; the headline gate;
a README M7 Status section.

**Out (explicit):**

- **The `brew jq` rung** — M8. M7 deliberately does one rung.
- **Rust `panic!`.** `panic` → `abort()` → `SIGABRT` lands on M6's deferred *signal delivery*
  boundary, not the `Stop::Fault` path. A panicking Rust guest is a separate milestone's question.
- **Threads.** `Sched` stays unused. If `std` init spawns a thread, that is a **hard stop and a
  re-park**, not something M7 absorbs — threads are a milestone, not a task.
- arm64e guests; `rwatch`/`awatch`; watch ranges > 8 bytes; old→new value printing; unclaimed stage-2
  aborts staying fatal — all unchanged from M5/M6.

## Exit criterion

The headline gate `hello_rust_e2e::hello_rust_records_and_replays_reaching_main`, a new integration
test **born `#[ignore]`d** per honest-gate discipline and un-ignored only on a genuine double pass:
`record-dyn` of `HELLO_RUST` yields `Outcome::Exit { code: 0 }` with stdout exactly
`"hi from rust\n"`; two `replay`s are byte-identical to the recording including the final
full-memory comparison. `just gate` stays 0 failed / 0 ignored at close, and the README gains an M7
Status section re-documenting the next boundary.

**If the wall does not fall inside this milestone, the gate stays `#[ignore]`d** with the wall named
in its ignore reason and in the README. That is a legitimate M7 outcome and the discipline working as
intended — not a failure, and not a reason to loosen the gate.

## Testing

- **Unit:** the `Stop::Fault` trace-log arm; `HELLO_RUST` parses as a Mach-O with an entry inside an
  executable segment (the existing per-guest parse-test pattern).
- **The guard test for the credibility fix:** a recorded *crash* outcome must FAIL the rung
  assertion. Without this, M7-gate is a claim rather than a mechanism — and the whole reason M7-gate
  exists is that a plausible-looking green fooled the obvious gate.
- **Regression:** the full M6 surface — `crashy_e2e`, `crashy_cli`, `crash`, `vaipa`, `watch_dyn` —
  plus `hello_dyn_e2e`, unchanged. Any fix touching the cache re-signer or `mach_vm_protect` must
  keep `cache_pager` and `reservecommit` green.
- **Headline:** the exit criterion above, double-passed.
- All tests run `--test-threads=1`; a test spawning the CLI goes through
  `crates/retrace/tests/util/mod.rs::bin()`.

## Risk register

- **R1 — the wall may be a chain.** M2's equivalent took ten sub-milestones. Mitigation: `M7-xxx`
  sub-milestones, the gate parked honestly between them, each wall diagnosed before it is routed.
  Probability: moderate. This is the ladder's stated purpose — finding walls is success, not failure.
- **R2 — `std` init may spawn a thread.** Would collide with `Sched` being unused and threads being
  out of scope. Mitigation: hard stop and re-park; do not improvise a scheduler. Probability: low for
  a single-threaded `main`, but Rust's runtime init is not fully surveyed.
- **R3 — hypothesis 1 implicates `mach_vm_protect`'s no-op**, load-bearing since M2. Changing it
  touches cache paging and reservation commit. Mitigation: treat as a route decision with its own
  symmetry analysis and full regression, not a local edit.
- **R4 — the classic-bind path may be broad**, not one pointer. If dyld's whole classic bind
  machinery is unexercised, the first fix may reveal a second. Mitigation: the diagnosis reports the
  *class* of the defect, not just the instance.
- **R5 — 466 KB and more cache pages** may stress loader paths (segment handling, page-in volume).
  Mitigation: none needed yet — 240 traps says the loader is already coping.

## Components

- `crates/retrace-guest/rs/hello_rust.rs` — NEW; `build.rs` + `src/lib.rs` const (M7-guest).
- `crates/retrace/tests/util/mod.rs` — the rung assertion helper (M7-gate).
- `crates/retrace-core/src/lib.rs` — `Stop::Fault` in the trace-log filter (M7-trace).
- `crates/retrace/tests/hello_rust_e2e.rs` — NEW; headline gate + the credibility guard test.
- The fix's home — `retrace-box` most likely (cache re-signer or a strip-on-consume arm), decided by
  diagnosis (M7-wall).
- `README.md` — M7 Status section.

## Open questions for implementation planning

1. **Which hypothesis holds?** Task-2 diagnosis, from the branch site and register state at the
   fault. Everything downstream depends on the answer; nothing downstream should be planned in
   detail before it.
2. **Rule 1 or rule 2 for the fix?** Follows from (1). Prior PAC walls both landed below the trace.
3. **Does the guard test need a dedicated crashing Rust guest,** or can it reuse M6's `CRASHY`?
   Reusing `CRASHY` is cheaper and tests the same discrimination; a Rust-specific crash guest would
   drift toward the out-of-scope `panic!` question.
4. **Where does the rung helper live** — `util/mod.rs` beside `record_dynamic`/`replay`, or its own
   module? Depends on how much the ladder is expected to grow; `util/mod.rs` is the existing home for
   shared test scaffolding.
