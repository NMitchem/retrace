use hv_sys::{Vm, Vcpu, reg, sysreg, MemFlags, EXIT_EXCEPTION};
use retrace_arch::{ec_of, Ec};
use retrace_guest::Loaded;
use retrace_trace::{Regs, Region};

pub const TRAMPOLINE_IPA: u64 = 0x0000_4000; // 16 KiB-aligned (hv_vm_map rejects 4 KiB alignment under the default granule)
pub const STACK_TOP_IPA:  u64 = 0x0002_0000;
const GRANULE: usize = 0x4000; // 16 KiB default granule

// A page-aligned host allocation mapped 1:1 into the guest at `ipa`.
pub struct Backing { pub host: *mut u8, pub ipa: u64, pub len: usize }

pub struct Box_ { pub vcpu: Vcpu, pub vm: Vm, pub backings: Vec<Backing> }

pub enum Stop { Syscall { num: u64, args: [u64;7] }, Other { esr: u64 } }

fn alloc_pages(len: usize) -> (*mut u8, usize) {
    let len = (len + GRANULE - 1) & !(GRANULE - 1);
    // SAFETY: plain RW anon mapping; guest exec comes from stage-2 RWX (verified in spikes).
    let p = unsafe {
        libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ|libc::PROT_WRITE,
                   libc::MAP_ANON|libc::MAP_PRIVATE, -1, 0)
    };
    assert!(p != libc::MAP_FAILED, "mmap backing failed");
    (p as *mut u8, len)
}

impl Box_ {
    pub fn load(loaded: &Loaded) -> Box_ {
        let vm = Vm::create().expect("hv_vm_create");
        let mut vcpu = Vcpu::create(&vm).expect("hv_vcpu_create");
        let mut backings = Vec::new();
        let mut map = |vm: &Vm, backings: &mut Vec<Backing>, ipa: u64, src: &[u8], memsz: usize| {
            let (host, len) = alloc_pages(memsz.max(src.len()).max(GRANULE));
            unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), host, src.len()); }
            vm.map(host, ipa, len, MemFlags::RWX).expect("hv_vm_map");
            backings.push(Backing { host, ipa, len });
        };
        // 1:1 segment mapping (guest VA == IPA because MMU is off).
        for s in &loaded.segments { map(&vm, &mut backings, s.vaddr, &s.data, s.memsz); }
        // Stack.
        map(&vm, &mut backings, STACK_TOP_IPA - GRANULE as u64, &[], GRANULE);
        // EL1 vector table: 16 slots * 0x80 bytes; every slot begins with `hvc #0` (0xd4000002).
        let mut vectors = vec![0u8; 0x800];
        for slot in 0..16 { vectors[slot*0x80..slot*0x80+4].copy_from_slice(&0xd4000002u32.to_le_bytes()); }
        map(&vm, &mut backings, TRAMPOLINE_IPA, &vectors, 0x800);

        // Initial CPU state: EL0t, MMU off, VBAR_EL1 -> trampoline, SP set, PC = entry.
        vcpu.set_sys(sysreg::SCTLR_EL1, 0x30d0_0800).unwrap();  // M(bit0)=0 => MMU off
        vcpu.set_sys(sysreg::VBAR_EL1, TRAMPOLINE_IPA).unwrap();
        vcpu.set_sys(sysreg::SP_EL0, STACK_TOP_IPA).unwrap();
        vcpu.set_reg(reg::CPSR, 0x0).unwrap();                  // EL0t
        vcpu.set_reg(reg::PC, loaded.entry).unwrap();
        Box_ { vm, vcpu, backings }
    }

    pub fn run(&mut self) -> Stop {
        loop {
            let e = self.vcpu.run().expect("hv_vcpu_run");
            if e.reason != EXIT_EXCEPTION { continue; }         // vtimer/canceled: control-plane only
            match ec_of(e.syndrome) {
                Ec::Hvc => {
                    // The trampoline fired because EL0 executed SVC; ESR_EL1 confirms the cause.
                    let esr1 = self.vcpu.get_sys(sysreg::ESR_EL1).unwrap();
                    assert_eq!(ec_of(esr1), Ec::Svc, "trampoline reached by non-SVC cause");
                    let num = self.vcpu.get_reg(reg::x(16)).unwrap();
                    let mut args = [0u64;7];
                    for i in 0..7 { args[i] = self.vcpu.get_reg(reg::x(i as u32)).unwrap(); }
                    return Stop::Syscall { num, args };
                }
                _ => return Stop::Other { esr: e.syndrome },
            }
        }
    }

    /// Replay-only: rebuild the guest from a snapshot's exact regions (no extra stack/trampoline).
    pub fn restore(regions: &[Region], regs: &Regs) -> Box_ {
        let vm = Vm::create().expect("hv_vm_create");
        let vcpu = Vcpu::create(&vm).expect("hv_vcpu_create");
        let mut backings = Vec::new();
        for r in regions {
            let (host, len) = alloc_pages(r.bytes.len().max(GRANULE));
            unsafe { std::ptr::copy_nonoverlapping(r.bytes.as_ptr(), host, r.bytes.len()); }
            vm.map(host, r.ipa, len, MemFlags::RWX).expect("hv_vm_map (restore)");
            backings.push(Backing { host, ipa: r.ipa, len });
        }
        // Fixed sysregs are not stored in the M0 snapshot; re-establish them.
        vcpu.set_sys(sysreg::SCTLR_EL1, 0x30d0_0800).unwrap(); // MMU off
        vcpu.set_sys(sysreg::VBAR_EL1, TRAMPOLINE_IPA).unwrap();
        // Restore captured architectural state.
        for i in 0..31 { vcpu.set_reg(reg::x(i as u32), regs.x[i]).unwrap(); }
        vcpu.set_reg(reg::PC, regs.pc).unwrap();
        vcpu.set_reg(reg::CPSR, regs.cpsr).unwrap();
        vcpu.set_sys(sysreg::SP_EL0, regs.sp_el0).unwrap();
        Box_ { vm, vcpu, backings }
    }

    pub fn set_x0_and_return(&mut self, ret: u64) {
        // Resume EL0 at the instruction after the SVC.
        let elr = self.vcpu.get_sys(sysreg::ELR_EL1).unwrap();
        let spsr = self.vcpu.get_sys(sysreg::SPSR_EL1).unwrap();
        self.vcpu.set_reg(reg::x(0), ret).unwrap();
        self.vcpu.set_reg(reg::PC, elr).unwrap();
        self.vcpu.set_reg(reg::CPSR, spsr).unwrap();
    }

    /// Read `len` bytes of guest memory at `ipa` (1:1, so directly from the backing).
    pub fn read_guest(&self, ipa: u64, len: usize) -> Vec<u8> {
        for bk in &self.backings {
            if ipa >= bk.ipa && ipa + len as u64 <= bk.ipa + bk.len as u64 {
                let off = (ipa - bk.ipa) as usize;
                return unsafe { std::slice::from_raw_parts(bk.host.add(off), len) }.to_vec();
            }
        }
        panic!("read_guest: ipa 0x{ipa:x} len {len} not mapped");
    }
}
