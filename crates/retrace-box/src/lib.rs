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

pub const PT_L2_IPA:  u64 = 0x8000;           // L2 table (TTBR0 target); one 16 KiB page = 2048 entries
// L3 tables live at 8 MiB..32 MiB inside block 0 (above the stack IPA at 0x1C000, below the
// 32 MiB block boundary), so they're identity-covered by block 0's own L3 (block 0 is already
// L3-promoted for the trampoline). ~1500 tables' worth of room; bump by GRANULE per promoted block.
pub const PT_L3_BASE: u64 = 0x0080_0000;
const PT_L3_CEIL: u64 = 0x0200_0000;          // 32 MiB block boundary
const TCR_EL1_V:  u64 = 0x1_0080_B51C;        // T0SZ=28, TG0=16K, WBWA, inner-share, EPD1, IPS=36-bit (spike-proven)
const MAIR_EL1_V: u64 = 0xFF;                 // attr0 = Normal WBWA
// base 0x30d00800 + M(1) + C(4) + I(0x1000), plus PAC enable bits:
// EnIA(31) | EnIB(30) | EnDA(27) | EnDB(13)
const SCTLR_MMU_ON: u64 = (0x30d0_0800 | 1 | 4 | 0x1000) | 0x8000_0000 | 0x4000_0000 | 0x0800_0000 | 0x2000;

// Fixed PAC keys (arbitrary constants; identical on record & replay => deterministic signing).
const PAC_KEYS: [(hv_sys::SysReg, u64); 10] = [
    (sysreg::APIAKEYLO_EL1, 0x5245545241434531), (sysreg::APIAKEYHI_EL1, 0x4D325350494B4559),
    (sysreg::APIBKEYLO_EL1, 0x0badc0de0badc0de), (sysreg::APIBKEYHI_EL1, 0xfeedface_feedface),
    (sysreg::APDAKEYLO_EL1, 0x1111111122222222), (sysreg::APDAKEYHI_EL1, 0x3333333344444444),
    (sysreg::APDBKEYLO_EL1, 0x5555555566666666), (sysreg::APDBKEYHI_EL1, 0x7777777788888888),
    (sysreg::APGAKEYLO_EL1, 0x99999999aaaaaaaa), (sysreg::APGAKEYHI_EL1, 0xbbbbbbbbcccccccc),
];

// Descriptor low/high attribute bundles (without base address or type bits). AF|SH-inner|AttrIndx0,
// then AP + execute-never bits. W^X: data is writable+non-exec; code/tramp are read-only+exec.
const A_COMMON: u64 = 0x400 /*AF*/ | 0x300 /*SH inner*/;   // AttrIndx 0 = Normal WBWA
const UXN: u64 = 1 << 54;                     // unprivileged (EL0) execute-never
const PXN: u64 = 1 << 53;                     // privileged (EL1) execute-never
const ATTR_DATA:  u64 = A_COMMON | 0x40 /*AP EL0RW*/ | UXN | PXN;   // RW, never executable
const ATTR_CODE:  u64 = A_COMMON | 0xC0 /*AP RO both ELs*/ | PXN;   // RO, EL0-exec (UXN clear), EL1 no-exec
const ATTR_TRAMP: u64 = A_COMMON | 0x80 /*AP EL1-RO, EL0 none*/ | UXN; // RO, EL1-exec (PXN clear)
const DESC_BLOCK: u64 = 0x1;                  // L2 block descriptor
const DESC_TABLE: u64 = 0x3;                  // L2 -> L3 table descriptor
const DESC_PAGE:  u64 = 0x3;                  // L3 page descriptor
const BLK: u64 = 1 << 25;                     // 32 MiB per L2 entry

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
    // SAFETY: plain RW anon mapping; guest exec permission is governed by stage-1 W^X (stage-2
    // stays RWX, the permissive term of the AND).
    let p = unsafe {
        libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ|libc::PROT_WRITE,
                   libc::MAP_ANON|libc::MAP_PRIVATE, -1, 0)
    };
    assert!(p != libc::MAP_FAILED, "mmap backing failed");
    (p as *mut u8, len)
}

// Raw macOS BSD syscall: x16 = number, args in x0..x7, `svc #0x80`. Returns (x0, carry).
// Carry set => error (x0 = errno); clear => success (x0 = full 64-bit result, e.g. an mmap ptr).
// The kernel may clobber the caller-saved scratch registers (x8-x15, x17); they are declared
// clobbered so the compiler keeps no live value across the svc. x18 is platform-reserved — never
// touch it. Flags are NOT preserved (we read the carry via `cset`), so no `preserves_flags`.
// SAFETY: record-only; the caller has already translated guest pointers to host addresses.
unsafe fn host_svc(num: u64, a: [u64; 8]) -> (u64, bool) {
    let ret: u64;
    let carry: u64;
    core::arch::asm!(
        "svc #0x80",
        "cset {c}, cs",
        in("x16") num,
        inout("x0") a[0] => ret,
        in("x1") a[1], in("x2") a[2], in("x3") a[3],
        in("x4") a[4], in("x5") a[5], in("x6") a[6], in("x7") a[7],
        c = out(reg) carry,
        out("x8") _, out("x9") _, out("x10") _, out("x11") _,
        out("x12") _, out("x13") _, out("x14") _, out("x15") _, out("x17") _,
        options(nostack),
    );
    (ret, carry != 0)
}

impl Box_ {
    // W^X identity stage-1 map. Default: every 32 MiB L2 entry is a data BLOCK (RW, non-exec) —
    // covers stack/data/heap/mmap-data and identity-covers the whole 36 GiB space. Each exec range
    // (ipa,len,attr) gets page-granularity pages with `attr`; its covering block(s) are promoted to
    // an L3 table (identity-filled with ATTR_DATA, then exec pages overwritten). Returns PT_L2_IPA.
    // Pushes the L2 + every L3 as backings; the caller stage-2-maps them. NEVER file-backed (SPTM).
    fn build_tables(backings: &mut Vec<Backing>, exec: &[(u64, u64, u64)]) -> u64 {
        debug_assert!(exec.iter().all(|&(_, len, _)| len > 0), "exec ranges must be non-empty");
        let (l2_host, l2_len) = alloc_pages(GRANULE);
        let l2 = unsafe { std::slice::from_raw_parts_mut(l2_host as *mut u64, 2048) };
        for (i, e) in l2.iter_mut().enumerate() { *e = ((i as u64) * BLK) | ATTR_DATA | DESC_BLOCK; }
        backings.push(Backing { host: l2_host, ipa: PT_L2_IPA, len: l2_len });

        // Which 32 MiB blocks contain an exec range? (sorted, deduped)
        let mut blocks: Vec<u64> = exec.iter()
            .flat_map(|&(va, len, _)| (va / BLK)..=((va + len - 1) / BLK)).collect();
        blocks.sort_unstable(); blocks.dedup();

        let mut next_l3 = PT_L3_BASE;
        for bi in blocks {
            let (l3_host, l3_len) = alloc_pages(GRANULE);
            let l3 = unsafe { std::slice::from_raw_parts_mut(l3_host as *mut u64, 2048) };
            let base = bi * BLK;
            for (j, e) in l3.iter_mut().enumerate() {
                *e = (base + (j as u64) * GRANULE as u64) | ATTR_DATA | DESC_PAGE;
            }
            assert!(next_l3 + GRANULE as u64 <= PT_L3_CEIL, "build_tables: too many exec blocks; L3 window exhausted");
            l2[bi as usize] = next_l3 | DESC_TABLE;
            backings.push(Backing { host: l3_host, ipa: next_l3, len: l3_len });
            // overwrite pages this block's exec ranges cover
            for &(va, len, attr) in exec {
                let s = va.max(base);
                let e = (va + len).min(base + BLK);
                let mut p = s & !(GRANULE as u64 - 1);
                while p < e {
                    l3[((p - base) / GRANULE as u64) as usize] = p | attr | DESC_PAGE;
                    p += GRANULE as u64;
                }
            }
            next_l3 += GRANULE as u64;
        }
        PT_L2_IPA
    }

    fn set_pac_keys(vcpu: &Vcpu) {
        for (r, v) in PAC_KEYS { vcpu.set_sys(r, v).unwrap(); }
    }

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

        // Build the W^X identity stage-1 map: the EL1 trampoline + every executable guest
        // segment are RO+exec; everything else defaults to RW+non-exec data blocks.
        let mut exec = vec![(TRAMPOLINE_IPA, 0x800u64, ATTR_TRAMP)];
        for s in &loaded.segments { if s.exec { exec.push((s.vaddr, s.memsz as u64, ATTR_CODE)); } }
        let pt_start = backings.len();
        let ttbr0 = Self::build_tables(&mut backings, &exec);
        for bk in &backings[pt_start..] {          // stage-2-map the new table backings (all anon)
            vm.map(bk.host, bk.ipa, bk.len, MemFlags::RWX).expect("hv_vm_map (pt)");
        }

        // Initial CPU state: EL0t, MMU on with identity map, VBAR_EL1 -> trampoline, SP set, PC = entry.
        vcpu.set_sys(sysreg::MAIR_EL1,  MAIR_EL1_V).unwrap();
        vcpu.set_sys(sysreg::TCR_EL1,   TCR_EL1_V).unwrap();
        vcpu.set_sys(sysreg::TTBR0_EL1, ttbr0).unwrap();
        Self::set_pac_keys(&vcpu);
        vcpu.set_sys(sysreg::SCTLR_EL1, SCTLR_MMU_ON).unwrap();   // was 0x30d00800 (MMU off)
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

    /// Honor munmap (debt #2): drop the backing covering `ipa` and `hv_vm_unmap` its stage-2
    /// range, then release the anon host allocation. No-op if `ipa` isn't a tracked backing
    /// (e.g. munmap of something retrace never mapped itself).
    pub fn guest_munmap(&mut self, ipa: u64, len: u64) {
        if let Some(pos) = self.backings.iter().position(|b| ipa >= b.ipa && ipa < b.ipa + b.len as u64) {
            let bk = self.backings.remove(pos);
            let _ = self.vm.unmap(bk.ipa, bk.len);       // stage-1 identity block stays; stage-2 removed
            // SAFETY: the anon host backing is no longer mapped into the guest; release it.
            unsafe { libc::munmap(bk.host as *mut _, bk.len); }
            let _ = len; // whole-backing unmap for M2's page-granular guests
        }
    }

    /// Honor mprotect (debt #2), best-effort: re-`hv_vm_protect`s the stage-2 range. Fidelity
    /// only — our security boundary is the VMM, so stage-2 stays RWX (the permissive term of
    /// the AND with stage-1 W^X), but accepting the call keeps record/replay from diverging.
    /// A finer prot map lands if a guest ever needs a fault from this.
    pub fn guest_mprotect(&mut self, ipa: u64, len: u64, _prot: u64) {
        let _ = self.vm.protect(ipa, len as usize, MemFlags::RWX);
    }

    /// Sum of tracked backing lengths (test observability: proves mmap grows / munmap shrinks
    /// the map set without exposing `backings` itself).
    pub fn mapped_len(&self) -> usize { self.backings.iter().map(|b| b.len).sum() }

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
        // Fixed sysregs are not stored in the M0 snapshot; re-establish them. The page tables are
        // already in the snapshot regions (captured from load's backings and re-mapped by the loop
        // above), so re-point TTBR0 at them and enable the MMU — do NOT rebuild (that would
        // double-map PT_L2_IPA -> HV_BAD_ARGUMENT and break every M1 replay test).
        vcpu.set_sys(sysreg::MAIR_EL1,  MAIR_EL1_V).unwrap();
        vcpu.set_sys(sysreg::TCR_EL1,   TCR_EL1_V).unwrap();
        vcpu.set_sys(sysreg::TTBR0_EL1, PT_L2_IPA).unwrap();
        Self::set_pac_keys(&vcpu);
        vcpu.set_sys(sysreg::SCTLR_EL1, SCTLR_MMU_ON).unwrap(); // MMU on (tables from snapshot)
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

    /// Carry-aware resume: like set_x0_and_return, but also forces PSTATE.C in the restored
    /// CPSR from `err` (carry set => the syscall failed and x0 = errno). This lets a
    /// deliberately-failing syscall replay identically to how it was recorded.
    pub fn set_x0_err_and_return(&mut self, ret: u64, err: bool) {
        let elr = self.vcpu.get_sys(sysreg::ELR_EL1).unwrap();
        let spsr = self.vcpu.get_sys(sysreg::SPSR_EL1).unwrap();
        let spsr = (spsr & !retrace_arch::PSTATE_C) | if err { retrace_arch::PSTATE_C } else { 0 };
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

    /// Memory-safety clamp (debt #1): a buffer-filling syscall must not have the host kernel write
    /// past the destination backing, so its forwarded byte count is capped at the buffer's
    /// available bytes. Inert when the guest's count already fits (the normal case).
    pub fn clamp_count(avail: usize, count: usize) -> usize { count.min(avail) }

    /// Record-side memory-diff. For each arg that points into a mapped region, snapshot a
    /// window (capped) and translate it to a host address; forward the real syscall via the
    /// raw-svc shim; diff. Returns the full 64-bit x0, the BSD carry flag (`err`), and any
    /// kernel writes. On error (`err`) no writes are captured — a failed syscall wrote nothing.
    pub fn forward_and_diff(&self, num: u64, args: [u64;7]) -> (u64, bool, Vec<Region>) {
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
        // Debt #1: read/pread fill the x1 buffer with up to x2 bytes; cap x2 at that buffer's
        // backing so the host kernel can never write past it. Normal guests already fit (inert).
        if num == retrace_arch::SYS_READ || num == retrace_arch::SYS_PREAD {
            if let Some((_, avail)) = self.host_span(args[1]) {
                debug_assert!((hargs[2] as usize) <= avail,
                    "forward_and_diff: syscall {num} count {} exceeds x1 buffer backing {avail}", hargs[2]);
                hargs[2] = Self::clamp_count(avail, hargs[2] as usize) as i64;
            }
        }
        // Forward via a raw `svc #0x80` shim (not `libc::syscall`, which narrows the return
        // toward 32 bits and hides the BSD carry flag). `hargs` is [i64;7] (x0..x6); build the
        // shim's [u64;8] explicitly, padding x7 = 0.
        let mut sa = [0u64; 8];
        for i in 0..7 { sa[i] = hargs[i] as u64; }
        let (ret, err) = unsafe { host_svc(num, sa) };
        // A failed syscall (carry set) wrote nothing to the guest's buffers, so skip the
        // post-diff write capture entirely.
        let mut writes = Vec::new();
        if !err {
            for (ipa, len, pre) in windows {
                let (hp, _) = self.host_span(ipa).unwrap();
                let post = unsafe { std::slice::from_raw_parts(hp, len) };
                if post != pre.as_slice() {
                    writes.push(Region { ipa, bytes: post.to_vec() });
                }
            }
        }
        (ret, err, writes)
    }

    /// Replay-side: apply recorded writes to guest memory, then resume. Never executes a syscall.
    pub fn apply_and_return(&mut self, ret: u64, err: bool, writes: &[Region]) {
        for w in writes {
            let (hp, avail) = self.host_span(w.ipa)
                .unwrap_or_else(|| panic!("apply_and_return: write ipa {:#x} outside any mapped region", w.ipa));
            assert!(w.bytes.len() <= avail,
                "apply_and_return: write at {:#x} ({} bytes) overruns backing ({} avail)", w.ipa, w.bytes.len(), avail);
            unsafe { std::ptr::copy_nonoverlapping(w.bytes.as_ptr(), hp, w.bytes.len()); }
        }
        self.set_x0_err_and_return(ret, err);
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
