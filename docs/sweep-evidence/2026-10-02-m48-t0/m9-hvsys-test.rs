use hv_sys::{Vm, Vcpu, simd};

// M48 Task 1: the value must come from the argument, not from whatever the host left in v0. The
// test above passes on the pre-M48 binding by accident: its constant happens to be in v0 at the
// call. This one poisons v0 before every call, so only a binding that passes the vector itself can
// read back what it set (walls.md §4 item 1).
#[test]
fn set_simd_installs_the_passed_value_not_what_the_host_left_in_v0() {
    let vm = Vm::create().unwrap();
    let vcpu = Vcpu::create(&vm).unwrap();
    for n in 0..32u32 {
        let v: u128 = std::hint::black_box(
            0x5eed_0000_0000_0000_0000_0000_0000_0000 ^ ((n as u128 + 1) * 0x0101_0101_0101_0101_0101_0101_0101_0101));
        // SAFETY: writes only v0, which the block declares clobbered.
        unsafe { core::arch::asm!("movi v0.2d, #0xffffffffffffffff", out("v0") _); }
        vcpu.set_simd(simd::q(n), v).unwrap();
        assert_eq!(vcpu.get_simd(simd::q(n)).unwrap(), v, "Q{n}: set_simd installed something other than its argument");
    }
}
