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
