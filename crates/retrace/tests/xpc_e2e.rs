// The headline M23 gate: an Apple system binary that opens an XPC connection records and replays
// bit-for-bit, because the box answers the connection attempt truthfully instead of choking on it.
//
// `/bin/date` is the fixture on purpose. It is the smallest of the 17 binaries that collapsed onto
// this wall, it takes zero vector fall-throughs (M23 t2 sweep), and its output is a *recorded*
// timestamp — so a replay that reproduced it by asking the clock again would print a different one.
mod util;
use retrace_trace::Event;

/// The refusal `Route::RefuseMqSend` returns. Duplicated here rather than imported so a change to
/// the production constant has to be made deliberately in two places — this value is a measurement
/// (M23 S6: of seven codes tried, `MACH_SEND_INVALID_RIGHT` is the only one that breaks this very
/// guest), not an implementation detail free to drift.
const MACH_SEND_INVALID_DEST: u64 = 0x1000_0003;
/// `mach_msg2_trap`. Not public from `retrace-core`, and this is the only thing the test needs.
const MACH_MSG2: u64 = (-47i64) as u64;

#[test]
fn a_guest_that_opens_an_xpc_connection_records_and_replays() {
    let (rec, trace) = util::record_dynamic("/bin/date");
    assert_eq!(rec.code, 0, "record must complete: {}", rec.stderr);

    // (1) The guest reached its OWN code. A guest that died inside dyld also "records", and its
    //     stdout would be empty — so assert on the output, not the exit code.
    let out = String::from_utf8_lossy(&rec.stdout).into_owned();
    assert!(out.trim().len() > 10 && out.contains(':'),
        "`date` must print a timestamp, i.e. reach main; stdout: {out:?}");

    // (2) The "@XPC" send was SERVICED, and serviced by the route this milestone added — not merely
    //     survived. This is the `protnone_rust_e2e` rule: assert on the difference the work makes.
    //     A guest that never reached XPC would exit 0 and print a date too, so nothing above this
    //     line distinguishes the milestone's work from the state before it.
    let (events, torn) = retrace_trace::Reader::open_checked(&trace).unwrap();
    assert!(!torn, "a recorder killed mid-run leaves a torn trace — this must be complete");
    //     The filter deliberately keys on `(num, ret)` ONLY. Folding the write-set check in here
    //     would make (3) dead code — it did, in the first draft, and the mutation that made the
    //     refusal write a synthetic reply was then caught by this assertion with a misleading
    //     message about the *count*.
    let refusals: Vec<_> = events.iter().filter(|e| matches!(e,
        Event::Syscall { num, ret, .. }
            if *num == MACH_MSG2 && *ret == MACH_SEND_INVALID_DEST)).collect();
    assert_eq!(refusals.len(), 1,
        "exactly one message-queue send must be refused — measured: every one of the 17 binaries \
         sends exactly one before giving up (M23 S6). More would mean the guest is retrying, which \
         is the behaviour the other six refusal codes produced and this one does not.");

    // (3) ...and the refusal wrote NOTHING into the guest's receive buffer. That empty write set is
    //     what makes the reply a constant, which is in turn what lets replay recompute and
    //     byte-compare it rather than trust the recording. Asserted separately from (2) so that an
    //     arm which started writing a synthetic reply fails with *that* as its reason.
    let Event::Syscall { err, writes, .. } = refusals[0] else { unreachable!() };
    assert!(!*err, "a mach trap reports its status in x0, not the carry flag");
    assert!(writes.is_empty(), "the refusal must leave the guest's receive buffer untouched, so that \
         both the return and the write set are constants replay can recompute; got {} write(s)",
         writes.len());

    // (4) Replay is bit-for-bit. The timestamp is the load-bearing part: `date` prints the time the
    //     RECORDING saw, so identical stdout proves the replay re-derived it from the trace rather
    //     than from the clock.
    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "replay must not diverge: {}", rep.stderr);
    assert_eq!(rec.stdout, rep.stdout,
        "replay must reproduce the RECORDED timestamp, not a fresh one:\nrecord: {out:?}\nreplay: {:?}",
        String::from_utf8_lossy(&rep.stdout));
}
