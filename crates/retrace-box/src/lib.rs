use hv_sys::{Vm, Vcpu, reg, sysreg, MemFlags, EXIT_EXCEPTION};
use retrace_arch::{ec_of, Ec};
use retrace_guest::Loaded;
use retrace_trace::{Regs, Region};

pub const TRAMPOLINE_IPA: u64 = 0x0000_4000; // 16 KiB-aligned (hv_vm_map rejects 4 KiB alignment under the default granule)
pub const STACK_TOP_IPA:  u64 = 0x0002_0000;
pub const PTR_WINDOW_CAP: usize = 64 * 1024;
// Bump-allocation base for guest_mmap regions: 8 GiB, within the 36-bit IPA space and above
// the guest's 0x1_0000_0000 segments, so mmap'd regions never collide with loaded segments.
pub const MMAP_BASE: u64 = 0x2_0000_0000;
const GRANULE: usize = 0x4000; // 16 KiB default granule

// A page-aligned host allocation mapped 1:1 into the guest at `ipa`.
pub struct Backing { pub host: *mut u8, pub ipa: u64, pub len: usize }

// Field order is load-bearing: Rust drops struct fields in declaration order, and HVF
// requires `hv_vcpu_destroy` before `hv_vm_destroy` — reordering `vm` before `vcpu` would
// silently reintroduce an HV_BUSY bug on the second in-process VM. `vcpu` MUST stay
// declared before `vm`.
pub struct Box_ {
    vcpu: Vcpu,
    #[allow(dead_code)] // never read; held only so Drop runs hv_vm_destroy after vcpu's
    vm: Vm,
    backings: Vec<Backing>,
    // Next fresh IPA for guest_mmap. Plain u64 (no Drop), declared after `backings` so the
    // load-bearing vcpu-before-vm drop order is unaffected.
    mmap_next: u64,
}

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
        let vcpu = Vcpu::create(&vm).expect("hv_vcpu_create");
        let mut backings = Vec::new();
        let map = |vm: &Vm, backings: &mut Vec<Backing>, ipa: u64, src: &[u8], memsz: usize| {
            let (host, len) = alloc_pages(memsz.max(src.len()).max(GRANULE));
            unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), host, src.len()); }
            assert!(ipa.is_multiple_of(GRANULE as u64), "retrace-box: guest region IPA {ipa:#x} is not 16 KiB-granule-aligned (hv_vm_map requires it); a differently-linked guest needs 16 KiB-aligned segments");
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
        Box_ { vm, vcpu, backings, mmap_next: MMAP_BASE }
    }

    /// Special case for mmap: allocate host pages, map 1:1 at a deterministic fresh IPA,
    /// track as a backing, return the guest IPA. Same call sequence => same IPAs on replay.
    pub fn guest_mmap(&mut self, len: u64) -> u64 {
        let (host, rlen) = alloc_pages(len as usize);
        let ipa = self.mmap_next;
        self.vm.map(host, ipa, rlen, MemFlags::RWX).expect("hv_vm_map (guest_mmap)");
        self.backings.push(Backing { host, ipa, len: rlen });
        self.mmap_next += rlen as u64;
        ipa
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
                    for (i, a) in args.iter_mut().enumerate() { *a = self.vcpu.get_reg(reg::x(i as u32)).unwrap(); }
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
        Box_ { vm, vcpu, backings, mmap_next: MMAP_BASE }
    }

    pub fn set_x0_and_return(&mut self, ret: u64) {
        // Resume EL0 at the instruction after the SVC.
        let elr = self.vcpu.get_sys(sysreg::ELR_EL1).unwrap();
        let spsr = self.vcpu.get_sys(sysreg::SPSR_EL1).unwrap();
        self.vcpu.set_reg(reg::x(0), ret).unwrap();
        self.vcpu.set_reg(reg::PC, elr).unwrap();
        self.vcpu.set_reg(reg::CPSR, spsr).unwrap();
    }

    // Translate a guest IPA to (host pointer, bytes available to the end of its backing).
    fn host_span(&self, ipa: u64) -> Option<(*mut u8, usize)> {
        for bk in &self.backings {
            if ipa >= bk.ipa && ipa < bk.ipa + bk.len as u64 {
                let off = (ipa - bk.ipa) as usize;
                return Some((unsafe { bk.host.add(off) }, bk.len - off));
            }
        }
        None
    }

    /// Record-side memory-diff. For each arg that points into a mapped region, snapshot a
    /// window (capped) and translate it to a host address; forward the real syscall; diff.
    /// M1 assumes the syscall succeeds (see plan's error-ABI note).
    pub fn forward_and_diff(&self, num: u64, args: [u64;7]) -> (u64, Vec<Region>) {
        let mut windows: Vec<(u64, usize, Vec<u8>)> = Vec::new(); // (guest_ipa, len, pre-image)
        let mut hargs = [0i64; 7];
        for i in 0..7 {
            match self.host_span(args[i]) {
                Some((hp, avail)) => {
                    let win = avail.min(PTR_WINDOW_CAP);
                    let pre = unsafe { std::slice::from_raw_parts(hp, win) }.to_vec();
                    windows.push((args[i], win, pre));
                    hargs[i] = hp as i64;
                }
                None => hargs[i] = args[i] as i64,
            }
        }
        // macOS binds `syscall(2)` as `int syscall(int, ...)` (BSD ABI); num is c_int, not
        // c_long. The 7 pointer/scalar args go through the variadic tail unchanged.
        let ret = unsafe {
            libc::syscall(num as libc::c_int, hargs[0], hargs[1], hargs[2],
                          hargs[3], hargs[4], hargs[5], hargs[6])
        } as u64;
        let mut writes = Vec::new();
        for (ipa, len, pre) in windows {
            let (hp, _) = self.host_span(ipa).unwrap();
            let post = unsafe { std::slice::from_raw_parts(hp, len) };
            if post != pre.as_slice() {
                writes.push(Region { ipa, bytes: post.to_vec() });
            }
        }
        (ret, writes)
    }

    /// Replay-side: apply recorded writes to guest memory, then resume. Never executes a syscall.
    pub fn apply_and_return(&mut self, ret: u64, writes: &[Region]) {
        for w in writes {
            let (hp, avail) = self.host_span(w.ipa)
                .unwrap_or_else(|| panic!("apply_and_return: write ipa {:#x} outside any mapped region", w.ipa));
            assert!(w.bytes.len() <= avail,
                "apply_and_return: write at {:#x} ({} bytes) overruns backing ({} avail)", w.ipa, w.bytes.len(), avail);
            unsafe { std::ptr::copy_nonoverlapping(w.bytes.as_ptr(), hp, w.bytes.len()); }
        }
        self.set_x0_and_return(ret);
    }

    /// Compare current guest memory against `expected`; return the first divergence, or None.
    pub fn diff_memory(&self, expected: &[Region]) -> Option<String> {
        for r in expected {
            let (hp, avail) = match self.host_span(r.ipa) {
                Some(s) => s,
                None => return Some(format!("expected region at {:#x} is not mapped in replay", r.ipa)),
            };
            let n = r.bytes.len().min(avail);
            let cur = unsafe { std::slice::from_raw_parts(hp, n) };
            if let Some(off) = (0..n).find(|&i| cur[i] != r.bytes[i]) {
                return Some(format!(
                    "memory divergence at ipa {:#x}: replay=0x{:02x} recorded=0x{:02x}",
                    r.ipa + off as u64, cur[off], r.bytes[off]));
            }
        }
        None
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

    /// Capture all backings + architectural registers as an Event::Snapshot.
    pub fn snapshot(&self) -> retrace_trace::Event {
        let mut mem = Vec::new();
        for bk in &self.backings {
            let bytes = unsafe { std::slice::from_raw_parts(bk.host, bk.len) }.to_vec();
            mem.push(Region { ipa: bk.ipa, bytes });
        }
        let mut x = [0u64;31];
        for (i, xi) in x.iter_mut().enumerate() { *xi = self.vcpu.get_reg(reg::x(i as u32)).unwrap(); }
        let regs = Regs {
            x, pc: self.vcpu.get_reg(reg::PC).unwrap(),
            sp_el0: self.vcpu.get_sys(sysreg::SP_EL0).unwrap(),
            cpsr: self.vcpu.get_reg(reg::CPSR).unwrap(),
        };
        retrace_trace::Event::Snapshot { regs, mem }
    }
    /// The post-`svc` return address (ELR_EL1) — the execution position at a syscall trap.
    pub fn position(&self) -> u64 { self.vcpu.get_sys(sysreg::ELR_EL1).unwrap() }
    /// The current PC (for non-syscall exits).
    pub fn pc(&self) -> u64 { self.vcpu.get_reg(reg::PC).unwrap() }
}
