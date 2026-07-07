use hv_sys::{Vm, Vcpu, reg, sysreg, MemFlags, EXIT_EXCEPTION};
use retrace_arch::{ec_of, Ec};
use retrace_guest::Loaded;
use retrace_trace::{Regs, Region};

pub const TRAMPOLINE_IPA: u64 = 0x0000_4000; // 16 KiB-aligned (hv_vm_map rejects 4 KiB alignment under the default granule)
pub const STACK_TOP_IPA:  u64 = 0x0002_0000;
// Dynamic-path constants (M1 static path is untouched). dyld is a PIE MH_DYLINKER at vmaddr 0,
// so it must be slid to a free base: 5 GiB is above the exe (~4 GiB) and below guest_mmap (8 GiB).
pub const DYLD_BASE: u64 = 0x1_4000_0000;      // 5 GiB slide for dyld
const DYN_STACK_TOP:  u64 = 0x0020_0000;       // 2 MiB (block 0, RW+non-exec by default, below PT_L3_BASE)
const DYN_STACK_SIZE: u64 = 0x0004_0000;       // 256 KiB
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
    // Live page-table state, hoisted here so runtime exec-mmap promotion (set_region_exec) can
    // edit the SAME L2 that build_tables built and continue the SAME L3 allocation window.
    // Both are plain (no Drop) and declared after `mmap_next`, so the vcpu-before-vm drop order
    // is unaffected. `l2_host` is the host pointer of the L2 table backing (at PT_L2_IPA);
    // `next_l3` is the next free L3 IPA (bumped as blocks are promoted).
    l2_host: *mut u8,
    next_l3: u64,
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
    // Promote every 32 MiB L2 block covering [ipa, ipa+len) from a data BLOCK to an L3 TABLE
    // (identity-filled with ATTR_DATA), then set the pages this range covers to `attr`. A block
    // already promoted to a table (by an earlier call/range) is REUSED, not re-promoted: its
    // existing L3 host is resolved from `backings` by the table descriptor's IPA. `alloc_l3`
    // mints a fresh L3, returning its (ipa, host); the returned Vec is the new L3 backings the
    // caller must register (push as a backing; the runtime path also stage-2-maps them). This is
    // the single implementation shared by load-time (`build_tables`) and runtime
    // (`set_region_exec`) so both promote identically.
    fn promote_and_set(
        l2: &mut [u64],
        backings: &[Backing],
        ipa: u64, len: u64, attr: u64,
        mut alloc_l3: impl FnMut() -> (u64, *mut u8),
    ) -> Vec<Backing> {
        let mut created = Vec::new();
        for bi in (ipa / BLK)..=((ipa + len - 1) / BLK) {
            let base = bi * BLK;
            let l3_host = if l2[bi as usize] & 0x3 == DESC_TABLE {
                // Already promoted: reuse the existing L3 (resolve its host by IPA).
                let l3_ipa = l2[bi as usize] & !(GRANULE as u64 - 1);
                backings.iter().find(|b| b.ipa == l3_ipa).map(|b| b.host)
                    .expect("promote_and_set: promoted L3 table backing not found")
            } else {
                // Fresh promotion: allocate an L3, identity-fill it with ATTR_DATA, point L2 at it.
                let (l3_ipa, l3_host) = alloc_l3();
                let l3 = unsafe { std::slice::from_raw_parts_mut(l3_host as *mut u64, 2048) };
                for (j, e) in l3.iter_mut().enumerate() {
                    *e = (base + (j as u64) * GRANULE as u64) | ATTR_DATA | DESC_PAGE;
                }
                l2[bi as usize] = l3_ipa | DESC_TABLE;
                created.push(Backing { host: l3_host, ipa: l3_ipa, len: GRANULE });
                l3_host
            };
            // Set the pages this range covers within this block to `attr`.
            let l3 = unsafe { std::slice::from_raw_parts_mut(l3_host as *mut u64, 2048) };
            let s = ipa.max(base);
            let e = (ipa + len).min(base + BLK);
            let mut p = s & !(GRANULE as u64 - 1);
            while p < e {
                l3[((p - base) / GRANULE as u64) as usize] = p | attr | DESC_PAGE;
                p += GRANULE as u64;
            }
        }
        created
    }

    // W^X identity stage-1 map. Default: every 32 MiB L2 entry is a data BLOCK (RW, non-exec) —
    // covers stack/data/heap/mmap-data and identity-covers the whole 36 GiB space. Each exec range
    // (ipa,len,attr) gets page-granularity pages with `attr` via `promote_and_set` (its covering
    // block(s) are promoted to an L3 table, identity-filled with ATTR_DATA, then exec pages
    // overwritten). Pushes the L2 + every L3 as backings; the caller stage-2-maps them. NEVER
    // file-backed (SPTM). Returns (ttbr0 = PT_L2_IPA, l2_host, next_l3) so runtime promotion can
    // edit the live L2 and continue the same L3 allocation window.
    fn build_tables(backings: &mut Vec<Backing>, exec: &[(u64, u64, u64)]) -> (u64, *mut u8, u64) {
        debug_assert!(exec.iter().all(|&(_, len, _)| len > 0), "exec ranges must be non-empty");
        let (l2_host, l2_len) = alloc_pages(GRANULE);
        let l2 = unsafe { std::slice::from_raw_parts_mut(l2_host as *mut u64, 2048) };
        for (i, e) in l2.iter_mut().enumerate() { *e = ((i as u64) * BLK) | ATTR_DATA | DESC_BLOCK; }
        backings.push(Backing { host: l2_host, ipa: PT_L2_IPA, len: l2_len });

        let mut next_l3 = PT_L3_BASE;
        for &(va, len, attr) in exec {
            let created = {
                let mut alloc_l3 = || {
                    assert!(next_l3 + GRANULE as u64 <= PT_L3_CEIL, "build_tables: too many exec blocks; L3 window exhausted");
                    let (h, _) = alloc_pages(GRANULE);
                    let a = next_l3; next_l3 += GRANULE as u64; (a, h)
                };
                Self::promote_and_set(l2, backings, va, len, attr, &mut alloc_l3)
            };
            backings.extend(created);
        }
        (PT_L2_IPA, l2_host, next_l3)
    }

    /// Runtime exec-mmap promotion: install RO+exec (`ATTR_CODE`) stage-1 pages for [ipa, ipa+len)
    /// by editing the LIVE page tables, so a `PROT_EXEC` mmap becomes executable under W^X. Any
    /// newly-needed L3 tables are anon-allocated (SPTM: never file-backed), stage-2-mapped
    /// immediately (the walker must reach them) AND tracked as backings. No TLB invalidation:
    /// mmap regions are freshly-mapped IPAs the guest has never translated before, so the first
    /// access does a fresh walk and sees ATTR_CODE.
    pub fn set_region_exec(&mut self, ipa: u64, len: u64) {
        let l2_host = self.l2_host;
        assert!(!l2_host.is_null(), "set_region_exec: no live L2 table (restore had no PT_L2 region)");
        let l2 = unsafe { std::slice::from_raw_parts_mut(l2_host as *mut u64, 2048) };
        let mut next_l3 = self.next_l3;
        let created = {
            let mut alloc_l3 = || {
                assert!(next_l3 + GRANULE as u64 <= PT_L3_CEIL, "set_region_exec: too many exec blocks; L3 window exhausted");
                let (h, _) = alloc_pages(GRANULE);
                let a = next_l3; next_l3 += GRANULE as u64; (a, h)
            };
            Self::promote_and_set(l2, &self.backings, ipa, len, ATTR_CODE, &mut alloc_l3)
        };
        self.next_l3 = next_l3;
        // Register each new L3: stage-2-map it (freshly, so the walker reaches it) then track it.
        for bk in created {
            self.vm.map(bk.host, bk.ipa, bk.len, MemFlags::RWX).expect("hv_vm_map (set_region_exec l3)");
            self.backings.push(bk);
        }
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
        let (ttbr0, l2_host, next_l3) = Self::build_tables(&mut backings, &exec);
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
        Box_ { vm, vcpu, backings, mmap_next: MMAP_BASE, l2_host, next_l3 }
    }

    pub fn sp(&self) -> u64 { self.vcpu.get_sys(sysreg::SP_EL0).unwrap() }

    /// Dynamic loader: map a dynamically-linked exe at its own vmaddrs + `/usr/lib/dyld` slid to
    /// `DYLD_BASE`, build the XNU process-start stack, and set PC = dyld's slid entry. Constructs
    /// the initial box state only — running dyld is Task 9. The M1 static `load` path is untouched.
    pub fn load_dynamic(exe: &Loaded, dyld: &Loaded, argv0: &str) -> Box_ {
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
        // exe at its own vmaddrs (arm64 PIE: __PAGEZERO skipped, __TEXT at 4 GiB).
        for s in &exe.segments  { map(&vm, &mut backings, s.vaddr, &s.data, s.memsz); }
        // dyld slid to DYLD_BASE (it is PIE at vmaddr 0 and self-relocates from PC — map raw bytes).
        for s in &dyld.segments { map(&vm, &mut backings, s.vaddr + DYLD_BASE, &s.data, s.memsz); }
        // Dynamic stack (anon, zero-filled). Capture its index NOW, before build_tables pushes the
        // page-table backings, so build_start_stack can address the stack backing by index.
        let stack_idx = backings.len();
        map(&vm, &mut backings, DYN_STACK_TOP - DYN_STACK_SIZE, &[], DYN_STACK_SIZE as usize);
        // EL1 vector table: 16 slots * 0x80; every slot begins with `hvc #0`.
        let mut vectors = vec![0u8; 0x800];
        for slot in 0..16 { vectors[slot*0x80..slot*0x80+4].copy_from_slice(&0xd4000002u32.to_le_bytes()); }
        map(&vm, &mut backings, TRAMPOLINE_IPA, &vectors, 0x800);

        // Build the XNU start stack in the (already-mapped, zeroed) stack backing; get guest SP.
        let sp = Self::build_start_stack(&backings[stack_idx], argv0);

        // W^X exec ranges: trampoline + exe exec segs (unslid) + dyld exec segs (slid).
        let mut exec = vec![(TRAMPOLINE_IPA, 0x800u64, ATTR_TRAMP)];
        for s in &exe.segments  { if s.exec { exec.push((s.vaddr,             s.memsz as u64, ATTR_CODE)); } }
        for s in &dyld.segments { if s.exec { exec.push((s.vaddr + DYLD_BASE, s.memsz as u64, ATTR_CODE)); } }
        let pt_start = backings.len();
        let (ttbr0, l2_host, next_l3) = Self::build_tables(&mut backings, &exec);
        for bk in &backings[pt_start..] { vm.map(bk.host, bk.ipa, bk.len, MemFlags::RWX).expect("hv_vm_map (pt)"); }

        Self::set_pac_keys(&vcpu);
        vcpu.set_sys(sysreg::VBAR_EL1, TRAMPOLINE_IPA).unwrap();
        vcpu.set_sys(sysreg::MAIR_EL1,  MAIR_EL1_V).unwrap();
        vcpu.set_sys(sysreg::TCR_EL1,   TCR_EL1_V).unwrap();
        vcpu.set_sys(sysreg::TTBR0_EL1, ttbr0).unwrap();
        vcpu.set_sys(sysreg::SCTLR_EL1, SCTLR_MMU_ON).unwrap();
        vcpu.set_sys(sysreg::SP_EL0, sp).unwrap();
        vcpu.set_reg(reg::CPSR, 0).unwrap();                        // EL0t
        vcpu.set_reg(reg::PC, dyld.entry + DYLD_BASE).unwrap();     // dyld's SLID entry
        Box_ { vm, vcpu, backings, mmap_next: MMAP_BASE, l2_host, next_l3 }
    }

    // Build the XNU start stack in the (already-mapped, zeroed) stack backing; return the guest SP.
    // Layout at SP (low->high): argc, argv[0..argc], NULL, envp..., NULL, apple..., NULL, then C-strings.
    // First cut — the exact apple[]/env set dyld requires is discovered empirically in Task 9.
    fn build_start_stack(stack: &Backing, argv0: &str) -> u64 {
        let base_ipa = stack.ipa;
        let top = stack.ipa + stack.len as u64;
        let strings: [Vec<u8>; 3] = [
            { let mut v = argv0.as_bytes().to_vec(); v.push(0); v },                 // argv[0]
            b"DYLD_SHARED_REGION=private\0".to_vec(),                                 // envp[0]
            { let mut v = format!("executable_path={argv0}").into_bytes(); v.push(0); v }, // apple[0]
        ];
        let mut p = top;
        let mut addr = [0u64; 3];
        for (i, s) in strings.iter().enumerate() {
            p -= s.len() as u64;
            addr[i] = p;
            unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), stack.host.add((p - base_ipa) as usize), s.len()); }
        }
        let words = [1u64, addr[0], 0, addr[1], 0, addr[2], 0];     // argc, argv, NULL, envp, NULL, apple, NULL
        let sp = (p - words.len() as u64 * 8) & !15u64;             // 16-byte aligned
        for (i, w) in words.iter().enumerate() {
            let off = (sp - base_ipa) as usize + i * 8;
            unsafe { std::ptr::copy_nonoverlapping(w.to_le_bytes().as_ptr(), stack.host.add(off), 8); }
        }
        sp
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

    const MAP_FIXED: u64 = 0x10;
    /// Address + stage-2-map an anon backing for an mmap. FIXED → `addr`; else bump `mmap_next`.
    /// Identical on record and replay. Returns the chosen guest IPA.
    fn map_mmap_region(&mut self, host: *mut u8, rlen: usize, addr: u64, flags: u64) -> u64 {
        let ipa = if flags & Self::MAP_FIXED != 0 { addr } else { self.mmap_next };
        self.vm.map(host, ipa, rlen, MemFlags::RWX).expect("hv_vm_map (mmap region)");
        self.backings.push(Backing { host, ipa, len: rlen });
        if flags & Self::MAP_FIXED == 0 { self.mmap_next += rlen as u64; }
        ipa
    }
    /// RECORD: anon-alloc, `pread` the file extent into it (SPTM: never map the file page itself),
    /// map, return (ipa, staged bytes to record so replay needs no file).
    pub fn guest_mmap_file(&mut self, addr: u64, len: u64, _prot: u64, flags: u64, fd: i32, off: u64)
        -> (u64, Vec<Region>) {
        let (host, rlen) = alloc_pages(len as usize);
        let n = unsafe { libc::pread(fd, host as *mut _, rlen, off as libc::off_t) };
        assert!(n >= 0, "guest_mmap_file: pread failed");
        let ipa = self.map_mmap_region(host, rlen, addr, flags);
        let bytes = unsafe { std::slice::from_raw_parts(host, rlen) }.to_vec();
        (ipa, vec![Region { ipa, bytes }])
    }
    /// REPLAY: anon-alloc (zeroed), address identically (no file access); caller applies the
    /// recorded writes to fill it. Returns the chosen IPA (must equal the recorded `ret`).
    pub fn guest_mmap_replay(&mut self, addr: u64, len: u64, flags: u64) -> u64 {
        let (host, rlen) = alloc_pages(len as usize);
        self.map_mmap_region(host, rlen, addr, flags)
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
        // Recover live page-table state from the snapshot regions so replay can honor runtime
        // exec-mmap promotion too: l2_host is the restored L2 backing (at PT_L2_IPA); next_l3 is
        // the next free L3 slot after every L3 table already present (they were built at load and
        // captured in the initial snapshot), so runtime promotion on replay continues the SAME
        // allocation window and mints IPAs matching the record run.
        let l2_host = backings.iter().find(|b| b.ipa == PT_L2_IPA).map(|b| b.host)
            .unwrap_or(std::ptr::null_mut());
        let next_l3 = backings.iter()
            .filter(|b| b.ipa >= PT_L3_BASE && b.ipa < PT_L3_CEIL)
            .map(|b| b.ipa + GRANULE as u64).max().unwrap_or(PT_L3_BASE);
        Box_ { vm, vcpu, backings, mmap_next: MMAP_BASE, l2_host, next_l3 }
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
