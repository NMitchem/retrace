use hv_sys::{Vm, Vcpu, sysreg};

// The MMU/PAC sysregs must be settable and read back on a real vCPU.
#[test]
fn mmu_and_pac_sysregs_roundtrip() {
    let vm = Vm::create().unwrap();
    let vcpu = Vcpu::create(&vm).unwrap();
    for (r, v) in [
        (sysreg::TTBR0_EL1, 0x8000u64),
        (sysreg::TCR_EL1,   0x1_0080_B51C),
        (sysreg::MAIR_EL1,  0xFF),
        (sysreg::APIAKEYLO_EL1, 0x5245545241434531),
        (sysreg::APIAKEYHI_EL1, 0x4D325350494B4559),
    ] {
        vcpu.set_sys(r, v).unwrap();
        assert_eq!(vcpu.get_sys(r).unwrap(), v);
    }
}
