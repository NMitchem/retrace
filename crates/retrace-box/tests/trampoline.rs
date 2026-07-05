use retrace_box::*;
#[test]
fn el0_svc_reaches_vmm_via_trampoline() {
    let bytes = std::fs::read(retrace_guest::HELLO).unwrap();
    let loaded = retrace_guest::parse_macho(&bytes);
    let mut b = Box_::load(&loaded);
    // First stop must be the guest's write() syscall issued from EL0.
    match b.run() {
        Stop::Syscall { num, args } => {
            assert_eq!(num, retrace_arch::SYS_WRITE);
            assert_eq!(args[0], 1);          // fd = stdout
            assert_eq!(args[2], 6);          // len = 6
        }
        Stop::Other { esr } => panic!("expected SVC-via-trampoline, got esr=0x{esr:x}"),
    }
}
