# M21-stackgrow design

**Goal:** a deep Rust recursion strikes its own guard page and reports `has overflowed its stack`,
instead of running off the backed stack into unbacked IPA and killing the recorder with a stage-2
fault — or park a gate at a wall M21 actually reached and measured.

**Prior measurements** are M8's, and they are already in the tree rather than in a companion file:
the `DYN_STACK_TOP` comment at `crates/retrace-box/src/lib.rs:37-56` and the `#[ignore]` reason on
`crates/retrace/tests/stackoverflow_rust_e2e.rs:10`. They are cited below as **R3-a** … **R3-d**.
The measurements M21 still owes are enumerated under "Measurements owed (t0)" and cited **T0-1** …
**T0-4**; no section below may be treated as settled where it cites one.

## The problem, precisely

The guest **believes** it has an 8 MiB stack and retrace **backs** 256 KiB of it.

libstd's `install_main_guard` mmaps its stack-overflow guard page `MAP_FIXED PROT_NONE` at
`pthread_get_stackaddr_np() - pthread_get_stacksize_np()`. macOS 26's libpthread reports the main
thread's size as a **constant `0x7fc000`** (**R3-a**), and retrace cannot influence that subtrahend:
M8 measured that answering `getrlimit(RLIMIT_STACK)` with `0x10000000` instead of `0x40000` left the
computed address **bit-identical** (**R3-b**). So the guard lands at `0x2004000`, while the backed
stack bottom is `DYN_STACK_TOP - DYN_STACK_SIZE` = `0x2800000 - 0x40000` = `0x27C0000` — **7.72 MiB
above it**.

A recursion therefore walks SP down out of the backing and touches unmapped IPA *long before*
reaching the guard. That is a stage-2 translation fault with no reservation behind it: a fatal
`describe_stop`, not a guest-visible signal. Forced with `--ignored`, the run dies exactly there
(**R3-c**):

```
RECORD ERROR: non-syscall exit: data abort (EC=0x24 ISS=0x1c08047 FSC=0x7)
far/ipa=0x27bff60 (UNMAPPED)
```

160 bytes below the stack bottom — nowhere near the `0x2004000` guard.

This is not merely a missing niceness. Any guest whose recursion exceeds 256 KiB is **un-recordable**:
the tool dies, rather than the guest.

## What is already rejected, and why it stays rejected

- **Back the full 8 MiB eagerly.** Measured at **~1.7x on `hello_rust` and far worse across the dyld
  suite** (**R3-d**). The cost law behind that number is the load-bearing part: *the per-syscall
  memory diff scales with total **mapped** memory.* M21 must not reintroduce it.
- **Move the subtrahend via `getrlimit`.** Foreclosed by **R3-b** — the reply is ignored.

Neither rejection touches a third option, which is what M21 takes — and which is not novel: **M18
already shipped it for a different stack.**

## The precedent: M21 is M18's worker-stack pattern, applied to the main thread

`place_worker_stack` (`retrace-box/src/lib.rs:3700`) gives each workqueue worker a ~0.5 MiB stack
that is **reserved, not mapped**, and grown page-by-page by `commit_reserved_page`. Its doc comment
states M21's own cost argument, in M8's words:

> **A reservation, not an `mmap`.** `guest_vm_reserve` is bookkeeping only, and each page is
> demand-committed with a fresh zeroed anon page on first touch by `commit_reserved_page` — the path
> both dispatch loops already run for the guest's own reservations. That satisfies the zero
> requirement by construction, and it keeps the ~0.5 MiB a worker is *given* from being ~0.5 MiB
> every subsequent syscall has to diff (M8-stack's measured lesson: growing a guest region costs
> per-syscall diff time).

And it makes M21's guard-page decision, for the same reason:

> Skip one granule so the guard page below `x2` lands in a hole no reservation covers.

pinned by `workq_reqthreads_leaves_an_uncommittable_guard_page_below_each_worker_stack`
(`retrace-box/tests/threads.rs:1241`), which asserts **refusal and a non-vacuous positive control
both** — `commit_reserved_page(w1)` true, `commit_reserved_page(w1 - GRANULE)` false.

This changes M21's risk profile and its shape. The mechanism is not a new idea to be validated; it is
a **shipped, tested pattern extended to the one stack that never received it**, because the main
thread's stack is built by `load_dynamic` rather than handed out by the workqueue emulator. M21's
tests should mirror `threads.rs:1241`'s structure rather than invent one.

## The mechanism

**Reserve the believed-stack window; let the existing demand-committer grow into it.**

`guest_vm_reserve(addr, size, anywhere=false)` is bookkeeping only — no host allocation, no stage-2
map, one `(base, len)` pushed onto `reservations`, deterministic by construction
(`retrace-box/src/lib.rs:1803`). `commit_reserved_page` then backs exactly one zeroed page per
stage-2 fault inside a tracked reservation, and **refuses everything outside one**
(`retrace-box/src/lib.rs:1271`).

So M21 is one call plus its tests. `load_dynamic` reserves

```
GUARD_TOP         = 0x0200_8000   // one granule ABOVE libstd's guard page at 0x2004000
DYN_STACK_BOTTOM  = 0x027C_0000   // DYN_STACK_TOP - DYN_STACK_SIZE
window            = 0x007B_8000   // 7.72 MiB
```

and nothing else changes. The four existing dispatch sites that already call
`commit_reserved_page(fault_ipa())` on a stage-2 fault (`retrace-core/src/lib.rs:1131` on record;
`:2343`, `:2505`, `:2533` on replay) service the growth without modification — **but only since task 2.5**. As first written this was false: replay never runs `load_dynamic`, and `Box_::restore` reset `reservations` to empty, so those arms saw no reservation covering the fault and returned `Divergence`. `restore` now re-establishes the reservation.

### The two fault routes are the whole design

The reservation stops **one granule above** the guard page, deliberately. That single decision is
what makes the mechanism work, and it is worth stating as a sequence:

1. SP descends below `0x27C0000` → **stage-2 translation** fault, unbacked, inside the reservation →
   `commit_reserved_page` backs one zeroed page → `Stop::Other`, never surfaced, no trace record.
2. Repeat, page by page, down to `0x2008000`.
3. SP reaches `0x2004000` — libstd's guard, which is **backed and `PROT_NONE`** → **stage-1
   permission** fault via the EL1 trampoline → `Stop::Fault` → M12's disposition check → `SIGSEGV` →
   libstd's handler compares `si_addr` against its own guard range → `has overflowed its stack` →
   `abort` → 134.

The hardware separates the two cases and the box already relies on that separation: it is M13's
central invariant, stated at `retrace-box/src/lib.rs:1335-1358`, that a protected page is *backed*
so it faults at stage 1, while a reserved-but-uncommitted page is *unbacked* so it faults at stage 2
"where `commit_reserved_page` would silently materialize it instead of faulting."

**This is exactly why the reservation must not cover the guard page.** If it did, the guard would be
a reserved page inside a tracked reservation, and step 3 would take the stage-2 route and be
*silently committed* — converting the overflow into a corrupted, silently-continuing guest. M13 and
`protnone.rs:97-153` exist to defend that boundary; M21 must respect it, not test it.

Leaving the guard page in free space also means M21 does **not** touch `protect_none`, whose
assertion that every page in its range is backed is the invariant that keeps the split unambiguous.

### Why this does not pay the rejected 1.7x

**R3-d**'s cost scales with *mapped* memory. A reservation maps nothing — it is a `Vec` entry. A
guest that never recurses deeply commits **zero** pages and pays **zero**. Only the recursion that
actually needs 7.72 MiB causes 7.72 MiB to be mapped, and it does so page-by-page as it goes.

This claim is the load-bearing one in the milestone, so it is not asserted on reasoning alone — it
is **T0-3**, and the exit criterion requires it measured.

### Determinism

Below the trace, by symmetry rule 2. The reservation is created in `load_dynamic`, which runs
identically on record and replay — **corrected in task 2.5: it does not, because replay never calls
`load_dynamic` at all. The reservation is re-established in `Box_::restore` instead, by calling the
same method with the same arguments.** `commit_reserved_page` is already documented as "deterministic and
trace-free: record and replay re-execute the guest's own stores, fault at the same IPAs in the same
order, and commit identical all-zero pages." No `Event` variant, no `TRACE_MAGIC` bump, no new mirror
arm, and therefore **no new `verify_thread` hole** — the seven-call-site count in `CLAUDE.md` is
unchanged by M21, and that is a property to assert in review, not to hope for.

### Why growth does not confuse `stack_geometry_from_memory`

Worth stating because a reader will rightly worry about it. `restore` recovers the guest's stack
geometry from the snapshot's region list and **fails loud rather than guessing**
(`retrace-box/src/lib.rs:206`); `deliver.rs:5` notes it panics unless the regions look like a stack.
Committed growth pages become new backings, so the question is whether they can change its answer.

They cannot, and the reason is narrow enough to be worth pinning: `covers` matches on
`r.ipa == base` — an **exact base**, not containment. The dynamic arm looks for a region starting at
exactly `DYN_STACK_TOP - DYN_STACK_SIZE` (`0x27C0000`). Every growth page starts *below* that, at its
own granule base, and `commit_reserved_page` pushes each as a separate `Backing` with no merging. So
the original stack backing remains the only region matching the probe, and the `(true, false)` arm
still fires.

This is a property M21 depends on but did not create, so it gets a test rather than a paragraph: a
box-level assertion that `stack_geometry_from_memory` still returns `(DYN_STACK_TOP, DYN_STACK_SIZE)`
after growth pages have been committed below the stack.

### Scope

**Main thread only.** A guest thread's stack comes from a real sized `mmap` and has no
believed-versus-backed mismatch; R3 is specifically about libpthread's hardcoded `0x7fc000` for the
main thread. `threadrust`, `thread_watch`, `sigthread` and the workqueue guests are untouched, and
M21 adds nothing to `thread.rs`.

**Static path untouched.** `load_static` keeps `STACK_TOP_IPA` / one-granule stack; M8's geometry
comment already scopes itself to the dynamic path.

## The residual wall, stated up front

libstd's guard is a **single 16 KiB page**. A guest frame larger than one granule can decrement SP
straight past it into the free IPA below `0x2004000`, which lies outside every reservation, so
`commit_reserved_page` correctly refuses and the fault stays fatal.

M21 **does not close this**, by decision. Closing it would mean inventing a fault classification
with no libstd counterpart, and doing so by loosening `commit_reserved_page`'s strict "never
materialize untracked memory" gate — the gate `reservecommit.rs` and `carveout.rs` exist to defend.
The trade is bad: it would weaken a tested invariant to serve a case the honest-gate discipline can
simply name.

So it gets named, in all three places honest-gate discipline requires: the test comment, the README's
Known limits, and the M21 section of `docs/status-log.md`.

The gate guest provably does not hit it — `rs/overflow.rs`'s frame is `[u64; 64]` (512 bytes) plus
overhead, far under the 16 KiB granule, so it descends page-by-page (**T0-4** confirms this rather
than assuming it). The wall M21 leaves behind is therefore *far* narrower than the one it clears:
from **any recursion past 256 KiB** to **only a frame exceeding 16 KiB**.

## Measurements owed (t0)

The plan's first task takes these before any production change. Each names what it would foreclose.

- **T0-1 — Does anything land in the window today?** `range_is_free` treats reservations as occupied
  (M2-carveout), so reserving `[0x2008000, 0x27C0000)` will make ANYWHERE placement *avoid* a range
  it may currently use. Expected: nothing lands there, since `MMAP_BASE` is 40 GiB and placement is
  hint-forward first-fit. If something does, every recording's addresses shift and the approach needs
  rework before, not after, the suite reddens.
- **T0-2 — Do checkpoints care?** `BoxState.reservations` is carried across checkpoints
  (`retrace-box/src/lib.rs:686`). One extra entry is trivial in cost; the question is whether any
  seek/checkpoint test pins a reservation **count** or index.
- **T0-3 — The cost claim (the load-bearing one).** Wall-clock `hello_rust` and the dyld suite before
  and after. The prediction is *no measurable change* for guests that do not recurse. A regression
  here invalidates the approach and re-opens **R3-d**.
- **T0-4 — Where does the guest actually land?** Instrument the forced-`--ignored` run to record the
  descending fault IPAs, confirming (a) they step by one granule, and (b) the final fault is a
  **stage-1 permission** fault at `0x2004000` and not a stage-2 fault below it.

## Fail-loud boundaries

- `commit_reserved_page` keeps refusing every address outside a tracked reservation. M21 adds a
  reservation; it must not relax the gate. A test asserts a fault *below* the guard page still
  returns `false` and stays fatal.
- The reservation's bounds are asserted at load, not merely computed: `GUARD_TOP` must be strictly
  greater than the guard page and strictly less than `DYN_STACK_BOTTOM`, and `GUARD_TOP` must be
  above `PT_L3_CEIL` (`0x2000000`) so the window can never collide with the L3 translation tables.
- If a future change moves `DYN_STACK_TOP` or `DYN_STACK_SIZE` such that the window inverts or
  swallows the guard page, the assertion fires at load rather than producing a silently-committed
  guard page — the one failure mode this design must never reach.

## Exit criterion

1. `stackoverflow_rust_e2e` **un-`#[ignore]`d and green**: `has overflowed its stack` on stderr,
   exit 134, and two replays byte-identical to the recording.
2. `protnone_rust_e2e` still green — the guard page still installs at `0x2004000` and still enforces.
3. **T0-3** shows no cost regression on `hello_rust` or the dyld suite.
4. `just gate` clean (chunked per `CLAUDE.md`, including the `--bins` chunk), reconciled `#[test]`
   count file-by-file against M20's 478/476/2.
5. The residual large-frame wall documented in the test comment, README Known limits, and the new
   `docs/status-log.md` section.

## Risk register

| # | Risk | Mitigation |
|---|---|---|
| 1 | ANYWHERE placement currently uses the window; reserving it shifts every address | **T0-1** before any production change |
| 2 | Reservation swallows the guard page ⇒ overflow silently committed, guest corrupted | Window starts one granule above; load-time assertion; explicit test that the guard page is **not** in a reservation |
| 3 | Cost regression reproduces R3-d's 1.7x | **T0-3**; approach is abandoned if it fires |
| 4 | A frame >16 KiB vaults the guard | Out of scope by decision; documented in three places |
| 5 | Exit snapshot grows up to 7.72 MiB | Overflow guest only; accepted and stated |
| 6 | Checkpoint tests pin reservation counts | **T0-2** |
| 7 | Growth pages confuse `stack_geometry_from_memory`, breaking `restore` | Disarmed by its exact-base match, not by luck — pinned by a test that commits growth pages and re-probes |

## Components

- `crates/retrace-box/src/lib.rs` — `GUARD_TOP` / `DYN_STACK_BOTTOM` constants with their derivation,
  one `guest_vm_reserve` call in `load_dynamic`, the load-time assertion. **No change to
  `commit_reserved_page`, `protect_none`, or the fault dispatch.**
- `crates/retrace-box/src/lib.rs`, `mod stack_geometry_tests` (line 4589, in-lib, not a `tests/`
  file) — extend for the new window arithmetic, and add the post-growth
  `stack_geometry_from_memory` probe.
- `crates/retrace-box/tests/` — reservation bounds; commit inside; refusal below the guard; the guard
  page is not inside any reservation. **Mirror the structure of
  `workq_reqthreads_leaves_an_uncommittable_guard_page_below_each_worker_stack`
  (`threads.rs:1241`)** — refusal *and* a non-vacuous positive control, so the refusals mean
  something rather than passing because every address refuses.
- `crates/retrace/tests/stackoverflow_rust_e2e.rs` — un-`#[ignore]`, rewrite the comment to describe
  the mechanism and the residual wall.
- `README.md` — "What works today" and "Known limits".
- `docs/status-log.md` — new appended section.

## Open questions for implementation planning

1. Should `GUARD_TOP` be **derived** from the libpthread constant (`DYN_STACK_TOP - 0x7fc000 +
   GRANULE`) rather than written as a literal `0x2008000`? Derivation documents the dependency and
   keeps the two in step if `DYN_STACK_TOP` moves; a literal is easier to read. Recommend derived,
   with the literal asserted alongside it so the arithmetic is pinned both ways.
2. Does the reservation belong inline in `load_dynamic`, or in a small `reserve_believed_stack`
   helper beside it — the shape `place_worker_stack` uses for the worker equivalent? Recommend the
   helper, so the geometry, its assertion, and its derivation sit together and the two stacks read
   as the same pattern.
3. ~~Is there an existing box-level test file for stack geometry to extend?~~ **Answered:**
   `mod stack_geometry_tests` is in-lib at `retrace-box/src/lib.rs:4589`. Extend it; do not add a
   file.
4. `place_worker_stack` skips its granule by bumping `mmap_next`; M21's window is FIXED, so it
   derives `GUARD_TOP` instead. Should the two grow a shared helper? Recommend **no** for M21 —
   they differ in placement mode and the shared part is one addition. Note it, do not build it.
