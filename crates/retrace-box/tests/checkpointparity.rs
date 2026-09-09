// M31-checkpointparity. `from_checkpoint` is the replay-side construction path that restores the
// most state and runs mid-run, where nothing sits at a default. Each field it has ever dropped got
// a point test written after its own bug (pacposture.rs, sigcheckpoint.rs, protnone.rs, tlbi.rs,
// threads.rs); what none of them provide is a forcing function for the NEXT field. This file is
// that: one structural diff, plus an obligation.
use retrace_box::Box_;
use retrace_guest::{parse_macho, HELLO};

/// The four debugger fields are the only `Box_` state with no accessor at all, and the guard cannot
/// honestly assert a field is reset unless a test can observe it being reset.
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
