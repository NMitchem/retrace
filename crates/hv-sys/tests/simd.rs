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
