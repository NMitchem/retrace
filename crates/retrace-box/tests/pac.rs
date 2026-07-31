use retrace_box::{Box_, Stop};
use retrace_arch::SYS_EXIT;
use retrace_guest::{parse_macho, PACGUEST};

#[test]
fn pac_signs_and_authenticates_in_guest() {
    let loaded = parse_macho(&std::fs::read(PACGUEST).unwrap());
    // PACGUEST is plain arm64, not arm64e, so `load`'s derived posture would leave PAC off
    // (as macOS does for a real plain-arm64 main executable). This test is specifically about
    // the PAC-ON in-guest signing/auth mechanism, so tell the box explicitly via
    // `load_with_pac(.., true)` rather than relying on posture derivation from the guest's
    // (in this case misleading) arch. This box is never recorded/replayed, so the posture
    // override cannot create a record/replay mismatch.
    let mut b = Box_::load_with_pac(&loaded, true);
    // The guest makes exactly one syscall (exit), so a single run suffices.
    match b.run() {
        Stop::Syscall { num, args } if num == SYS_EXIT =>
            assert_eq!(args[0], 0, "PAC not engaged or auth failed => x0={}", args[0]),
        Stop::Syscall { .. } => panic!("unexpected syscall"),
        Stop::Other { esr } => panic!("guest faulted esr=0x{esr:x}"),
        Stop::Fault { pc, esr, far } => panic!("guest crashed pc=0x{pc:x} esr=0x{esr:x} far=0x{far:x}"),
        Stop::Step => unreachable!("run() does not single-step"),
    }
}
