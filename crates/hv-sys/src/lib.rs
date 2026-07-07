#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code)]
pub mod raw {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}
use raw::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HvError(pub u32);
fn check(r: hv_return_t) -> Result<(), HvError> {
    if r == 0 { Ok(()) } else { Err(HvError(r as u32)) }
}

#[derive(Clone, Copy)]
pub struct MemFlags(pub u64);
impl MemFlags {
    // bindgen emits the memory flags as a bare anonymous enum (`HV_MEMORY_READ`),
    // not under an `hv_memory_flags_t_` prefix.
    pub const RWX: MemFlags = MemFlags(
        (HV_MEMORY_READ | HV_MEMORY_WRITE | HV_MEMORY_EXEC) as u64);
}

pub struct Vm(());
impl Vm {
    pub fn create() -> Result<Vm, HvError> {
        check(unsafe { hv_vm_create(std::ptr::null_mut()) })?;
        Ok(Vm(()))
    }
    pub fn map(&self, host: *mut u8, ipa: u64, len: usize, flags: MemFlags) -> Result<(), HvError> {
        check(unsafe { hv_vm_map(host as *mut _, ipa, len, flags.0) })
    }
    pub fn protect(&self, ipa: u64, len: usize, flags: MemFlags) -> Result<(), HvError> {
        check(unsafe { hv_vm_protect(ipa, len, flags.0) })
    }
    pub fn unmap(&self, ipa: u64, len: usize) -> Result<(), HvError> {
        check(unsafe { hv_vm_unmap(ipa, len) })
    }
}
impl Drop for Vm {
    fn drop(&mut self) { unsafe { hv_vm_destroy(); } }
}

#[derive(Clone, Copy)] pub struct Reg(pub hv_reg_t);
#[derive(Clone, Copy)] pub struct SysReg(pub hv_sys_reg_t);
pub mod reg {
    use super::*;
    pub const X0: Reg = Reg(hv_reg_t_HV_REG_X0);
    pub const PC: Reg = Reg(hv_reg_t_HV_REG_PC);
    pub const FP: Reg = Reg(hv_reg_t_HV_REG_FP);
    pub const LR: Reg = Reg(hv_reg_t_HV_REG_LR);
    pub const CPSR: Reg = Reg(hv_reg_t_HV_REG_CPSR);
    pub fn x(n: u32) -> Reg { Reg(hv_reg_t_HV_REG_X0 + n) } // X0..X30 are contiguous
}
pub mod sysreg {
    use super::*;
    pub const SP_EL0:   SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_SP_EL0);
    pub const SP_EL1:   SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_SP_EL1);
    pub const SCTLR_EL1:SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_SCTLR_EL1);
    pub const VBAR_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_VBAR_EL1);
    pub const ELR_EL1:  SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_ELR_EL1);
    pub const SPSR_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_SPSR_EL1);
    pub const ESR_EL1:  SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_ESR_EL1);
    pub const FAR_EL1:  SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_FAR_EL1);
    pub const CPACR_EL1:SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_CPACR_EL1);
    pub const TPIDRRO_EL0:SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_TPIDRRO_EL0);
    pub const TTBR0_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_TTBR0_EL1);
    pub const TCR_EL1:   SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_TCR_EL1);
    pub const MAIR_EL1:  SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_MAIR_EL1);
    pub const TPIDR_EL0: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_TPIDR_EL0);
    pub const APIAKEYLO_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_APIAKEYLO_EL1);
    pub const APIAKEYHI_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_APIAKEYHI_EL1);
    pub const APIBKEYLO_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_APIBKEYLO_EL1);
    pub const APIBKEYHI_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_APIBKEYHI_EL1);
    pub const APDAKEYLO_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_APDAKEYLO_EL1);
    pub const APDAKEYHI_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_APDAKEYHI_EL1);
    pub const APDBKEYLO_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_APDBKEYLO_EL1);
    pub const APDBKEYHI_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_APDBKEYHI_EL1);
    pub const APGAKEYLO_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_APGAKEYLO_EL1);
    pub const APGAKEYHI_EL1: SysReg = SysReg(hv_sys_reg_t_HV_SYS_REG_APGAKEYHI_EL1);
}

pub struct Vcpu { id: hv_vcpu_t, exit: *mut hv_vcpu_exit_t }
impl Vcpu {
    pub fn create(_vm: &Vm) -> Result<Vcpu, HvError> {
        let mut id: hv_vcpu_t = 0;
        let mut exit: *mut hv_vcpu_exit_t = std::ptr::null_mut();
        let cfg = unsafe { hv_vcpu_config_create() };
        check(unsafe { hv_vcpu_create(&mut id, &mut exit, cfg) })?;
        Ok(Vcpu { id, exit })
    }
    pub fn set_reg(&self, r: Reg, v: u64) -> Result<(), HvError> { check(unsafe { hv_vcpu_set_reg(self.id, r.0, v) }) }
    pub fn get_reg(&self, r: Reg) -> Result<u64, HvError> { let mut v=0; check(unsafe { hv_vcpu_get_reg(self.id, r.0, &mut v) })?; Ok(v) }
    pub fn set_sys(&self, r: SysReg, v: u64) -> Result<(), HvError> { check(unsafe { hv_vcpu_set_sys_reg(self.id, r.0, v) }) }
    pub fn get_sys(&self, r: SysReg) -> Result<u64, HvError> { let mut v=0; check(unsafe { hv_vcpu_get_sys_reg(self.id, r.0, &mut v) })?; Ok(v) }
    pub fn set_trap_debug_exceptions(&self, on: bool) -> Result<(), HvError> { check(unsafe { hv_vcpu_set_trap_debug_exceptions(self.id, on) }) }
    /// Run until VMEXIT. Returns (reason, esr_el2, far/ipa) copied out of the exit struct.
    pub fn run(&mut self) -> Result<Exit, HvError> {
        check(unsafe { hv_vcpu_run(self.id) })?;
        // `exit` points at HVF-owned memory populated by hv_vcpu_run; `exception` is a
        // plain struct (not a union), so reading its fields off the reference is safe.
        let e = unsafe { &*self.exit };
        Ok(Exit { reason: e.reason, syndrome: e.exception.syndrome,
                  virtual_address: e.exception.virtual_address })
    }
}
impl Drop for Vcpu { fn drop(&mut self) { unsafe { hv_vcpu_destroy(self.id); } } }

#[derive(Debug, Clone, Copy)]
pub struct Exit { pub reason: u32, pub syndrome: u64, pub virtual_address: u64 }
// Verified in spike: HV_EXIT_REASON_EXCEPTION == 1.
pub const EXIT_EXCEPTION: u32 = hv_exit_reason_t_HV_EXIT_REASON_EXCEPTION;
pub const EXIT_VTIMER: u32 = hv_exit_reason_t_HV_EXIT_REASON_VTIMER_ACTIVATED;
pub const EXIT_CANCELED: u32 = hv_exit_reason_t_HV_EXIT_REASON_CANCELED;
