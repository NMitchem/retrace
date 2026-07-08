use retrace_box::Box_;

// The forwarded byte count must never exceed the destination buffer's available backing bytes.
#[test]
fn count_is_clamped_to_backing() {
    assert_eq!(Box_::clamp_count(/*avail=*/8,    /*count=*/1 << 30), 8);     // overflow => clamped
    assert_eq!(Box_::clamp_count(/*avail=*/4096, /*count=*/100),     100);   // fits => unchanged
    assert_eq!(Box_::clamp_count(/*avail=*/4096, /*count=*/1 << 30), 4096);  // overflow => clamped
    assert_eq!(Box_::clamp_count(/*avail=*/0,    /*count=*/16),      0);     // no room => 0
}
