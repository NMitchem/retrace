// M54 t0b: the guest's frozen commpage copy must not carry the HOST's sleep history into the guest
// clock. `_COMM_PAGE_TIMEBASE_OFFSET` (+0x88) is -(the time the host has spent asleep) in ticks, and
// the guest's `mach_absolute_time` is the synthetic counter plus that word. Copied live, a host
// that had slept more than 2^32 ticks (about three minutes) made the guest's clock wrap below zero,
// and the Swift `DispatchTime.now` sleep check and Rust's channel/park timeouts failed on an
// unchanged tree (`fc_sw_sleep`, `fc_rs_threads`). The word is now pinned to 0 at load.
use retrace_box::{Box_, COMMPAGE_IPA};
use retrace_guest::{parse_macho, slice_arm64e, HELLO_DYN, DYLD_PATH};

#[test]
fn the_guest_clock_does_not_carry_the_hosts_sleep_offset() {
    let exe = parse_macho(&std::fs::read(HELLO_DYN).unwrap());
    let dyld = parse_macho(slice_arm64e(&std::fs::read(DYLD_PATH).unwrap()));
    let b = Box_::load_dynamic(&exe, &dyld, &["hello_dyn".to_string()]);
    let w = u64::from_le_bytes(b.read_guest(COMMPAGE_IPA + 0x88, 8).try_into().unwrap());
    assert_eq!(w, 0, "commpage +0x88 (timebase offset) must be host-independent, was {w:#x}");
}

// Host-independent guard: the load-time test above passes without the fix on a host that never
// slept (its +0x88 is already 0). This one feeds the pin the measured word.
#[test]
fn the_pin_zeroes_only_the_timebase_offset_word() {
    let mut g = vec![0u8; 0x4000];
    for (i, b) in g.iter_mut().enumerate() { *b = (i % 251) as u8 | 1; }
    g[0x88..0x90].copy_from_slice(&0xfffffc6a9fc326d2u64.to_le_bytes());
    let before = g.clone();
    retrace_box::pin_commpage_timebase_offset(&mut g);
    assert_eq!(&g[0x88..0x90], &[0u8; 8]);
    for i in 0..g.len() {
        if !(0x88..0x90).contains(&i) { assert_eq!(g[i], before[i], "byte {i:#x} changed"); }
    }
}
