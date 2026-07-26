use retrace_box::{Box_, Stop};
use retrace_arch::{SYS_EXIT, SYS_OPEN};
use retrace_guest::{parse_macho, FAILSYS};

// Recording the failing open must capture err=true and errno in x0; the guest then exits with
// that errno. ENOENT is 2 on macOS.
#[test]
fn failing_open_records_carry_and_errno() {
    let loaded = parse_macho(&std::fs::read(FAILSYS).unwrap());
    let mut b = Box_::load(&loaded);
    let mut saw_err = false;
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == SYS_OPEN => {
                let (ret, err, _writes) = b.forward_and_diff(num, args);
                assert!(err, "failing open must set carry");
                assert_eq!(ret, 2, "errno should be ENOENT=2");
                saw_err = true;
                b.set_x0_err_and_return(ret, err);
            }
            Stop::Syscall { num, args } if num == SYS_EXIT => { assert_eq!(args[0], 2); break; }
            Stop::Syscall { .. } => {}
            Stop::Other { esr } => panic!("faulted esr=0x{esr:x}"),
            Stop::Fault { pc, esr, far } => panic!("guest crashed pc=0x{pc:x} esr=0x{esr:x} far=0x{far:x}"),
            Stop::Step => unreachable!("run() does not single-step"),
        }
    }
    assert!(saw_err);
}
