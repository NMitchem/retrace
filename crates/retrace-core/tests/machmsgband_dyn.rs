// M32 Task 1 fix round 1, Important 3 + "(b)" of the reviewer's two-halves requirement: a REAL
// (len, send_size, rcv_size) triple captured on a call `SEED_MACH_MSG2` actually governs.
//
// `crates/retrace-box/tests/machmsgband.rs`'s structural proof establishes the invariant for every
// POSSIBLE forwarded call; this test supplies the thing a pure proof cannot -- a genuine sample
// from a real dynamically-linked guest, on a call that is actually forwarded (`Route::Forward`),
// which is the only route `SEED_MACH_MSG2` can ever change anything for
// (`crates/retrace-core/src/machmsg.rs:102`, `FORWARD_ALLOWLIST`).
//
// msgh_id 3405 (task_info/TASK_AUDIT_TOKEN) is the id to use: every dynamically linked guest sends
// it during libsecinit's app-sandbox check (see `crates/retrace/tests/hello_dyn_e2e.rs`'s M2-taskinfo
// history comment, which measured its send as 40 bytes against a live run), and
// `crates/retrace-core/src/machmsg.rs`'s `FORWARD_ALLOWLIST` genuinely routes it through
// `forward_and_diff` on record.
//
// This lives in `retrace-core` (not `retrace-box`) because reaching a `Route::Forward` call at all
// needs `retrace-core`'s own mach_msg2 routing -- exactly the routing a bare `Box_` harness in
// `retrace-box`'s own tests does not have (see that crate's `machmsgband.rs` file comment).
use retrace_core::machmsg::Msg2;

const MACH_MSG2: u64 = (-47i64) as u64;
const TASK_INFO_MSGH_ID: u32 = 3405;

#[test]
fn the_real_forwarded_task_info_calls_band_lands_at_or_past_send_size() {
    let exe = retrace_guest::parse_macho(&std::fs::read(retrace_guest::HELLO_DYN).unwrap());
    let dyld_path = exe.dylinker.clone().unwrap_or_else(|| retrace_guest::DYLD_PATH.to_string());
    let dyld_bytes = std::fs::read(&dyld_path).unwrap_or_else(|e| panic!("read dyld {dyld_path}: {e}"));
    let dyld = retrace_guest::parse_macho(retrace_guest::slice_arm64e(&dyld_bytes));
    let argv = vec![retrace_guest::HELLO_DYN.to_string()];
    let trace = std::env::temp_dir()
        .join(format!("retrace-m32t1-machmsgband-dyn-{}.bin", std::process::id()));
    retrace_core::record_dynamic(&exe, &dyld, &argv, &trace).expect("record hello_dyn");

    // Find the landmark index of the REAL 3405 send. `Reader::open` yields the same `events`
    // vector `ReplaySession` walks internally, so this index is a valid `seek` coordinate.
    let events = retrace_trace::Reader::open(&trace).unwrap();
    let n = events.iter().position(|e| matches!(e,
            retrace_trace::Event::Syscall { num, args, .. }
                if *num == MACH_MSG2 && Msg2::unpack(args).msgh_id == TASK_INFO_MSGH_ID))
        .unwrap_or_else(|| panic!(
            "hello_dyn's recorded trace never sent msgh_id {TASK_INFO_MSGH_ID} (task_info) -- \
             the real-call fixture this measurement depends on did not fire; see \
             hello_dyn_e2e.rs's M2-taskinfo history comment for the call this was expected to be"));

    // Seek to (n, 0): the session is positioned with landmark n as the NEXT event to consume,
    // memory state exactly as it was when the real record run reached this same trap (replay is
    // byte-identical by construction) -- every backing that will exist at the live trap already
    // exists, because every syscall that could have created one has already been consumed.
    let session = retrace_core::seek(&trace, n, 0).expect("seek to the 3405 landmark");
    let (num, args) = session.peek_syscall().expect("landmark n must be a Syscall event");
    assert_eq!(num, MACH_MSG2);
    let m = Msg2::unpack(&args);
    assert_eq!(m.msgh_id, TASK_INFO_MSGH_ID);

    let len = session.dbg_window_len_for(args[0])
        .expect("task_info's message buffer must be a mapped guest IPA");
    // Printed so the triple lands in the fix-round report verbatim.
    eprintln!("[M32 t1 fix] REAL msgh_id={} buf={:#x} len={len} send_size={} rcv_size={} \
               (Route::Forward, genuinely governed by SEED_MACH_MSG2)",
        m.msgh_id, args[0], m.send_size, m.rcv_size);

    assert!(len >= m.send_size as usize,
        "REAL forwarded call: band would land INSIDE the kernel-read region: len {len} < \
         send_size {} for buffer {:#x} -- the hypothesis is REFUTED on a call this decision \
         actually governs, and mach_msg2 must stay withheld (spec §7, last bullet)",
        m.send_size, args[0]);

    // Cross-checks the `restore` production constructor's window_cap against a REAL instance --
    // `ReplaySession::open` (which `seek` calls) builds `self.b` via `Box_::restore`, the one
    // production constructor `machmsgband.rs`'s structural proof does not reach directly.
    assert_eq!(session.dbg_window_cap(), retrace_box::PTR_WINDOW_CAP,
        "Box_::restore (the constructor every ReplaySession uses) must ALSO set window_cap == \
         PTR_WINDOW_CAP");

    // And the fourth production constructor (`BoxState`'s restore path, used by
    // `ReplaySession::from_checkpoint`), reachable for free from the session already in hand.
    // `checkpoint()` returns an owned, self-contained value, so it can be captured before
    // dropping `session` -- and it MUST be dropped first: one VM per process (`seek`'s own doc
    // comment says so explicitly), and `from_checkpoint` below builds a second live `Box_`.
    let checkpoint = session.checkpoint();
    drop(session);
    let restored = retrace_core::ReplaySession::from_checkpoint(&trace, &checkpoint)
        .expect("from_checkpoint on a freshly taken checkpoint must succeed");
    assert_eq!(restored.dbg_window_cap(), retrace_box::PTR_WINDOW_CAP,
        "Box_::from_checkpoint's BoxState restore path (the fourth production constructor) must \
         ALSO set window_cap == PTR_WINDOW_CAP -- all four production constructors now checked \
         against a live instance, none only cited");
}
