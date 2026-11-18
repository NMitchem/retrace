// Plain longjmp and siglongjmp. libplatform ends both with sigreturn(NULL, UC_SET_ALT_STACK or
// UC_RESET_ALT_STACK, 0), which xnu answers with 0 after flipping the thread's on-alternate-stack flag.
// retrace used to treat every syscall 184 as a handler returning and ended the recording at the
// "sigreturn token mismatch" wall. Each mode runs native, recorded and replayed twice with native's stdout,
// and the recording must hold the 184 landmark itself (exit status alone would not show it was reached).
// Limit, not guarded: a hand-made SET with the stack pointer off the alternate stack makes native's sigaltstack
// report SS_ONSTACK; retrace derives that flag from the stack pointer and does not.
mod util;
use retrace_trace::{Event, Reader};

fn rung(mode: &str) -> util::RungOut {
    let g = retrace_guest::LONGJMP_DYN;
    let native = util::run_bounded(g, &[mode], &[], 30);
    assert_eq!(native.code, Some(0), "native {mode}: {}", native.stderr);
    let again = util::run_bounded(g, &[mode], &[], 30);
    assert_eq!(native.stdout, again.stdout, "native {mode} must be byte-stable");
    let r = util::assert_rung_records_and_replays(g, &[mode], &native.stdout);
    let ev = Reader::open(&r.trace).unwrap();
    let flips = ev.iter().filter(|e| matches!(e, Event::Syscall { num, args, ret, err, .. }
        if *num == retrace_arch::SYS_SIGRETURN && args[0] == 0
            && matches!(args[1] as u32, retrace_arch::UC_SET_ALT_STACK | retrace_arch::UC_RESET_ALT_STACK)
            && *ret == 0 && !*err)).count();
    assert!(flips >= 1, "{mode}: no sigreturn(NULL, UC_*_ALT_STACK) landmark in the recording");
    r
}

#[test] fn longjmp_matches_native() { rung("longjmp"); }
#[test] fn siglongjmp_matches_native() { rung("siglongjmp"); }
#[test] fn a_thousand_round_trips_match_native() {
    let r = rung("loop");
    assert_eq!(String::from_utf8(r.stdout).unwrap(), "count 1000\n");
}
#[test] fn a_jump_out_of_an_altstack_handler_matches_native() {
    let r = rung("handler");
    let out = String::from_utf8(r.stdout).unwrap();
    assert!(out.contains("handler ran on the alternate stack"), "{out}");
}

#[test]
fn a_wrong_thread_at_the_altstack_flag_landmark_is_a_divergence() {
    let (rec, trace) = util::record_dynamic_args(retrace_guest::LONGJMP_DYN, &["longjmp"]);
    assert_eq!(rec.code, 0, "{}", rec.stderr);
    let mut events = Reader::open(&trace).unwrap();
    let i = events.iter().position(|e| matches!(e, Event::Syscall { num, args, .. }
        if *num == retrace_arch::SYS_SIGRETURN && args[0] == 0)).expect("a 184 flag landmark");
    if let Event::Syscall { thread, .. } = &mut events[i] { *thread += 1; }
    let mut w = retrace_trace::Writer::create(&trace).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);
    let rep = util::replay(&trace);
    assert_eq!(rep.code, 3, "{}", rep.stderr);
    assert!(rep.stderr.contains("the schedule diverged"), "{}", rep.stderr);
}
