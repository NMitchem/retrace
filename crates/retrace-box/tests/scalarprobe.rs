use retrace_box::*;

// M37 positive control for the §4b fix (spec §4 item 3). `lseek`'s offset register holds 0x4000,
// TRAMPOLINE_IPA. Pre-fix, forward_and_diff's probe rewrote it to the trampoline's host address
// and the kernel returned THAT (a 47-bit number); post-fix a Scalar is forwarded verbatim and
// lseek returns 0x4000. Run red on the pre-fix tree first — the report pastes the value it saw.
#[test]
fn a_scalar_register_holding_a_mapped_ipa_is_forwarded_verbatim() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::SCALARPROBE).unwrap());
    let mut b = Box_::load(&loaded);
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_LSEEK => {
                assert_eq!(args[1], 0x4000, "precondition: the guest asked for offset 0x4000");
                assert!(b.host_span_for_test(0x4000).is_some(), "precondition: 0x4000 is a mapped IPA (the trampoline)");
                let (ret, _ret1, err, _w) = b.forward_and_diff(num, args);
                assert!(!err, "lseek failed: errno {ret}");
                assert_eq!(ret, 0x4000,
                    "lseek returned {ret:#x}: the offset register was rewritten to a host pointer — \
                     forward_and_diff probed a Scalar position (M34 §4b's class)");
                return;
            }
            Stop::Syscall { num, args } => {
                let (ret, _ret1, err, _w) = b.forward_and_diff(num, args);
                b.set_x0_err_and_return(ret, err);
            }
            Stop::Other { esr } => panic!("unexpected exit esr=0x{esr:x}"),
            Stop::Fault { pc, esr, far } => panic!("guest crashed pc=0x{pc:x} esr=0x{esr:x} far=0x{far:x}"),
            Stop::Step => unreachable!("run() does not single-step"),
        }
    }
}
