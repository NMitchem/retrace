use retrace_box::{Box_, Stop};

// M27: the guard band is DIRECT evidence, not a heuristic. Between the pre-image and post-image
// copies the only thing that runs is `host_svc` — the guest vCPU is halted and recorder threads are
// banned — so any pre != post byte in a band placed PAST the diff window is provably a kernel write
// that ran past that window. Kernel writes into a destination buffer are contiguous from the buffer
// start, so an overrun cannot skip the band.
#[test]
fn a_changed_guard_band_means_the_write_ran_past_the_window() {
    assert!(Box_::overran_window(&[0u8; 64], &[1u8; 64]),
        "a wholly rewritten band is an overrun");
    assert!(Box_::overran_window(&[0u8; 64], &{ let mut b = [0u8; 64]; b[0] = 1; b }),
        "ONE changed byte is enough: writes are contiguous, so the first byte past the window is \
         the one an overrun touches first");
}

#[test]
fn an_unchanged_guard_band_is_not_an_overrun() {
    assert!(!Box_::overran_window(&[0u8; 64], &[0u8; 64]));
    assert!(!Box_::overran_window(&[7u8; 64], &[7u8; 64]));
}

// A band that could not be taken (the window already covers the whole backing) is never an
// overrun: nothing can be past the backing without a separate memory-safety bug, which is a
// different failure with its own loud symptom.
#[test]
fn an_empty_guard_band_is_never_an_overrun() {
    assert!(!Box_::overran_window(&[], &[]));
}

// M28: the POSITIVE control. Everything else touching the guard band is a NEGATIVE control —
// `bigread_e2e` and `memdiff`'s M26 guard prove it does not FALSE-fire. Nothing proved it fires at
// all. `overran_window`'s unit tests above cover `!pre.is_empty() && pre != post`, the one part
// that cannot be wrong; the offset (`hp.add(win)`), the sizing (`GUARD_BAND.min(avail - win)`) and
// whether the assert is REACHED were covered by nothing. `let band = 0;` passed the entire
// 523-test gate identically to the shipped code, and the M27 band has never been observed to fire
// on a real syscall — M26's "fired exactly once" was the tail-of-window PROTOTYPE, a different
// detector, and /bin/ps was measured NOT to trip this one.
//
// `fstat` is the right syscall here and `read` is the wrong one. `read` is in
// `retrace_arch::dest_buffer`, so `diff_window` widens its window to the full byte count no matter
// how small the cap is, and this test would be vacuously green. `fstat` is deliberately absent from
// that table (its length is not in a register), so a shrunken cap really does truncate it.
//
// MEASURED: `sizeof(struct stat)` is 144 bytes on this SDK, so a 64-byte window
// is genuinely overrun by a real kernel write.
#[test]
#[should_panic(expected = "changed a byte in the")]
fn the_band_fires_when_the_kernel_writes_past_the_window() {
    const CAP: usize = 64;
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::FILEIO).unwrap());
    let mut b = Box_::load(&loaded);
    b.set_window_cap_for_test(CAP);
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_FSTAT => {
                b.forward_and_diff(num, args);
                // Deliberately worded to share NO substring with the assert's message, so
                // `should_panic` cannot be satisfied by this panic instead of the real one.
                panic!("NOT-THE-GUARD-BAND: fstat wrote past a {CAP}-byte window and nothing fired");
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
