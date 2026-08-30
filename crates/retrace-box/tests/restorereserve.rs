// M21-stackgrow task 2.5. The believed-stack reservation is created by `load_dynamic`, and REPLAY
// NEVER CALLS `load_dynamic` — it builds its box through `Box_::restore`, which resets `reservations`
// to empty. `commit_reserved_page` services a growth fault only for a page inside a reservation, so
// without this the first stack growth on replay is not serviced and comes back as a divergence:
// M21 would be record-only, and its headline gate (two byte-identical replays of a run that grows
// the stack by 7.72 MiB) could not pass.
//
// `restore`'s reset is CORRECT for the guest's own reservations — replay rebuilds those by
// re-executing the guest's `mach_vm_reserve` landmarks, whose dispatch arms are mirrored. M21's is
// the one entry with no landmark to rebuild from, precisely because M21 keeps it below the trace.
// The asymmetry is exactly one entry wide, and this pins it from both sides.
use retrace_box::Box_;
use retrace_guest::{parse_macho, slice_arm64e, HELLO, HELLO_DYN, DYLD_PATH};
use retrace_trace::Event;

#[test]
fn a_restored_dynamic_box_has_the_same_reservations_as_the_one_it_was_snapshotted_from() {
    let exe = parse_macho(&std::fs::read(HELLO_DYN).unwrap());
    let dyld = parse_macho(slice_arm64e(&std::fs::read(DYLD_PATH).unwrap()));
    let b = Box_::load_dynamic(&exe, &dyld, &["hello_dyn".to_string()]);

    let recorded: Vec<(u64, u64)> = b.reservations().to_vec();
    let (start, end) = b.believed_stack_window();
    assert!(recorded.iter().any(|&(s, l)| (s, s + l) == (start, end)),
        "precondition: load_dynamic must reserve the believed stack {start:#x}..{end:#x}; got {recorded:?}");

    let Event::Snapshot { regs, mem } = b.snapshot() else { panic!("snapshot must be Event::Snapshot") };
    drop(b); // one VM per process (HVF): tear down before restore builds the second

    let r = Box_::restore(&mem, &regs);
    assert_eq!(r.reservations(), &recorded[..],
        "restore must enter landmark 1 with the SAME reservation state record had — otherwise the \
         first stack-growth fault on replay is unserviced and reports as a divergence");
}

/// The control, and it is not optional: it is what catches the mirrored over-correction of adding
/// the reservation unconditionally. A STATIC guest never had a believed-stack reservation, so a
/// restored static box must not acquire one. Without this the fix above passes just as happily when
/// it reserves 7.72 MiB of a static guest's address space that nothing ever reserved.
#[test]
fn a_restored_static_box_has_no_believed_stack_reservation() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let b = Box_::load(&loaded);
    assert!(b.reservations().is_empty(),
        "precondition: the static path reserves nothing; got {:?}", b.reservations());

    let Event::Snapshot { regs, mem } = b.snapshot() else { panic!("snapshot must be Event::Snapshot") };
    drop(b);

    let r = Box_::restore(&mem, &regs);
    assert!(r.reservations().is_empty(),
        "a restored STATIC box must not acquire a believed-stack reservation it never had; got {:?}",
        r.reservations());
}
