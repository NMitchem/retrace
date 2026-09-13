use retrace_box::*;

// M28 asked whether a FAILING syscall writes into the guest's buffer, drove `sysctl(kern.ostype)`
// into a 2-byte buffer (ENOMEM: "Darwin\0" needs seven), read `buf` before and after, found it
// unchanged, and concluded "the kernel wrote nothing, before or after". That was true of `buf`
// and false of the call: it read `args[2]` and never `args[3]`. xnu's `sysctl()` entry
// (bsd/kern/kern_newsysctl.c, `sysctl`: `if (error && error != ENOMEM) return error;` then
// `suulong(uap->oldlenp, oldlen)`) writes `*oldlenp` back on the ENOMEM path, with the `oldidx`
// the handler left — 0 here, because `sysctl_old_user` refuses before copying. M35 measured it
// end to end first: this fixture recorded cleanly and its replay DIVERGED at `oldlen`
// (`ipa 0x100004010 replay=0x02 recorded=0x00`), because `forward_and_diff` skipped the capture
// on `err` and replay had nothing to apply.
//
// So this test now asserts the call, not the buffer: `buf` is still untouched (M28's datum
// stands), and the eight bytes at `oldlenp` are captured as a write and read back as zero.
#[test]
fn a_failing_sysctl_is_measured_for_writes() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::FAILSYSCTL).unwrap());
    let mut b = Box_::load(&loaded);
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_SYSCTL => {
                // `buf` is the full 64-byte backing (`.space 64` in failsysctl.s); `oldlen` is the
                // 8 bytes the guest set to 2. Both read through the seam, independently of the
                // capture, so the capture can be checked AGAINST them.
                let buf_before = b.read_bytes_for_test(args[2], 64);
                let oldlen_before = b.read_bytes_for_test(args[3], 8);
                assert_eq!(oldlen_before, 2u64.to_le_bytes(), "precondition: the guest asked for 2 bytes");

                let (ret, err, writes) = b.forward_and_diff(num, args);
                assert!(err, "the undersized sysctl should FAIL; got ret={ret} err={err}");
                assert_eq!(ret, 12, "ENOMEM");

                // MEASURED (M28 Task 4, still true): the data buffer is untouched — xnu's
                // `sysctl_old_user` returns ENOMEM before its copyout.
                assert_eq!(buf_before, b.read_bytes_for_test(args[2], 64),
                    "a failing sysctl wrote into `buf` after all; xnu's sysctl_old_user must have \
                     changed shape — re-read it before touching this test");

                // MEASURED (M35): `*oldlenp` is written back as 0 on the ENOMEM path.
                let oldlen_after = b.read_bytes_for_test(args[3], 8);
                assert_eq!(oldlen_after, 0u64.to_le_bytes(),
                    "the kernel writes *oldlenp back on ENOMEM (xnu sysctl(): suulong after the \
                     ENOMEM pass-through); the seam sees it did not — re-read kern_newsysctl.c");

                // And the capture must agree with the seam: some captured region covers the
                // 8 bytes at args[3] and carries the zero. Until M35 `forward_and_diff` returned
                // NO writes on `err` — the `if !err` skip — which is exactly the divergence this
                // fixture's replay showed.
                let captured = writes.iter().find_map(|r| {
                    let end = r.ipa + r.bytes.len() as u64;
                    (r.ipa <= args[3] && args[3] + 8 <= end)
                        .then(|| r.bytes[(args[3] - r.ipa) as usize..][..8].to_vec())
                });
                assert_eq!(captured, Some(oldlen_after),
                    "forward_and_diff captured no write covering *oldlenp on a failing syscall: \
                     the `if !err` skip is dropping a real kernel write (writes captured: {})",
                    writes.len());
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
