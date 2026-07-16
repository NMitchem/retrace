use retrace_box::{Box_, Stop};
use retrace_arch::SYS_EXIT;
use retrace_guest::{parse_macho, PACGUEST};

#[test]
fn pac_signs_and_authenticates_in_guest() {
    let loaded = parse_macho(&std::fs::read(PACGUEST).unwrap());
    let mut b = Box_::load(&loaded);
    // The guest makes exactly one syscall (exit), so a single run suffices.
    match b.run() {
        Stop::Syscall { num, args } if num == SYS_EXIT =>
            assert_eq!(args[0], 0, "PAC not engaged or auth failed => x0={}", args[0]),
        Stop::Syscall { .. } => panic!("unexpected syscall"),
        Stop::Other { esr } => panic!("guest faulted esr=0x{esr:x}"),
        Stop::Step => unreachable!("run() does not single-step"),
    }
}
