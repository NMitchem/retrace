use retrace_box::Box_;

// The pattern must depend on the guest ADDRESS and nothing else. That is what makes two
// overlapping bands agree on every shared byte, so verification is order-independent.
#[test]
fn the_canary_is_a_pure_function_of_the_address() {
    assert_eq!(Box_::canary_byte(0x1000), Box_::canary_byte(0x1000));
    assert_ne!(Box_::canary_byte(0x1000), Box_::canary_byte(0x1001));
    // Never zero at ipa 0: an all-zero fill is the commonest accidental band content, and a
    // canary that matched it there would be blind in exactly the case this milestone exists for.
    assert_ne!(Box_::canary_byte(0), 0);
    // Nor all-ones, the other common uninitialised fill.
    assert_ne!(Box_::canary_byte(0), 0xFF);
}

#[test]
fn an_intact_canary_is_recognised() {
    let base = 0x4000u64;
    let band: Vec<u8> = (0..64).map(|i| Box_::canary_byte(base + i)).collect();
    assert!(Box_::canary_intact(&band, base));
}

#[test]
fn a_single_disturbed_byte_is_caught() {
    let base = 0x4000u64;
    for victim in [0usize, 1, 31, 63] {
        let mut band: Vec<u8> = (0..64).map(|i| Box_::canary_byte(base + i)).collect();
        band[victim] ^= 0xFF;
        assert!(!Box_::canary_intact(&band, base), "byte {victim} flipped but not caught");
    }
}

// THE case this milestone exists to close: the kernel writes zeros over what was already zeros.
// `overran_window` cannot see it; the canary must.
#[test]
fn zeros_written_over_the_band_are_caught() {
    let base = 0x4000u64;
    assert!(!Box_::overran_window(&[0u8; 64], &[0u8; 64]), "the old detector is blind here");
    assert!(!Box_::canary_intact(&[0u8; 64], base), "the canary must not be");
}

// Matches `overran_window`'s own rule: an empty band (the window covered the whole backing)
// is never an overrun.
#[test]
fn an_empty_band_is_never_disturbed() {
    assert!(Box_::canary_intact(&[], 0x4000));
}
