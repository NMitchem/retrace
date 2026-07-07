use hv_sys::{Vm, Vcpu, reg, sysreg, MemFlags, EXIT_EXCEPTION};
use retrace_arch::{ec_of, Ec};
use retrace_guest::Loaded;
use retrace_trace::{Regs, Region};

mod cache;
pub use cache::AuthSlot;

pub const TRAMPOLINE_IPA: u64 = 0x0000_4000; // 16 KiB-aligned (hv_vm_map rejects 4 KiB alignment under the default granule)
pub const STACK_TOP_IPA:  u64 = 0x0002_0000;
// Dynamic-path constants (M1 static path is untouched). dyld is a PIE MH_DYLINKER at vmaddr 0,
// so it must be slid to a free base: 5 GiB is above the exe (~4 GiB) and below guest_mmap (8 GiB).
pub const DYLD_BASE: u64 = 0x1_4000_0000;      // 5 GiB slide for dyld
const DYN_STACK_TOP:  u64 = 0x0020_0000;       // 2 MiB (block 0, RW+non-exec by default, below PT_L3_BASE)
const DYN_STACK_SIZE: u64 = 0x0004_0000;       // 256 KiB
pub const PTR_WINDOW_CAP: usize = 64 * 1024;
// Bump-allocation base for guest_mmap / mach_vm allocations: 16 GiB. Within the 36-bit (64 GiB)
// IPA space and ABOVE both the loaded segments (~4-5 GiB) and the demand-paged shared-cache
// window [SHARED_REGION_START, SHARED_REGION_END) below, so guest allocations never collide with
// either.
pub const MMAP_BASE: u64 = 0x4_0000_0000;
const GRANULE: usize = 0x4000; // 16 KiB default granule
// The dyld shared cache is mapped into every process (a nested VM submap) in this fixed VA window
// (~6 GiB base + boot slide, up to ~11.6 GiB). dyld reads/executes it directly at these addresses,
// so the guest needs it in stage-2. We do NOT forward the cache-mapping syscall; instead we
// DEMAND-PAGE: on a guest stage-2 fault in this window, copy the corresponding page from retrace's
// own (identical, read-only) cache mapping into an anon backing at the same IPA. The cache's exec
// regions' stage-1 attributes are pre-set to ATTR_CODE at load (see host_exec_regions) so a text
// fault is a pure stage-2 translation fault and demand-paging needs no runtime promotion / TLBI.
pub const SHARED_REGION_START: u64 = 0x1_8000_0000;
pub const SHARED_REGION_END:   u64 = 0x3_0000_0000;
// The kernel maps a read-only "commpage" of CPU capabilities / cached timing into every process
// at this fixed high VA; dyld reads it during early init. It is within the 36-bit identity space
// (block 2047, a default RW/non-exec data block) but not in the guest's stage-2, so load_dynamic
// stages a FROZEN anon copy of the host commpage here (one granule). Freezing a copy — not the
// live kernel page — makes record and replay read identical bytes; the copy is captured in the
// initial snapshot, so restore re-maps it and replay diverges nowhere.
pub const COMMPAGE_IPA: u64 = 0x0000_000F_FFFF_C000;
// A second commpage-region page the kernel maps just below the data commpage (dyld reads it in
// early init). Same treatment: freeze a host copy. Both pages are one granule each.
pub const COMMPAGE2_IPA: u64 = 0x0000_000F_FFFF_4000;
// Thread-pointer TSD block. The kernel points TPIDRRO_EL0 at the main thread's thread-specific
// data; dyld/libSystem read it (errno slot, pthread self) via `mrs x, TPIDRRO_EL0; ldr .., [x,#N]`.
// We stage one zeroed granule at this fixed IPA (block 0, RW data) and set TPIDRRO_EL0/TPIDR_EL0 to
// it in load_dynamic AND restore, so record and replay share the same thread pointer. Fixed IPA so
// restore can re-establish it without threading the value through the snapshot.
pub const TSD_IPA: u64 = 0x0003_0000;
// Deterministic synthetic timebase: an emulated timebase MRS (Apple fast counter / CNTVCT)
// returns SYNTH_TSC_START + k*SYNTH_TSC_STRIDE for the k-th read. Identical on record & replay
// (both re-execute the same reads from the same entry), so timing dyld folds into memory can't
// diverge. Monotonic and nonzero so a delta is always positive.
const SYNTH_TSC_START:  u64 = 0x0000_0001_0000_0000;
const SYNTH_TSC_STRIDE: u64 = 0x2400; // ~ one 24 MHz-ish tick step per read (value is arbitrary)

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
// CPACR_EL1.FPEN = 0b11 (bits [21:20]): EL0 and EL1 may use FP/SIMD without trapping. dyld's
// early code uses NEON (memcpy, hashing); without this an FP access traps EC=0x07.
const CPACR_FP_ON: u64 = 0x3 << 20;

// Fixed PAC keys (arbitrary constants; identical on record & replay => deterministic signing).
const PAC_KEYS: [(hv_sys::SysReg, u64); 10] = [
    (sysreg::APIAKEYLO_EL1, 0x5245545241434531), (sysreg::APIAKEYHI_EL1, 0x4D325350494B4559),
    (sysreg::APIBKEYLO_EL1, 0x0badc0de0badc0de), (sysreg::APIBKEYHI_EL1, 0xfeedface_feedface),
    (sysreg::APDAKEYLO_EL1, 0x1111111122222222), (sysreg::APDAKEYHI_EL1, 0x3333333344444444),
    (sysreg::APDBKEYLO_EL1, 0x5555555566666666), (sysreg::APDBKEYHI_EL1, 0x7777777788888888),
    (sysreg::APGAKEYLO_EL1, 0x99999999aaaaaaaa), (sysreg::APGAKEYHI_EL1, 0xbbbbbbbbcccccccc),
];

// --- Guest signing oracle (M2-cache Task 4) ---
// Two anon scratch pages, lazy-init'd on the first sign_slots/authenticate, at fixed reserved IPAs
// in block 0's free area [TSD_end 0x34000, DYN_STACK 0x1C0000) — clear of trampoline (0x4000),
// PT_L2 (0x8000), stack (0x1C000), TSD (0x30000), dyn stack, PT_L3 (0x800000+), segments (>=4GiB),
// mmap (>=16GiB), and the cache (>=6GiB). W^X: the STUB page is code (RO+exec, ATTR_CODE) — a fresh
// IPA promoted via set_region_exec; the TABLE page is data (RW+non-exec, block 0's default
// ATTR_DATA). Executing a writable page would HANG hv_vcpu_run (Apple-Silicon W^X). Both anon
// (SPTM: never file-backed).
const SIGN_STUB_IPA:  u64 = 0x0004_0000; // 256 KiB: the signing stub (RO+exec)
const SIGN_TABLE_IPA: u64 = 0x0004_4000; // 272 KiB: the (value, modifier, op) I/O table (RW+non-exec)
// Table entry: value(8) + modifier(8) + op(8). The stub writes the pac*/aut* result back over the
// value field (offset 0). At 24 B/entry a 16 KiB table page holds up to SIGN_CHUNK entries; larger
// batches are processed in successive runs (see run_pac_batch's chunk loop).
const PAC_ENTRY_BYTES: usize = 24;
const SIGN_CHUNK: usize = GRANULE / PAC_ENTRY_BYTES; // 682 entries per 16 KiB table page
// Per-entry op selector (bit1 = authenticate, bit0 = DA/data key), matched by the stub's tbnz
// dispatch: 0=pacia, 1=pacda, 2=autia, 3=autda. A/family only (the v5 cache uses IA/DA).
const OP_PACIA: u64 = 0;
const OP_PACDA: u64 = 1;
const OP_AUTIA: u64 = 2;
const OP_AUTDA: u64 = 3;
// Safety net for the stub's own run() loop: a correct stub reaches its terminating `svc` in ONE
// hv_vcpu_run (its per-entry loop never exits to EL2 until done), so any higher count means a bug.
const SIGN_STUB_BOUND: u32 = 8;
// The signing stub, hand-assembled (verified against spikes/pacsign.c's pac*/aut* encodings; see
// scratchpad stub.s). x9 = table IPA, x10 = entry count; runs at EL0 (ATTR_CODE is EL0-exec) and
// ends with `svc #0` (EL0 cannot HVC) → trampoline → EL2. Uses GPRs + the table only (no stack).
//   loop: cbz x10,done; ldr x0,[x9]; ldr x1,[x9,#8]; ldr x2,[x9,#16]
//         tbnz w2,#1,is_auth; tbnz w2,#0,do_pacda; pacia x0,x1; b store
//   do_pacda: pacda x0,x1; b store
//   is_auth:  tbnz w2,#0,do_autda; autia x0,x1; b store
//   do_autda: autda x0,x1
//   store: str x0,[x9]; add x9,x9,#24; sub x10,x10,#1; b loop
//   done: svc #0
const SIGN_STUB: [u32; 19] = [
    0xb400_024a, 0xf940_0120, 0xf940_0521, 0xf940_0922,
    0x3708_00c2, 0x3700_0062, 0xdac1_0020, 0x1400_0007,
    0xdac1_0820, 0x1400_0005, 0x3700_0062, 0xdac1_1020,
    0x1400_0002, 0xdac1_1820, 0xf900_0120, 0x9100_6129,
    0xd100_054a, 0x17ff_ffef, 0xd400_0001,
];

// The full architectural state sign_slots saves before running its stub and restores after, so a
// mid-run caller (the cache pager) sees no disturbance. The stub's `svc` overwrites ELR/SPSR/ESR/
// FAR_EL1; ELR_EL1 & SPSR_EL1 are load-bearing (set_x0_and_return resumes EL0 from them).
struct SavedState {
    x: [u64; 31],
    pc: u64,
    cpsr: u64,
    sp_el0: u64,
    elr_el1: u64,
    spsr_el1: u64,
    esr_el1: u64,
    far_el1: u64,
}

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

// Minimal mach VM FFI for shared-cache staging (not in the libc crate).
mod machffi {
    extern "C" {
        pub static mach_task_self_: u32;
        // kern_return_t mach_vm_region_recurse(vm_map_t, mach_vm_address_t*, mach_vm_size_t*,
        //     natural_t* nesting_depth, vm_region_recurse_info_t, mach_msg_type_number_t*);
        pub fn mach_vm_region_recurse(target: u32, address: *mut u64, size: *mut u64,
            nesting_depth: *mut u32, info: *mut i32, info_cnt: *mut u32) -> i32;
    }
}

// Enumerate retrace's OWN executable memory regions in [start, end). The shared cache lives in a
// nested VM submap, so recurse (VM_REGION_SUBMAP_INFO_64) to see real per-region protections
// rather than the flat submap. Returns clamped [(addr, size)] for every VM_PROT_EXECUTE region.
fn host_exec_regions(start: u64, end: u64) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    let mut a = start;
    loop {
        let (mut sz, mut depth, mut cnt) = (0u64, 1000u32, 19u32); // VM_REGION_SUBMAP_INFO_COUNT_64
        let mut info = [0i32; 19]; // vm_region_submap_info_64; info[0] = protection
        let kr = unsafe {
            machffi::mach_vm_region_recurse(machffi::mach_task_self_, &mut a, &mut sz,
                &mut depth, info.as_mut_ptr(), &mut cnt)
        };
        if kr != 0 || sz == 0 || a >= end { break; }
        if info[0] & 0x4 != 0 { // VM_PROT_EXECUTE
            let e = (a + sz).min(end);
            if e > a { out.push((a, e - a)); }
        }
        a = a.saturating_add(sz);
    }
    out
}

// Is the host page at `page` (GRANULE-aligned) currently mapped in retrace? mincore returns
// ENOMEM for a range with any unmapped page, so a 0 return means safe to read (used to avoid
// faulting retrace when the guest touches an unmapped hole inside the shared-region window).
fn host_page_mapped(page: u64) -> bool {
    let mut vec = [0u8; 1];
    unsafe { libc::mincore(page as *mut libc::c_void, GRANULE, vec.as_mut_ptr() as *mut libc::c_char) == 0 }
}

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
    // Fault VA (FAR) of the most recent non-syscall VM exit, for legible bring-up diagnostics
    // (describe_stop). Plain u64, declared last so the vcpu-before-vm drop order is unaffected.
    last_far: u64,
    // Deterministic synthetic timebase counter (see SYNTH_TSC_*), advanced per emulated timebase
    // MRS. Plain u64; declared last, so the vcpu-before-vm drop order is unaffected.
    synthetic_tsc: u64,
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

    /// Demand-page a shared-cache page on a guest stage-2 fault: if `ipa` is in the shared-region
    /// window and the corresponding host page is mapped in retrace, copy that page into a fresh
    /// anon backing at the same IPA (the guest then reads/executes the cache there). Returns true
    /// if paged in, false if `ipa` is outside the window / already mapped / an unmapped hole (a
    /// real fault to surface). Deterministic: record and replay fault on the same cache pages and
    /// copy identical host bytes (the cache is read-only and stable within a boot); cache text
    /// stage-1 is pre-set ATTR_CODE at load, so this adds only a stage-2 backing — no promotion,
    /// no TLB invalidation.
    fn try_demand_page(&mut self, ipa: u64) -> bool {
        if !(SHARED_REGION_START..SHARED_REGION_END).contains(&ipa) { return false; }
        let page = ipa & !(GRANULE as u64 - 1);
        if self.host_span(page).is_some() { return false; }  // already mapped => not a missing page
        if !host_page_mapped(page) { return false; }         // unmapped hole => genuine fault
        let (host, rlen) = alloc_pages(GRANULE);
        // SAFETY: host_page_mapped confirmed [page, page+GRANULE) is mapped RO in retrace.
        unsafe { std::ptr::copy_nonoverlapping(page as *const u8, host, GRANULE); }
        self.vm.map(host, page, rlen, MemFlags::RWX).expect("hv_vm_map (demand cache page)");
        self.backings.push(Backing { host, ipa: page, len: rlen });
        true
    }

    /// If `esr1` (an EC=0x18 trapped MSR/MRS) is a READ of a timebase register — Apple's fast
    /// counter `S3_4_C15_C10_6` or the architectural `CNTVCT_EL0`/`CNTPCT_EL0` — service it with a
    /// deterministic synthetic value and skip the instruction (EL0 resumes at ELR+4). Returns true
    /// if emulated. These registers trap under HVF (Apple IMPDEF / counter virtualization) and are
    /// nondeterministic time sources; a monotonic synthetic value identical on record and replay
    /// keeps any timing dyld folds into memory divergence-free. Non-timebase sysreg traps return
    /// false (surfaced as Stop::Other for diagnosis).
    fn try_emulate_timebase(&mut self, esr1: u64) -> bool {
        let iss = esr1 & 0x1ff_ffff;
        let dir = iss & 1;                       // 1 = read (MRS)
        let crm = (iss >> 1) & 0xf;
        let rt  = ((iss >> 5) & 0x1f) as u32;
        let crn = (iss >> 10) & 0xf;
        let op1 = (iss >> 14) & 0x7;
        let op2 = (iss >> 17) & 0x7;
        let op0 = (iss >> 20) & 0x3;
        let apple_tb = op0 == 3 && op1 == 4 && crn == 15 && crm == 10 && op2 == 6; // S3_4_C15_C10_6
        let cntvct   = op0 == 3 && op1 == 3 && crn == 14 && crm == 0 && (op2 == 1 || op2 == 2);
        if dir != 1 || !(apple_tb || cntvct) { return false; }
        self.synthetic_tsc = self.synthetic_tsc.wrapping_add(SYNTH_TSC_STRIDE);
        let v = self.synthetic_tsc;
        if rt != 31 { self.vcpu.set_reg(reg::x(rt), v).unwrap(); } // x31 = XZR: value discarded
        // Skip the trapping instruction: for a synchronous trap ELR_EL1 points AT the mrs, so
        // resume EL0 at ELR+4, restoring the saved EL0 PSTATE.
        let elr = self.vcpu.get_sys(sysreg::ELR_EL1).unwrap();
        let spsr = self.vcpu.get_sys(sysreg::SPSR_EL1).unwrap();
        self.vcpu.set_reg(reg::PC, elr + 4).unwrap();
        self.vcpu.set_reg(reg::CPSR, spsr).unwrap();
        true
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
        Box_ { vm, vcpu, backings, mmap_next: MMAP_BASE, l2_host, next_l3, last_far: 0, synthetic_tsc: SYNTH_TSC_START }
    }

    pub fn sp(&self) -> u64 { self.vcpu.get_sys(sysreg::SP_EL0).unwrap() }

    /// Re-sign a batch of shared-cache auth slots with the GUEST's fixed PAC keys, returning the
    /// signed pointers (in slot order). Each slot is signed in-guest with `pacia` (IA,
    /// `key_is_data == false`) or `pacda` (DA, `key_is_data == true`) — the guest's own keys sign by
    /// definition, so the result authenticates under the same keys the guest will use to load the
    /// cache. Does NOT disturb the caller's guest state: the stub runs on a dedicated scratch region
    /// and the full architectural state is saved and restored around it (see `run_pac_batch`).
    pub fn sign_slots(&mut self, slots: &[AuthSlot]) -> Vec<u64> {
        let entries: Vec<(u64, u64, u64)> = slots
            .iter()
            .map(|s| (s.target_va, s.modifier, if s.key_is_data { OP_PACDA } else { OP_PACIA }))
            .collect();
        self.run_pac_batch(&entries)
    }

    /// The inverse in-guest oracle: authenticate each `(signed_ptr, modifier, key_is_data)` via
    /// `autia`/`autda` and return the recovered pointer — equal to the original target iff the
    /// signature is valid under the guest's keys. Used to round-trip-verify `sign_slots` (and to
    /// audit a re-signed cache). Same scratch + full save/restore. NB: a WRONG modifier makes
    /// `autia`/`autda` fault under FEAT_FPAC (surfaced as a loud panic by `run_sign_stub`).
    pub fn authenticate(&mut self, items: &[(u64, u64, bool)]) -> Vec<u64> {
        let entries: Vec<(u64, u64, u64)> = items
            .iter()
            .map(|&(p, m, kd)| (p, m, if kd { OP_AUTDA } else { OP_AUTIA }))
            .collect();
        self.run_pac_batch(&entries)
    }

    /// Lazy-init the signing scratch on first use: a stub CODE page (RO+exec, ATTR_CODE) and an I/O
    /// TABLE page (RW+non-exec) at fixed reserved IPAs. W^X: the stub is code, so it is promoted to
    /// ATTR_CODE via the live-page-table path (`set_region_exec`) — never mapped writable+exec at
    /// stage-1 (that HANGS the vCPU on Apple Silicon). Both are anon (SPTM: never file-backed). A
    /// stage-2 backing at SIGN_STUB_IPA means already-initialized (nothing else ever maps there).
    fn ensure_sign_scratch(&mut self) {
        if self.host_span(SIGN_STUB_IPA).is_some() { return; }
        // Stub page: write the stub bytes, stage-2-map, then promote its stage-1 to RO+exec.
        let (stub_host, stub_len) = alloc_pages(GRANULE);
        for (i, w) in SIGN_STUB.iter().enumerate() {
            unsafe { std::ptr::copy_nonoverlapping(w.to_le_bytes().as_ptr(), stub_host.add(i * 4), 4); }
        }
        self.vm.map(stub_host, SIGN_STUB_IPA, stub_len, MemFlags::RWX).expect("hv_vm_map (sign stub)");
        self.backings.push(Backing { host: stub_host, ipa: SIGN_STUB_IPA, len: stub_len });
        // Table page: RW+non-exec (block 0's default ATTR_DATA); just stage-2-map it.
        let (tbl_host, tbl_len) = alloc_pages(GRANULE);
        self.vm.map(tbl_host, SIGN_TABLE_IPA, tbl_len, MemFlags::RWX).expect("hv_vm_map (sign table)");
        self.backings.push(Backing { host: tbl_host, ipa: SIGN_TABLE_IPA, len: tbl_len });
        // W^X: RO+exec stage-1 for the stub page only (the table stays ATTR_DATA). Sound without a
        // TLBI: SIGN_STUB_IPA is a fresh IPA the guest has never translated (set_region_exec's
        // invariant), so its first fetch does a clean walk and sees ATTR_CODE.
        self.set_region_exec(SIGN_STUB_IPA, GRANULE as u64);
    }

    /// Run the in-guest pac*/aut* stub over `entries` (`(value, modifier, op)`), returning the
    /// per-entry result. Batched: one `hv_vcpu_run` per SIGN_CHUNK-sized table-full of entries.
    /// Saves the FULL architectural state up front and restores it after every chunk completes, so
    /// a mid-run caller sees byte-identical registers. Drives its OWN run loop (not the main
    /// dispatch), so the stub's terminating `svc` is unambiguous.
    fn run_pac_batch(&mut self, entries: &[(u64, u64, u64)]) -> Vec<u64> {
        self.ensure_sign_scratch();
        let saved = self.save_state();
        let mut out = Vec::with_capacity(entries.len());
        for chunk in entries.chunks(SIGN_CHUNK) {
            // Fill the table page: [value, modifier, op] per 24-byte entry.
            let (tbl, avail) = self.host_span(SIGN_TABLE_IPA).expect("sign table not mapped");
            assert!(chunk.len() * PAC_ENTRY_BYTES <= avail, "sign chunk overflows table page");
            for (i, &(value, modifier, op)) in chunk.iter().enumerate() {
                let base = i * PAC_ENTRY_BYTES;
                for (k, word) in [value, modifier, op].into_iter().enumerate() {
                    unsafe {
                        std::ptr::copy_nonoverlapping(word.to_le_bytes().as_ptr(), tbl.add(base + k * 8), 8);
                    }
                }
            }
            // Point the vCPU at the stub: x9 = table, x10 = count, PC = stub, EL0t.
            self.vcpu.set_reg(reg::x(9), SIGN_TABLE_IPA).unwrap();
            self.vcpu.set_reg(reg::x(10), chunk.len() as u64).unwrap();
            self.vcpu.set_reg(reg::PC, SIGN_STUB_IPA).unwrap();
            self.vcpu.set_reg(reg::CPSR, 0).unwrap(); // EL0t (matches the guest's normal execution EL)
            self.run_sign_stub();
            // Read each result back (written over the value field at offset 0).
            let (tbl, _) = self.host_span(SIGN_TABLE_IPA).expect("sign table not mapped");
            for i in 0..chunk.len() {
                let mut b = [0u8; 8];
                unsafe { std::ptr::copy_nonoverlapping(tbl.add(i * PAC_ENTRY_BYTES), b.as_mut_ptr(), 8); }
                out.push(u64::from_le_bytes(b));
            }
        }
        self.restore_state(&saved);
        out
    }

    /// Drive the stub to its terminating `svc`. The stub's per-entry loop stays in EL0 until done,
    /// so exactly one `hv_vcpu_run` should reach the `svc` → trampoline → EL2 (EC=HVC); we confirm
    /// the underlying EL1 syndrome is SVC (not an FPAC auth-failure / abort). The bounded loop +
    /// panics turn a stub/W^X mistake into a loud failure instead of silent garbage. (A W^X hang
    /// where `hv_vcpu_run` never returns is caught by the external process-group timeout.)
    fn run_sign_stub(&mut self) {
        for _ in 0..SIGN_STUB_BOUND {
            let e = self.vcpu.run().expect("hv_vcpu_run (sign stub)");
            if e.reason != EXIT_EXCEPTION { continue; }
            match ec_of(e.syndrome) {
                Ec::Hvc => {
                    let esr1 = self.vcpu.get_sys(sysreg::ESR_EL1).unwrap();
                    match ec_of(esr1) {
                        Ec::Svc => return, // the stub's terminating svc: this chunk is done
                        other => panic!(
                            "sign stub faulted at EL0: ESR_EL1 EC={other:?} (esr={esr1:#x}) — wrong \
                             modifier (FEAT_FPAC) or a stub bug (bad encoding / non-exec stub page)"
                        ),
                    }
                }
                _ => panic!(
                    "unexpected VM exit during sign stub: syndrome={:#x} far={:#x} (scratch mis-mapped?)",
                    e.syndrome, e.virtual_address
                ),
            }
        }
        panic!("sign stub did not reach its terminating svc within {SIGN_STUB_BOUND} runs (W^X hang / bad stub)");
    }

    fn save_state(&self) -> SavedState {
        let mut x = [0u64; 31];
        for (i, xi) in x.iter_mut().enumerate() { *xi = self.vcpu.get_reg(reg::x(i as u32)).unwrap(); }
        SavedState {
            x,
            pc: self.vcpu.get_reg(reg::PC).unwrap(),
            cpsr: self.vcpu.get_reg(reg::CPSR).unwrap(),
            sp_el0: self.vcpu.get_sys(sysreg::SP_EL0).unwrap(),
            elr_el1: self.vcpu.get_sys(sysreg::ELR_EL1).unwrap(),
            spsr_el1: self.vcpu.get_sys(sysreg::SPSR_EL1).unwrap(),
            esr_el1: self.vcpu.get_sys(sysreg::ESR_EL1).unwrap(),
            far_el1: self.vcpu.get_sys(sysreg::FAR_EL1).unwrap(),
        }
    }

    fn restore_state(&self, s: &SavedState) {
        for (i, &xi) in s.x.iter().enumerate() { self.vcpu.set_reg(reg::x(i as u32), xi).unwrap(); }
        self.vcpu.set_reg(reg::PC, s.pc).unwrap();
        self.vcpu.set_reg(reg::CPSR, s.cpsr).unwrap();
        self.vcpu.set_sys(sysreg::SP_EL0, s.sp_el0).unwrap();
        self.vcpu.set_sys(sysreg::ELR_EL1, s.elr_el1).unwrap();
        self.vcpu.set_sys(sysreg::SPSR_EL1, s.spsr_el1).unwrap();
        self.vcpu.set_sys(sysreg::ESR_EL1, s.esr_el1).unwrap();
        self.vcpu.set_sys(sysreg::FAR_EL1, s.far_el1).unwrap();
    }

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
        // Frozen commpage copies (see COMMPAGE_IPA / COMMPAGE2_IPA): SAFETY — each is a live RO
        // kernel mapping at this exact VA in every process; read one granule of each.
        for &cp in &[COMMPAGE_IPA, COMMPAGE2_IPA] {
            let bytes = unsafe { std::slice::from_raw_parts(cp as *const u8, GRANULE) }.to_vec();
            map(&vm, &mut backings, cp, &bytes, GRANULE);
        }
        // Thread-pointer TSD block (see TSD_IPA): one zeroed granule; TPIDRRO_EL0 points here.
        map(&vm, &mut backings, TSD_IPA, &[], GRANULE);

        // The main executable's mach-header address (dyld4's KernelArgs.mainExecutable): the exe
        // segment whose bytes begin with MH_MAGIC_64. dyld dereferences this to parse the main exe.
        let main_hdr = exe.segments.iter()
            .find(|s| s.data.len() >= 4 && s.data[0..4] == [0xcf, 0xfa, 0xed, 0xfe])
            .map(|s| s.vaddr)
            .expect("load_dynamic: no exe segment carries the Mach-O header (MH_MAGIC_64)");
        // Build the XNU start stack in the (already-mapped, zeroed) stack backing; get guest SP.
        let sp = Self::build_start_stack(&backings[stack_idx], argv0, main_hdr);

        // W^X exec ranges: trampoline + exe exec segs (unslid) + dyld exec segs (slid) + the shared
        // cache's executable regions. The cache text stage-1 must be ATTR_CODE BEFORE the guest
        // translates it, so demand-paging a text page is a pure stage-2 fault (no runtime block
        // promotion, hence no stale-TLB gap). Enumerate retrace's own cache text regions and add
        // them; only pages the guest actually touches get stage-2 backings (demand-paged).
        let mut exec = vec![(TRAMPOLINE_IPA, 0x800u64, ATTR_TRAMP)];
        for s in &exe.segments  { if s.exec { exec.push((s.vaddr,             s.memsz as u64, ATTR_CODE)); } }
        for s in &dyld.segments { if s.exec { exec.push((s.vaddr + DYLD_BASE, s.memsz as u64, ATTR_CODE)); } }
        for (addr, size) in host_exec_regions(SHARED_REGION_START, SHARED_REGION_END) {
            exec.push((addr, size, ATTR_CODE));
        }
        let pt_start = backings.len();
        let (ttbr0, l2_host, next_l3) = Self::build_tables(&mut backings, &exec);
        for bk in &backings[pt_start..] { vm.map(bk.host, bk.ipa, bk.len, MemFlags::RWX).expect("hv_vm_map (pt)"); }

        Self::set_pac_keys(&vcpu);
        vcpu.set_sys(sysreg::VBAR_EL1, TRAMPOLINE_IPA).unwrap();
        vcpu.set_sys(sysreg::MAIR_EL1,  MAIR_EL1_V).unwrap();
        vcpu.set_sys(sysreg::TCR_EL1,   TCR_EL1_V).unwrap();
        vcpu.set_sys(sysreg::TTBR0_EL1, ttbr0).unwrap();
        vcpu.set_sys(sysreg::CPACR_EL1, CPACR_FP_ON).unwrap(); // FPEN=0b11: EL0/EL1 FP/SIMD (dyld uses NEON)
        vcpu.set_sys(sysreg::TPIDRRO_EL0, TSD_IPA).unwrap();   // thread pointer (kernel-provided TSD)
        vcpu.set_sys(sysreg::TPIDR_EL0,   TSD_IPA).unwrap();
        vcpu.set_sys(sysreg::SCTLR_EL1, SCTLR_MMU_ON).unwrap();
        vcpu.set_sys(sysreg::SP_EL0, sp).unwrap();
        vcpu.set_reg(reg::CPSR, 0).unwrap();                        // EL0t
        vcpu.set_reg(reg::PC, dyld.entry + DYLD_BASE).unwrap();     // dyld's SLID entry
        Box_ { vm, vcpu, backings, mmap_next: MMAP_BASE, l2_host, next_l3, last_far: 0, synthetic_tsc: SYNTH_TSC_START }
    }

    // Build dyld4's process-start stack (its `KernelArgs`) in the zeroed stack backing; return SP.
    // `__dyld_start` does `x0 = sp; b start`, and dyld reads the struct at SP as:
    //   [0]  mainExecutable  (pointer to the main exe's mach_header)
    //   [8]  argc
    //   [16] argv[0..argc], NULL
    //        envp[..], NULL
    //        apple[..], NULL
    // (mainExecutable is the dyld4 addition over the classic XNU `argc,argv,envp,apple` frame.)
    // The exact apple[]/env set is refined empirically; this carries argv[0], one env
    // (DYLD_SHARED_REGION=private, so dyld maps a private cache through our file-mmap path), and the
    // `executable_path` apple entry. No env vars (empty envp): dyld then uses the standard shared
    // region (which retrace demand-pages), rather than the DYLD_SHARED_REGION=private path whose
    // cache-mapping syscall retrace does not emulate.
    fn build_start_stack(stack: &Backing, argv0: &str, main_hdr: u64) -> u64 {
        let base_ipa = stack.ipa;
        let top = stack.ipa + stack.len as u64;
        let strings: [Vec<u8>; 2] = [
            { let mut v = argv0.as_bytes().to_vec(); v.push(0); v },                       // argv[0]
            { let mut v = format!("executable_path={argv0}").into_bytes(); v.push(0); v },  // apple[0]
        ];
        let mut p = top;
        let mut addr = [0u64; 2];
        for (i, s) in strings.iter().enumerate() {
            p -= s.len() as u64;
            addr[i] = p;
            unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), stack.host.add((p - base_ipa) as usize), s.len()); }
        }
        // KernelArgs: mainExecutable, argc, argv[0], NULL(argv end), NULL(envp empty), apple[0], NULL
        let words = [main_hdr, 1u64, addr[0], 0, 0, addr[1], 0];
        let sp = (p - words.len() as u64 * 8) & !15u64;             // 16-byte aligned
        for (i, w) in words.iter().enumerate() {
            let off = (sp - base_ipa) as usize + i * 8;
            unsafe { std::ptr::copy_nonoverlapping(w.to_le_bytes().as_ptr(), stack.host.add(off), 8); }
        }
        sp
    }

    /// mach_vm_allocate / _kernelrpc_mach_vm_map_trap (anonymous): allocate `size` bytes of GUEST
    /// memory (these must land in guest IPA space, not be forwarded to the host task). ANYWHERE =>
    /// a fresh deterministic bump IPA (exec regions block-aligned, matching the file-mmap TLB-gap
    /// fix); otherwise map at the requested `addr` (MAP_FIXED-like). `exec` promotes the region to
    /// RO+exec. Returns the chosen IPA. Deterministic: identical call sequence => identical IPAs on
    /// replay.
    pub fn guest_vm_map(&mut self, addr: u64, size: u64, anywhere: bool, exec: bool) -> u64 {
        let (host, rlen) = alloc_pages(size as usize);
        let ipa = if anywhere {
            if exec { self.mmap_next = (self.mmap_next + (BLK - 1)) & !(BLK - 1); }
            let a = self.mmap_next; self.mmap_next += rlen as u64; a
        } else { addr };
        self.vm.map(host, ipa, rlen, MemFlags::RWX).expect("hv_vm_map (guest_vm_map)");
        self.backings.push(Backing { host, ipa, len: rlen });
        if exec { self.set_region_exec(ipa, size); }
        ipa
    }

    /// Read an 8-byte little-endian word from guest memory (for reading in/out pointer args).
    pub fn read_u64(&self, ipa: u64) -> u64 {
        u64::from_le_bytes(self.read_guest(ipa, 8).try_into().unwrap())
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
    const PROT_EXEC: u64 = 0x4;
    /// Address + stage-2-map an anon backing for an mmap. FIXED → `addr`; else bump `mmap_next`.
    /// Identical on record and replay. Returns the chosen guest IPA.
    ///
    /// Step 3c (TLB-gap fix): a non-FIXED `PROT_EXEC` mmap is placed in a FRESH, block-exclusive
    /// 32 MiB block — round `mmap_next` up to the next `BLK` boundary before choosing the IPA.
    /// `set_region_exec` promotes an entire 32 MiB block from a data BLOCK to an L3 TABLE without
    /// TLB invalidation, which is only sound if that block was never translated before. Data mmaps
    /// pack normally and never promote; keeping exec regions block-exclusive guarantees promotion
    /// always hits a pristine block. (A MAP_FIXED exec mmap onto a touched block would need a TLBI;
    /// dyld in private mode is not expected to do that — if a run shows it, add a guest-side TLBI.)
    fn map_mmap_region(&mut self, host: *mut u8, rlen: usize, addr: u64, prot: u64, flags: u64) -> u64 {
        if flags & Self::MAP_FIXED == 0 && prot & Self::PROT_EXEC != 0 {
            self.mmap_next = (self.mmap_next + (BLK - 1)) & !(BLK - 1);
        }
        let ipa = if flags & Self::MAP_FIXED != 0 { addr } else { self.mmap_next };
        self.vm.map(host, ipa, rlen, MemFlags::RWX).expect("hv_vm_map (mmap region)");
        self.backings.push(Backing { host, ipa, len: rlen });
        if flags & Self::MAP_FIXED == 0 { self.mmap_next += rlen as u64; }
        ipa
    }
    /// RECORD: anon-alloc, `pread` the file extent into it (SPTM: never map the file page itself),
    /// map, return (ipa, staged bytes to record so replay needs no file).
    pub fn guest_mmap_file(&mut self, addr: u64, len: u64, prot: u64, flags: u64, fd: i32, off: u64)
        -> (u64, Vec<Region>) {
        let (host, rlen) = alloc_pages(len as usize);
        let n = unsafe { libc::pread(fd, host as *mut _, rlen, off as libc::off_t) };
        assert!(n >= 0, "guest_mmap_file: pread failed");
        let ipa = self.map_mmap_region(host, rlen, addr, prot, flags);
        let bytes = unsafe { std::slice::from_raw_parts(host, rlen) }.to_vec();
        (ipa, vec![Region { ipa, bytes }])
    }
    /// REPLAY: anon-alloc (zeroed), address identically (no file access); caller applies the
    /// recorded writes to fill it. Returns the chosen IPA (must equal the recorded `ret`). `prot`
    /// must match record so the exec block-alignment in `map_mmap_region` chooses the same IPA.
    pub fn guest_mmap_replay(&mut self, addr: u64, len: u64, prot: u64, flags: u64) -> u64 {
        let (host, rlen) = alloc_pages(len as usize);
        self.map_mmap_region(host, rlen, addr, prot, flags)
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
                    // The trampoline (VBAR_EL1) fires `hvc #0` for ANY exception EL0 takes to EL1;
                    // ESR_EL1 says which. SVC => a syscall/mach-trap. Anything else (a trapped
                    // sysreg access, an EL0 fault) is surfaced as Stop::Other carrying the EL1 ESR
                    // so describe_stop can decode it, instead of panicking.
                    let esr1 = self.vcpu.get_sys(sysreg::ESR_EL1).unwrap();
                    match ec_of(esr1) {
                        Ec::Svc => {}
                        // A trapped timebase MRS: emulate it deterministically and resume EL0
                        // without ever surfacing to the record/replay loop (so both stay in
                        // lockstep automatically).
                        Ec::SysReg if self.try_emulate_timebase(esr1) => continue,
                        _ => {
                            self.last_far = self.vcpu.get_sys(sysreg::FAR_EL1).unwrap();
                            return Stop::Other { esr: esr1 };
                        }
                    }
                    let num = self.vcpu.get_reg(reg::x(16)).unwrap();
                    let mut args = [0u64;7];
                    for (i, a) in args.iter_mut().enumerate() { *a = self.vcpu.get_reg(reg::x(i as u32)).unwrap(); }
                    return Stop::Syscall { num, args };
                }
                _ => {
                    // Stage-2 abort taken by the hypervisor (not via the guest VBAR). If it is a
                    // data/instruction abort on a missing shared-cache page, demand-page it and
                    // resume; otherwise surface it for diagnosis.
                    let ec = (e.syndrome >> 26) & 0x3f;
                    let is_abort = matches!(ec, 0x20 | 0x21 | 0x24 | 0x25);
                    if is_abort && self.try_demand_page(e.virtual_address) { continue; }
                    self.last_far = e.virtual_address;
                    return Stop::Other { esr: e.syndrome };
                }
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
        vcpu.set_sys(sysreg::CPACR_EL1, CPACR_FP_ON).unwrap(); // match load: EL0/EL1 FP/SIMD enabled
        vcpu.set_sys(sysreg::TPIDRRO_EL0, TSD_IPA).unwrap();   // match load: thread pointer (harmless for M1)
        vcpu.set_sys(sysreg::TPIDR_EL0,   TSD_IPA).unwrap();
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
        // Minor (a): the `next_l3` recovery below assumes every backing in the L3 window
        // [PT_L3_BASE, PT_L3_CEIL) is a single-GRANULE L3 table (that is the only thing load
        // ever allocates there). Pin that invariant so a stray non-table region in the window
        // can never silently corrupt the recovered allocation cursor.
        for b in backings.iter().filter(|b| b.ipa >= PT_L3_BASE && b.ipa < PT_L3_CEIL) {
            assert_eq!(b.len, GRANULE,
                "restore: backing at L3-window ipa {:#x} has len {} != GRANULE (only L3 tables belong here)", b.ipa, b.len);
        }
        let next_l3 = backings.iter()
            .filter(|b| b.ipa >= PT_L3_BASE && b.ipa < PT_L3_CEIL)
            .map(|b| b.ipa + GRANULE as u64).max().unwrap_or(PT_L3_BASE);
        Box_ { vm, vcpu, backings, mmap_next: MMAP_BASE, l2_host, next_l3, last_far: 0, synthetic_tsc: SYNTH_TSC_START }
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
        // backing so the host kernel can never write past it. x2 is a COUNT, never a pointer, so
        // use the ORIGINAL arg (the generic loop above may have mis-"translated" it to a host
        // pointer if the count value happened to equal a mapped low IPA — e.g. dyld's pread count
        // 0x4000 collides with the trampoline IPA). This both fixes that mis-forward and keeps the
        // host kernel from writing past the destination backing.
        if num == retrace_arch::SYS_READ || num == retrace_arch::SYS_PREAD {
            let count = args[2] as usize;
            hargs[2] = match self.host_span(args[1]) {
                Some((_, avail)) => Self::clamp_count(avail, count) as i64,
                None => count as i64,
            };
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

    /// Bring-up diagnostic: dump x0..x30, SP_EL0, PC, ELR/SPSR/FAR as a multi-line string.
    pub fn dbg_regs(&self) -> String {
        let mut s = String::new();
        for i in 0..31 {
            s += &format!("x{i:<2}={:#018x}  ", self.vcpu.get_reg(reg::x(i as u32)).unwrap());
            if i % 4 == 3 { s.push('\n'); }
        }
        s += &format!("\nsp={:#x} pc={:#x} elr={:#x} far={:#x}",
            self.vcpu.get_sys(sysreg::SP_EL0).unwrap(), self.pc(),
            self.vcpu.get_sys(sysreg::ELR_EL1).unwrap(), self.last_far);
        s
    }

    /// Decode a non-syscall VM exit into a legible one-line diagnostic: the ESR exception class,
    /// the ISS, the faulting VA/IPA (VA==IPA under our identity map) and whether that IPA is
    /// staged in any backing, and the PC. This turns a dyld bring-up failure ("Stop::Other
    /// esr=0x…") into a classified, addressable clue (data/instruction abort at an unmapped IPA,
    /// a trapped MSR/MRS, etc.).
    pub fn describe_stop(&self, esr: u64) -> String {
        let ec = (esr >> 26) & 0x3f;
        let iss = esr & 0x1ff_ffff;
        let class = match ec {
            0x20 | 0x21 => "instruction abort",
            0x24 | 0x25 => "data abort",
            0x18        => "MSR/MRS/sysreg trap",
            0x00        => "unknown/uncategorized",
            _           => "exception",
        };
        let far = self.last_far;
        let mapped = if self.host_span(far).is_some() { "mapped" } else { "UNMAPPED" };
        // For a trampoline-relayed EL0 exception the live PC is the trampoline; the faulting EL0
        // PC is in ELR_EL1. Report both. For a data/instruction abort, ISS[5:0] is the DFSC/IFSC.
        let elr = self.vcpu.get_sys(sysreg::ELR_EL1).unwrap();
        format!("non-syscall exit: {class} (EC={ec:#04x} ISS={iss:#x} FSC={:#x}) far/ipa={far:#x} ({mapped}) pc={:#x} elr={elr:#x}",
                iss & 0x3f, self.pc())
    }
}
