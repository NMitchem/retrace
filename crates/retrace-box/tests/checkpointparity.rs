// M31-checkpointparity. `from_checkpoint` is the replay-side construction path that restores the
// most state and runs mid-run, where nothing sits at a default. Each field it has ever dropped got
// a point test written after its own bug (pacposture.rs, sigcheckpoint.rs, protnone.rs, tlbi.rs,
// threads.rs); what none of them provide is a forcing function for the NEXT field. This file is
// that: one structural diff, plus an obligation.
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
/// safe. This path has dropped a field at least five times, each caught only after it shipped.
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
