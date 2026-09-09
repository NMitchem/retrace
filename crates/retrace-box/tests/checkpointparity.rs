// M31-checkpointparity. `from_checkpoint` is the replay-side construction path that restores the
// most state and runs mid-run, where nothing sits at a default. Each field it has ever dropped got
// a point test written after its own bug (pacposture.rs, sigcheckpoint.rs, protnone.rs, tlbi.rs,
// threads.rs); what none of them provide is a forcing function for the NEXT field. This file is
// that: one structural diff, plus an obligation.
//
// THE RESULT, recorded so a guard that found nothing does not read as a guard that did not look:
// on its first run this diff found NO asymmetry. `from_checkpoint` reproduced every field both
// tiers reach — a two-thread table, fd slots (Open and Closed), the signal table, all three
// pthread/workqueue scalars, a PROT_NONE extent, the cache pager, a bootstrap port, an armed
// breakpoint, an armed watchpoint, and `tpidrro_el0`. That is a real finding about the code's
// current state, not an absence of effort, and it is bounded exactly by what the fixtures reach —
// the bound is stated under "What this deliberately does NOT do" below, and the guard was proven
// able to fail by the two M31 t4 mutations recorded there.
use retrace_box::Box_;
use retrace_guest::{parse_macho, HELLO};

/// The four debugger fields are not the only `Box_` state with no accessor — `window_cap` and
/// `l2_host` also have none — but they are the ones this guard needs to observe being reset, and
/// the guard cannot honestly assert a reset unless a test can observe it.
#[test]
fn the_debug_state_accessor_reports_armed_watchpoints() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    let clean = b.dbg_debug_state();
    assert!(clean.contains("bps_armed=false"), "precondition: nothing armed yet, got {clean}");
    assert!(clean.contains("wps_armed=false"), "precondition: nothing armed yet, got {clean}");

    b.arm_hw_watchpoint(0, b.stack_top() - 0x100, 8);
    let armed = b.dbg_debug_state();
    assert!(armed.contains("wps_armed=true"), "arming must be observable, got {armed}");
    assert!(armed.contains("watch_ranges=[("), "the ranges must be observable, got {armed}");
}

/// The entries that differ between two SORTED `(ipa, len)` backing lists, tagged with the side they
/// appear on and rendered in hex — empty when the two maps are identical. A two-pointer merge, so it
/// is multiplicity-exact: `[a, a, b]` against `[a, b, b]` has an empty set difference and is still
/// not the same map. (Same shape as `restoreparity.rs`'s helper, kept local because Rust integration
/// tests are separate binaries and sharing would mean a `util` module both must import.)
fn diff_backings(live: &[(u64, usize)], restored: &[(u64, usize)]) -> String {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    let show = |side, e: &(u64, usize)| format!("{side}-only (ipa {:#x}, len {:#x})", e.0, e.1);
    while i < live.len() || j < restored.len() {
        match (live.get(i), restored.get(j)) {
            (Some(l), Some(r)) if l == r => { i += 1; j += 1; }
            (Some(l), Some(r)) if l < r => { out.push(show("live", l)); i += 1; }
            (Some(_), Some(r)) => { out.push(show("restored", r)); j += 1; }
            (Some(l), None) => { out.push(show("live", l)); i += 1; }
            (None, Some(r)) => { out.push(show("restored", r)); j += 1; }
            (None, None) => unreachable!("loop condition guarantees one side still has entries"),
        }
    }
    out.join(", ")
}

/// The ONE field group that legitimately differs across `from_checkpoint` — asserted, not excused.
///
/// The four debugger fields (`bps_armed`, `wps_armed`, `watch_ranges`, `syscall_watch_hit`) are not
/// carried in `BoxState` and come back at their defaults. That is correct: the DEBUGGER owns the
/// watch list, not the box, and re-arms from its own stored copy after every seek — see
/// `crates/retrace/src/debug.rs:608`, `:641` and `:761`, each calling
/// `ReplaySession::arm_watchpoints(&ws)` where `ws` is the debugger's list. A box that restored them
/// would be a second authority for the same state.
///
/// This is deliberately a POSITIVE assertion rather than a `normalise()` that strips the field out
/// of the comparison. Stripping excuses a difference invisibly and passes just as well when the
/// field stops being reset; asserting the reset states what is supposed to happen and fails if it
/// stops happening. The rich tier arms a watchpoint before capture precisely so this assertion has
/// something to observe.
///
/// `cache_installed` is deliberately NOT treated this way: `from_checkpoint` re-installs the pager
/// on its last line (`crates/retrace-box/src/lib.rs`, `if state.cache_installed {
/// b.install_cache_pager(); }`), so it must compare EQUAL as ordinary coverage.
fn assert_debug_state_is_deliberately_reset(r: &Box_, label: &str) {
    assert_eq!(r.dbg_debug_state(),
        "bps_armed=false wps_armed=false watch_ranges=[] syscall_watch_hit=None",
        "{label}: the debugger four must come back at their defaults — the debugger re-arms from \
         its own list (crates/retrace/src/debug.rs:608, :641, :761). If this now differs, either \
         from_checkpoint started carrying them (delete this assertion and compare them instead) or \
         a new field joined the group (add it here).");
}

/// The structural guard.
///
/// **OBLIGATION when you add a field to `Box_`, or any `from_checkpoint`-time write:** it must be
/// either (a) compared here and EQUAL, (b) asserted above as deliberately reset, with the mechanism
/// that re-establishes it on the replay side cited by file and line, or (c) named HERE as knowingly
/// excluded, citing the comment that documents the exclusion. There is no fourth option that is
/// safe. How often this path has actually dropped a field is counted in ONE place — the
/// `M24-restoreaudit` section of `docs/status-log.md` — and is deliberately not recounted here; the
/// `BoxState` field list is a different thing and not that count (see the note on `BoxState`). The
/// clause that used to end this sentence, "each caught only after it shipped", was too strong and
/// is withdrawn (M31 t5): every instance that list attributes to this path was closed inside its own
/// milestone — M9 t3 by a code-review follow-up, M10 t4 and M11 t6 by a later task, M14 by `t7 fix
/// round 1` — and M18's `wq_thread_pc` was carried in `e93f8dc`, the same commit that introduced it,
/// so it never had a gap at all.
/// `window_cap` (`crates/retrace-box/src/lib.rs:5259`) and `canary_disturbances` (`:5264`) are
/// bucket (c): both are test-only instrumentation nothing in production reads (M28 and M30
/// respectively), documented at those two lines as deliberately NOT carried in `BoxState`, so a
/// restored box always gets the production default / a fresh zero rather than the live value —
/// correct, not lossy, and not worth asserting on since "always the default" is not a fact about
/// `from_checkpoint` doing anything.
///
/// **What this deliberately does NOT do**, so it is not mistaken for more than it is:
/// it compares CONSTRUCTION at one landmark, not evolution afterwards
/// (`crates/retrace/tests/checkpoint_seek.rs` is that axis); two boxes wrong in the SAME way are
/// invisible to any test that only diffs them against each other; and the memory comparison below
/// is over the MAP (which `(ipa, len)` regions exist), never over CONTENTS — `from_checkpoint`
/// populates every backing's bytes with a `memcpy` straight from `state.mem`, so a byte-for-byte
/// compare here would be near-tautological. (`restoreparity.rs`'s L1 case, by contrast, does
/// byte-compare the EL1 vector table — a reader should not assume this file does the same.)
///
/// **Proven able to fire (M31 t4).** Replacing `sigtable: state.sigtable.clone()` with
/// `SigTable::default()` in `from_checkpoint` turns
/// `a_checkpointed_box_with_rich_state_matches_the_box_it_came_from` RED at `rich: signal
/// dispositions`. Recorded because a guard nobody has watched fail is a guard nobody knows is wired
/// up — M28's `let band = 0;` passed a 523-test gate before its own positive control existed.
///
/// **Second control: the two-thread fixture is load-bearing, not decorative.** Replacing
/// `threads: state.threads.clone()` with a version that zeroes every NON-current thread's `ctx`
/// (keeping thread count and `current` unchanged) turns the rich tier RED at `rich: the restored
/// thread table must reproduce the CAPTURED table exactly` — but leaves the static tier GREEN,
/// because with only one thread there is no non-current entry to corrupt. That asymmetry is the
/// point: on Task 2's single-thread fixture this exact bug would have been invisible, caught only
/// after Task 3 added a second thread.
fn assert_checkpoint_parity(b: Box_, label: &str) {
    let live_internal = b.dbg_internal_state();
    let (top, size) = (b.stack_top(), b.stack_size());
    let (tp, tpro) = (b.tpidr_el0(), b.tpidrro_el0());
    let cur = b.threads().current();
    let nthreads = b.threads().len();
    // The CURRENT thread's table entry is deliberately stale in a live box — only
    // `switch_to_thread` refreshes it — and `Box_::checkpoint` folds the live vCPU into it before
    // carrying it (see the `threads:` arm of `checkpoint()` in crates/retrace-box/src/lib.rs). So a
    // restored table legitimately holds MORE current state than the live box's own table, and
    // comparing the two directly asserts something that is false by design. It failed on this
    // guard's first run for exactly that reason.
    let live_cur_ctx = b.save_ctx();
    // Non-current entries are authoritative in a live box — `switch_to_thread` refreshes them on
    // the way out — so they are compared against the LIVE box, not merely against the captured
    // state. Without this the guard is clone-fidelity only: a `checkpoint()` that folded into the
    // wrong index, or corrupted another thread's ctx, would make restored == captured and pass.
    let live_other_ctxs: Vec<String> = (0..nthreads)
        .filter(|&i| i != cur)
        .map(|i| format!("{:?}", b.threads().ctx_of(i)))
        .collect();
    let fds = b.fds().slots();
    let sigtable = format!("{:?}", b.sigtable());
    let start_pc = b.thread_start_pc();
    let wq_pc = b.wq_thread_pc();
    let psize = b.pthread_size();
    let noaccess = b.noaccess().to_vec();
    let fts = b.fall_throughs();
    let mut live_backings = b.dbg_backings();
    live_backings.sort_unstable();
    let next_l3 = b.dbg_next_l3();
    let tlbi_ready = b.dbg_tlbi_stub_ready();

    let state = b.checkpoint();
    drop(b); // one VM per process (HVF)
    let r = Box_::from_checkpoint(&state);

    assert_eq!(r.dbg_internal_state(), live_internal, "{label}: internal bookkeeping");
    assert_debug_state_is_deliberately_reset(&r, label);
    assert_eq!((r.stack_top(), r.stack_size()), (top, size), "{label}: stack geometry");
    assert_eq!((r.tpidr_el0(), r.tpidrro_el0()), (tp, tpro), "{label}: thread-pointer sysregs");
    assert_eq!(r.threads().len(), nthreads, "{label}: thread count");
    assert_eq!(format!("{:?}", r.threads()), format!("{:?}", state.threads),
        "{label}: the restored thread table must reproduce the CAPTURED table exactly");
    assert_eq!(*r.threads().ctx_of(cur), live_cur_ctx,
        "{label}: the current thread's restored context must equal the live box's save_ctx() — \
         checkpoint() folds the live vCPU into the table before carrying it, so this is the fold \
         itself being asserted, not the stale live table");
    let restored_other_ctxs: Vec<String> = (0..nthreads)
        .filter(|&i| i != cur)
        .map(|i| format!("{:?}", r.threads().ctx_of(i)))
        .collect();
    assert_eq!(restored_other_ctxs, live_other_ctxs,
        "{label}: non-current threads' saved contexts must match the LIVE box — these are not \
         stale, unlike the running thread's entry");
    assert_eq!(r.fds().slots(), fds, "{label}: guest-visible fd slots");
    assert_eq!(format!("{:?}", r.sigtable()), sigtable, "{label}: signal dispositions");
    assert_eq!(r.thread_start_pc(), start_pc, "{label}: bsdthread_register start pc");
    assert_eq!(r.wq_thread_pc(), wq_pc, "{label}: workqueue thread pc");
    assert_eq!(r.pthread_size(), psize, "{label}: pthread struct size");
    assert_eq!(r.noaccess(), noaccess.as_slice(), "{label}: PROT_NONE map");
    assert_eq!(r.fall_throughs(), fts, "{label}: fall-through counter");
    let mut restored_backings = r.dbg_backings();
    restored_backings.sort_unstable();
    assert_eq!(restored_backings.len(), live_backings.len(), "{label}: backing count");
    let diff = diff_backings(&live_backings, &restored_backings);
    assert!(diff.is_empty(), "{label}: live and restored disagree on the memory map: {diff}");
    assert_eq!(r.dbg_next_l3(), next_l3,
        "{label}: next free L3 table IPA — restored derived {:#x}, live had {next_l3:#x}",
        r.dbg_next_l3());
    assert_eq!(r.dbg_tlbi_stub_ready(), tlbi_ready,
        "{label}: TLBI stub readiness — from_checkpoint re-derives this from the restored backings, \
         exactly as it re-derives next_l3 above, so this is a second derivation of one fact");
}

/// The static tier. Deliberately mid-run, not landmark 0: at landmark 0 a defaulted field and a
/// correctly-restored one are indistinguishable, which is the whole reason `checkpoint.rs` runs
/// mid-run too. Its reach is still narrow, though: running `HELLO` to its first syscall moves only
/// pc/elr/spsr off their landmark-0 values — `dbg_internal_state`, the signal table, the fd table,
/// `thread_start_pc`, `wq_thread_pc`, `pthread_size` and `noaccess` are all still default-vs-default
/// here, so this tier cannot catch a bug in restoring any of them. Task 3's richer fixture is where
/// that reach arrives.
#[test]
fn a_checkpointed_static_box_matches_the_box_it_came_from() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    let _ = b.run(); // reach the first syscall, so this is genuinely mid-run
    assert_checkpoint_parity(b, "static");
}

/// The rich tier, and the reason this milestone is not just Task 2.
///
/// A field left at its default is compared, and the comparison proves nothing: `Default ==
/// Default` passes for a reason unrelated to the assertion's name. `restoreparity.rs` names that
/// trap explicitly. So this stages a NON-DEFAULT value into every field the structural diff reaches
/// that the static tier leaves empty, and asserts each one is non-default BEFORE capturing — the
/// preconditions are what separate a guard that agrees from a guard that cannot see.
///
/// State is staged through `Box_`'s own public methods rather than by running a real threaded guest:
/// a guest that spawns threads needs dyld and libpthread, and `retrace-box` cannot depend on
/// `retrace-core`. `tests/threads.rs` established this pattern (it builds thread contexts by hand on
/// a static box for exactly the same reason).
///
/// The three pthread/workqueue scalars are staged via `guest_bsdthread_register`, not
/// `set_thread_start_pc`: the setter only reaches `thread_start_pc`, leaving `wq_thread_pc` and
/// `pthread_size` at `None` on both the live and restored box — `Default == Default`, the exact
/// trap this fixture exists to eliminate. `guest_bsdthread_register`'s whole body
/// (`crates/retrace-box/src/lib.rs:4119`) sets all three from one call and has no other effect:
/// `self.thread_start_pc = Some(args[0]); self.wq_thread_pc = Some(args[1]); self.pthread_size =
/// Some(args[2] as u32); WORKQ_FEATURE_WORD as u64`.
///
/// **Honest reach limit.** Even with everything below staged, seven fields the structural diff
/// reaches are STILL `Default == Default` here, so this fixture proves nothing about
/// `from_checkpoint` restoring them:
///   - `synthetic_tsc` — advances only when the guest issues the timebase MRS `run()` emulates;
///     HELLO never executes that instruction. `tests/checkpoint.rs` reaches it, but only by
///     `Box_::step`-ping a DIFFERENT guest (STEPPY) that does — there is no public "bump the
///     timebase" setter, only a guest whose code contains the instruction.
///   - `last_far` — written only when `run()` takes a genuine stage-2 fault; there is no public
///     "stage a fault" method, only a guest that actually faults (same class of limit as above).
///   - `cache_refault_ipa` / `cache_refault_count` — advance only on a real demand-page refault
///     against the dyld shared cache; HELLO's static asm path never walks the cache at all.
///   - `pac_enabled` — deliberately NOT staged: M7 established PAC is a per-process macOS posture
///     (arm64e guests only), so forcing it on via `load_with_pac(.., true)` over a non-arm64e guest
///     would assert a posture the guest's own binary never claims.
///   - `fall_throughs` — increments only on one specific dispatch fallback path, not reachable from
///     a static box through any staging call.
///   - `tpidr_el0` — no public setter exists (contrast `tpidrro_el0`, staged below via
///     `set_tpidrro_el0`).
///   - `syscall_watch_hit` — set only when a real watched write occurs during syscall-diff
///     application, not reachable without forwarding an actual syscall.
///
/// `stack_top` / `stack_size` are a DIFFERENT class from the seven above, and do not belong in that
/// list: they are not absent defaults but always-identical non-trivial constants (`STACK_TOP_IPA` /
/// `GRANULE`). Nothing in `Box_`'s public interface moves them post-load, so this fixture cannot
/// force them away from the landmark-0 value, and the comparison cannot tell a genuine carry-through
/// of `state.stack_top`/`state.stack_size` (`crates/retrace-box/src/lib.rs:5232-5233`) from a
/// hardcoded recomputation of the same constant.
#[test]
fn a_checkpointed_box_with_rich_state_matches_the_box_it_came_from() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    let _ = b.run(); // mid-run

    // fds: one open, one closed — Closed must stay distinguishable from Free across the restore.
    let open_fd = b.fds_mut().alloc();
    let closed_fd = b.fds_mut().alloc();
    b.fds_mut().close(closed_fd);

    // sigtable: a non-default disposition.
    b.sigtable_mut().set_action(6, retrace_box::SigAction {
        disp: retrace_box::Disposition::Ign, tramp: 0, mask: 0xf, flags: 0x2 });

    // per-thread signal state (carried wholesale inside `threads`).
    b.threads_mut().set_mask_of(0, retrace_arch::SIG_SETMASK, 0b1010);

    // A SECOND thread, so the non-current-context comparison inside `assert_checkpoint_parity` is
    // not empty-vs-empty. With only one thread `cur == 0` and `live_other_ctxs`/
    // `restored_other_ctxs` are both `[]` — the assertion that exists specifically to stop this
    // guard being clone-fidelity-only would be comparing nothing to nothing. The context is
    // derived from the live vCPU (same pattern as `tests/deliver.rs:307-312`), on a different
    // stack offset still inside the live stack backing, so no new backing is needed.
    let mut child = b.save_ctx();
    let other_sp = child.regs.sp_el0 - 0x2000;
    child.regs.sp_el0 = other_sp;
    let tid = b.threads_mut().spawn(child, (other_sp, 0));
    // A DISTINCT mask on tid 1: two near-identical contexts would let an index-swap bug in
    // `checkpoint()`'s fold pass unnoticed, so differentiating the entries is part of the point.
    b.threads_mut().set_mask_of(tid, retrace_arch::SIG_SETMASK, 0b0101);

    // the three pthread/workqueue scalars, all from one call — see the doc comment above. The
    // pthread_size value is deliberately distinctive (not 0x4000, which the PROT_NONE extent and
    // the mmap below also use) — a bug that fed either of those lengths into this slot by mistake
    // must not coincidentally satisfy the precondition.
    b.guest_bsdthread_register([0x1234_5000, 0x5678_9000, 0x2A10, 0, 0, 0, 0, 0]);

    // noaccess: a real PROT_NONE extent over backed memory (protect_none asserts on unbacked).
    let base = b.guest_vm_reserve(0, 0x10000, true);
    assert!(b.commit_reserved_page(base), "precondition: the reservation must commit");
    b.protect_none(base, 0x4000);

    // an ordinary anon mmap, to move mmap_next off its initial value.
    let _mapped = b.guest_mmap(0, 0x4000, 3, 0x1002);

    // cache_installed and bootstrap_port: both bundled inside `dbg_internal_state`'s one string
    // (no individual accessor exists), staged exactly as `tests/checkpoint.rs` does on a static box.
    b.install_cache_pager();
    let _port = b.mint_bootstrap_port();

    // A watchpoint, ARMED — so `assert_debug_state_is_deliberately_reset` is observing a real reset
    // rather than a field that was already at its default. Without this the reset assertion would
    // pass on a box that never armed anything, which proves nothing about from_checkpoint.
    b.arm_hw_watchpoint(0, base, 8);

    // A hardware BREAKPOINT too — `bps_armed` is the other half of the debugger-four reset check,
    // and until now nothing armed it, so that half of the reset assertion was as vacuous as the
    // fields above.
    b.arm_hw_breakpoint(0, b.stack_top() - 0x200);

    // tpidrro_el0: the box-level sysreg (distinct from each thread's saved `ThreadCtx.tpidrro_el0`),
    // read back by `assert_checkpoint_parity` via `Box_::tpidrro_el0()`.
    b.set_tpidrro_el0(0xDEAD_0000);

    // PRECONDITIONS. Without these the comparison below is Default == Default.
    assert_eq!(b.fds().slots()[open_fd as usize], retrace_box::FdSlot::Open,
        "precondition: an OPEN guest fd");
    assert_eq!(b.fds().slots()[closed_fd as usize], retrace_box::FdSlot::Closed,
        "precondition: a CLOSED guest fd, distinct from Free");
    assert_ne!(format!("{:?}", b.sigtable()), format!("{:?}", retrace_box::SigTable::default()),
        "precondition: a non-default disposition table");
    assert_eq!(b.threads().mask_of(0), 0b1010, "precondition: a non-default blocked mask");
    assert_eq!(tid, 1, "precondition: the second thread must land at index 1");
    assert_eq!(b.threads().len(), 2, "precondition: a second thread must actually exist");
    assert_eq!(b.threads().mask_of(tid), 0b0101, "precondition: tid 1's own non-default mask");
    assert_ne!(b.threads().mask_of(0), b.threads().mask_of(tid),
        "precondition: tid 0 and tid 1 must carry DIFFERENT masks, or an index-swap bug is invisible");
    assert_eq!(b.thread_start_pc(), Some(0x1234_5000), "precondition: bsdthread_register seen");
    assert_eq!(b.wq_thread_pc(), Some(0x5678_9000), "precondition: bsdthread_register seen");
    assert_eq!(b.pthread_size(), Some(0x2A10), "precondition: bsdthread_register seen");
    assert_eq!(b.noaccess(), &[(base, 0x4000)], "precondition: a non-empty PROT_NONE map");
    let internal = b.dbg_internal_state();
    assert!(internal.contains("cache_installed=true"),
        "precondition: install_cache_pager must flip cache_installed, got {internal}");
    assert!(internal.contains("bootstrap_port=Some("),
        "precondition: mint_bootstrap_port must set bootstrap_port, got {internal}");
    assert!(!internal.contains(&format!("mmap_next={:#x} ", retrace_box::MMAP_BASE)),
        "precondition: mmap_next must move off MMAP_BASE, got {internal}");
    let debug_state = b.dbg_debug_state();
    assert!(debug_state.contains("wps_armed=true"),
        "precondition: a watchpoint is ARMED, so the reset assertion has something to observe, \
         got {debug_state}");
    assert!(debug_state.contains("bps_armed=true"),
        "precondition: a breakpoint is ARMED too — the other half of the debugger-four reset \
         check, got {debug_state}");
    assert_eq!(b.tpidrro_el0(), 0xDEAD_0000, "precondition: tpidrro_el0 staged");

    assert_checkpoint_parity(b, "rich");
}
