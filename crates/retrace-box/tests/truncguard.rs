use retrace_box::Box_;

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
