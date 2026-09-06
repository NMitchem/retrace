use retrace_box::*;

// M28: does a FAILING syscall write into the guest's buffer?
//
// `forward_and_diff` answers "no" by construction — it skips the whole post-diff block when the
// carry flag is set, and the M27 guard band sits inside that same block, so neither the capture nor
// the detector runs. The comment there states it as fact; nothing has measured it. This test drives
// the one case the README already names as suspect.
//
// It asserts only what is already known (the call fails) and PRINTS the rest. Task 5 turns the
// measured answer into an assertion — writing one now would be guessing at the result.
#[test]
fn a_failing_sysctl_is_measured_for_writes() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::FAILSYSCTL).unwrap());
    let mut b = Box_::load(&loaded);
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_SYSCTL => {
                // Snapshot the destination before, so the measurement does not depend on
                // forward_and_diff's own (skipped) capture. `buf` is the full 64-byte backing
                // (`.space 64` in failsysctl.s), read whole rather than just the 2 requested bytes
                // so the contamination check covers everything the kernel could have touched.
                let before = b.read_bytes_for_test(args[2], 64);
                let (ret, err, writes) = b.forward_and_diff(num, args);
                let after = b.read_bytes_for_test(args[2], 64);
                assert!(err, "the undersized sysctl should FAIL; got ret={ret} err={err}");
                // The `if !err` skip means `forward_and_diff` never even LOOKS for writes on this
                // path — pin that directly, not just its effect on `buf`.
                assert!(writes.is_empty(),
                    "forward_and_diff captured writes on a failing syscall; the `if !err` skip is \
                     no longer skipping");
                // MEASURED (M28 Task 4): this failing sysctl writes NOTHING into the guest buffer,
                // so `forward_and_diff`'s `if !err` skip loses nothing HERE. That is a measurement
                // of one case, not a proof about failing syscalls in general — the gate stays open,
                // now with one datum in it instead of none.
                assert_eq!(before, after,
                    "a failing sysctl wrote into the guest buffer after all: the `if !err` skip is \
                     dropping real kernel writes, and this test's premise has changed — see the \
                     M28 spec's Component 3");
                return;
            }
            Stop::Syscall { num, args } => {
                let (ret, _e, _w) = b.forward_and_diff(num, args);
                b.set_x0_and_return(ret);
            }
            Stop::Other { esr } => panic!("unexpected exit esr=0x{esr:x}"),
            Stop::Fault { pc, esr, far } => panic!("guest crashed pc=0x{pc:x} esr=0x{esr:x} far=0x{far:x}"),
            Stop::Step => unreachable!("run() does not single-step"),
        }
    }
}
