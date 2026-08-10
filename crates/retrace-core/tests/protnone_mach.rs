// M13 t9: `mach_vm_protect` is routed into the box on BOTH sides of the record/replay loop.
//
// This is the gate the box-level test cannot be. `retrace-box/tests/protnone.rs`'s
// `mach_vm_protect_denies_access_too` calls `guest_mprotect` itself, so it passes whether or not the
// dispatch arms are wired — it pins the arg layout only. Here the real `record`/`replay` loop drives
// the arm, so the wiring is the thing under test:
//
//   - Without the RECORD arm's call, `mach_vm_protect` is a no-op success, the guest's second store
//     lands on a still-writable page, and the guest reaches its `exit(7)` — Outcome::Exit, not Crash.
//   - Without the REPLAY arm's call, the page is never protected on the replay run, so the replay
//     guest survives the store the recording died on — which surfaces as a divergence.
//
// So both halves of symmetry rule 1 are covered by one guest.
use retrace_core::{record, replay, Outcome};
use retrace_trace::{Event, Reader};

#[test]
fn mach_vm_protect_is_routed_into_the_box_on_both_sides() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::PROTNONE_MACH).unwrap());
    let trace = std::env::temp_dir()
        .join(format!("retrace-protnone-mach-{}.bin", std::process::id()));

    let rec = record(&loaded, &trace).expect("record must SUCCEED on a guest that faults");
    let Outcome::Crash { esr, far, pc } = rec.outcome else {
        panic!("the protected store must kill the guest, but it recorded {:?}. An Exit(7) here is \
                precisely the pre-M13 behavior: mach_vm_protect answering KERN_SUCCESS without ever \
                calling into the box.", rec.outcome);
    };
    assert_eq!((esr >> 26) & 0x3f, 0x24, "lower-EL data abort EC");
    assert_eq!(esr & 0x3f, 0x0f,
        "DFSC must be 0x0f — a stage-1 PERMISSION fault, level 3. A translation fault (0x07) would \
         mean the page was invalidated rather than AP-denied, which is a different mechanism.");
    assert_ne!(far, 0, "the faulting address is the protected page");
    assert!(pc != 0);

    // Terminal shape: the fault has no handler disposition, so it stays on M6's Crash path.
    let events = Reader::open(&trace).unwrap();
    assert!(matches!(events[events.len() - 2], Event::Crash { .. }));
    assert!(matches!(events[events.len() - 1], Event::Snapshot { .. }));

    // The replay arm is what makes this reproduce; twice, like the other crash gates.
    for _ in 0..2 {
        let rep = replay(&trace).expect("replay of the protected-fault trace succeeds");
        assert_eq!(rep.outcome, rec.outcome,
            "replay must protect the page too, or it survives the store the recording died on");
    }
}
