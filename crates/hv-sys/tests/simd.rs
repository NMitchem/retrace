use hv_sys::{Vm, Vcpu, reg, simd};

// FPCR/FPSR (ordinary Reg values) and the V0-V31 SIMD/FP registers must be settable and read back
// on a real vCPU — the M4 checkpoint machinery depends on this to capture/restore live NEON state
// across a mid-run checkpoint (dyld's early init uses NEON for memcpy/hashing).
#[test]
fn fp_and_simd_regs_roundtrip() {
    let vm = Vm::create().unwrap();
    let vcpu = Vcpu::create(&vm).unwrap();
    // DN (bit 25) + FZ (bit 24): defined, always-implemented FPCR fields.
    vcpu.set_reg(reg::FPCR, 0x0300_0000).unwrap();
    assert_eq!(vcpu.get_reg(reg::FPCR).unwrap(), 0x0300_0000);
    // IOC (bit 0): a defined, writable FPSR cumulative-exception flag.
    vcpu.set_reg(reg::FPSR, 0x0000_0001).unwrap();
    assert_eq!(vcpu.get_reg(reg::FPSR).unwrap(), 0x0000_0001);
    for n in [0u32, 15, 31] {
        let v: u128 = 0x0102_0304_0506_0708_090A_0B0C_0D0E_0F00 | n as u128;
        vcpu.set_simd(simd::q(n), v).unwrap();
        assert_eq!(vcpu.get_simd(simd::q(n)).unwrap(), v, "Q{n} did not round-trip");
    }
}

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
