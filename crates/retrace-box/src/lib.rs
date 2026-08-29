use hv_sys::{Vm, Vcpu, reg, sysreg, simd, MemFlags, EXIT_EXCEPTION};
use retrace_arch::{ec_of, Ec};
use retrace_guest::Loaded;
use retrace_trace::{Regs, Region};

mod cache;
pub use cache::AuthSlot;
use cache::{walk_page, CacheMeta, DEFAULT_CACHE_PATH};

mod sig;
pub mod thread;
pub use sig::{
    build_frame, choose_frame_base, decode_act, decode_stack, encode_oldact, encode_oldstack,
    sigreturn_token, Disposition, EntryRegs, FrameInput, NeonState, SigAction, SigTable,
    ThreadState, FRAME_LEN, FRAME_MCONTEXT_OFF, FRAME_SIGINFO_OFF, FRAME_SLACK, FRAME_UCONTEXT_OFF,
};

/// PSTATE bits a guest may set through its own signal frame: NZCV only.
///
/// The frame sits on the GUEST's stack, so `__ss.__cpsr` is guest-writable before `sigreturn`.
/// Restoring it verbatim into the resume PSTATE would let a guest select its own exception level.
/// This is the only place in M12 where guest-controlled bytes reach a system register.
///
/// Masking to the user bits and forcing EL0t coincide here, which is worth stating rather than
/// leaving as a coincidence: this guest runs at **EL0t**, whose mode bits are zero (`CPSR = 0` at
/// load — see `load_static`/`load_dynamic`). So zeroing everything outside NZCV lands exactly on
/// the guest's own legitimate execution state. A design that ran the guest at any other EL would
/// need to OR the trusted mode bits back in rather than rely on that.
pub const PSTATE_USER_MASK: u64 = 0xf000_0000;

pub const TRAMPOLINE_IPA: u64 = 0x0000_4000; // 16 KiB-aligned (hv_vm_map rejects 4 KiB alignment under the default granule)
pub const STACK_TOP_IPA:  u64 = 0x0002_0000;
// Dynamic-path constants (M1 static path is untouched). dyld is a PIE MH_DYLINKER at vmaddr 0,
// so it must be slid to a free base: 5 GiB is above the exe (~4 GiB) and below guest_mmap (8 GiB).
pub const DYLD_BASE: u64 = 0x1_4000_0000;      // 5 GiB slide for dyld
// The dynamic guest's stack. The TOP is load-bearing in a way the size is not: libstd's
// `install_main_guard` mmaps its stack-overflow guard page MAP_FIXED at
// `pthread_get_stackaddr_np() - pthread_get_stacksize_np()`, and macOS 26's libpthread reports the
// main thread's size as a CONSTANT 0x7fc000 (8 MiB minus one granule) — it calls
// `getrlimit(RLIMIT_STACK)` and then ignores the reply (measured: answering 0x10000000 instead of
// 0x40000 left the computed address bit-identical). Retrace cannot influence that subtrahend, so the
// top must leave room above it: at the old 2 MiB the subtraction UNDERFLOWED to 0xffffffffffa04000
// and the guard-page mmap was refused, which is what parked rung 1 (`hello_rust`) through M7 and M8.
//
// 40 MiB puts the computed guard page at 0x2004000 — just above PT_L3_CEIL (32 MiB), so it cannot
// collide with the L3 translation tables, and comfortably below the stack backing it guards. Only
// the top moved: the guard page lands in FREE address space and is mapped fresh on demand, so the
// stack itself stays 256 KiB and no per-syscall memory diff got any more expensive. (Backing a full
// 8 MiB instead was measured at ~1.7x on `hello_rust` and far worse across the dyld suite — the
// diff scales with total mapped memory.) `stack_geometry_tests` pins the arithmetic.
//
// Fidelity gap, unchanged and tracked as spec risk R3: the guest BELIEVES it has an 8 MiB stack
// while 256 KiB is backed, so a deep enough recursion faults on unmapped IPA instead of striking
// the guard page. That was equally true when the stack sat at 2 MiB.
const DYN_STACK_TOP:  u64 = 0x0280_0000;       // 40 MiB — above PT_L3_CEIL by libpthread's 0x7fc000
const DYN_STACK_SIZE: u64 = 0x0004_0000;       // 256 KiB
/// macOS 26 libpthread's main-thread stack size: 8 MiB minus one 16 KiB page. It calls
/// `getrlimit(RLIMIT_STACK)` and then IGNORES the reply — M8 measured that answering 0x10000000
/// instead of 0x40000 left libstd's computed guard address bit-identical, so retrace cannot
/// influence this subtrahend and must lay its geometry out around it.
pub const LIBPTHREAD_MAIN_STACK_SIZE: u64 = 0x7fc000;
/// The guest's BELIEVED stack bottom — where libstd's `install_main_guard` mmaps its guard page.
const GUARD_PAGE_IPA: u64 = DYN_STACK_TOP - LIBPTHREAD_MAIN_STACK_SIZE;   // 0x2004000
/// The believed-but-unbacked window M21 reserves: one granule ABOVE the guard page (so the guard
/// stays in free space and faults at stage 1), up to the real backed stack bottom.
const GUARD_TOP: u64 = GUARD_PAGE_IPA + GRANULE as u64;                    // 0x2008000
const DYN_STACK_BOTTOM: u64 = DYN_STACK_TOP - DYN_STACK_SIZE;              // 0x27C0000
pub const PTR_WINDOW_CAP: usize = 64 * 1024;
// Bump-allocation base for guest_mmap / mach_vm allocations: 40 GiB. Within the 36-bit (64 GiB)
// IPA space and ABOVE the loaded segments (~4-5 GiB), the demand-paged shared-cache window
// [SHARED_REGION_START, SHARED_REGION_END), AND libmalloc's FIXED 24-GiB nano "pointer range"
// reservation [NANO_BAND_START, NANO_BAND_END). The last is critical: libmalloc reserves that
// band and then commits nano sub-ranges at EXACT addresses inside it, both with the ANYWHERE bit
// CLEAR (`_nano_common_map_vm_space` in nano_malloc_common.c) — i.e. plain FIXED placement, not a
// hinted-ANYWHERE request. Retrace's FIXED map path (`unmap_overlapping` + map) never consults
// `range_is_free`, so nano's commits are unaffected by whether `range_is_free` treats reservations
// as occupied (see M2-carveout, which made `range_is_free` reservation-aware for ANYWHERE placement
// only — `nano_fixed_commit_lands_at_requested_base_inside_a_reservation` in
// crates/retrace-box/tests/carveout.rs is the regression guard). If the bump allocator's IPAs fell
// inside the band, an early dyld allocation would still occupy a nano sub-range and nano's FIXED
// commit there would unmap-and-overwrite it — leaving libmalloc's nano zone pointing at the wrong
// page (wild-pointer abort). Basing bumps ABOVE the band keeps it pristine for libmalloc.
pub const NANO_BAND_START: u64 = 0x4_0000_0000;
pub const NANO_BAND_END:   u64 = 0xA_0000_0000; // 0x4_0000_0000 + 0x6_0000_0000 (24 GiB)
pub const MMAP_BASE: u64 = NANO_BAND_END;
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
// We stage a zeroed region and set TPIDRRO_EL0 to `TSD_IPA` in load_dynamic AND restore, so record
// and replay share the same thread pointer. Fixed IPA so restore can re-establish it without
// threading the value through the snapshot. The thread pointer points into the MIDDLE of the
// staged region [TSD_REGION_BASE, TSD_REGION_BASE+TSD_REGION_SIZE): libpthread/libplatform touch
// both positive TSD-key slots (`[tp,#+N]`) AND negative pthread-struct fields (`[tp,#-N]`, observed
// as low as tp-0xE0), so the backing must span generously below and above tp. The whole region sits
// in block 0's free area below the sign scratch (0x40000) and above PT_L2 (0x8000).
// TPIDR_EL0 is a SEPARATE register and is NOT a second TSD pointer: macOS 26 reads the guest's
// current CPU number from TPIDR_EL0[11:0] and cluster number from TPIDR_EL0[>=12] (see the
// set-sys call sites in load_dynamic/restore for the full rationale); it is set to 0 (cpu 0 /
// cluster 0), never to `TSD_IPA` (M2-cpuid).
pub const TSD_IPA: u64 = 0x0003_0000;
const TSD_REGION_BASE: u64 = 0x0002_8000; // 0x8000 below tp
const TSD_REGION_SIZE: u64 = 0x0001_0000; // 4 granules; tp = TSD_IPA sits 0x8000 into it
// Deterministic synthetic timebase: an emulated timebase MRS (Apple fast counter / CNTVCT)
// returns SYNTH_TSC_START + k*SYNTH_TSC_STRIDE for the k-th read. Identical on record & replay
// (both re-execute the same reads from the same entry), so timing dyld folds into memory can't
// diverge. Monotonic and nonzero so a delta is always positive.
const SYNTH_TSC_START:  u64 = 0x0000_0001_0000_0000;
const SYNTH_TSC_STRIDE: u64 = 0x2400; // ~ one 24 MHz-ish tick step per read (value is arbitrary)

pub const PT_L2_IPA:  u64 = 0x8000;           // L2 table (one level below the L1 TTBR0 target); one 16 KiB page = 2048 entries
pub const PT_L1_IPA:  u64 = 0xC000;           // L1 table (TTBR0 target under the 47-bit VA); one
                                              // 16 KiB page, only entry[0] valid (-> L2). Free
                                              // block-0 page between L2 (0x8000) and the stack (0x1C000).
// L3 tables live at 8 MiB..32 MiB inside block 0 (above the stack IPA at 0x1C000, below the
// 32 MiB block boundary), so they're identity-covered by block 0's own L3 (block 0 is already
// L3-promoted for the trampoline). ~1500 tables' worth of room; bump by GRANULE per promoted block.
pub const PT_L3_BASE: u64 = 0x0080_0000;
const PT_L3_CEIL: u64 = 0x0200_0000;          // 32 MiB block boundary
// arm64e data-pointer PAC placement: TBI0(bit37)+TBID0(bit51) match Apple's user TCR so a signed
// DATA pointer's PAC lands in [54:47] with the top byte (incl. bit 63) preserved = 0. Without TBI
// the 47-bit-VA PAC field spans [63:56]∪[54:47]; a re-signed class_data_bits pointer then sets
// bit 63, which objc reads as FAST_IS_RW_POINTER (isRealized) → spurious already-realized →
// validateAlreadyRealizedClass fatal. See docs/.../2026-07-14-retrace-m2-tbi-design.md.
const TCR_EL1_V:  u64 = 0x8_0021_0080_B511;    // +TBI0+TBID0. T0SZ=17 (47-bit VA), TG0=16K, WBWA, inner-share, EPD1, IPS=36-bit
const MAIR_EL1_V: u64 = 0xFF;                 // attr0 = Normal WBWA
// base 0x30d00800 + M(1) + C(4) + I(0x1000). PAC is NOT in the base: it is per-guest (see below).
const SCTLR_MMU_ON_BASE: u64 = 0x30d0_0800 | 1 | 4 | 0x1000;
// EnIA(31) | EnIB(30) | EnDA(27) | EnDB(13)
const SCTLR_PAC_EN: u64 = 0x8000_0000 | 0x4000_0000 | 0x0800_0000 | 0x2000;

/// The main executable's load address. Every guest this repo builds links `__TEXT` at
/// `0x1_0000_0000`, and replay has NO independent way to learn it — a snapshot is a flat set of IPA
/// regions with no Mach-O in sight. Naming it makes the one assumption `pac_posture_from_memory`
/// rests on explicit and checkable instead of buried.
pub const EXE_BASE: u64 = 0x1_0000_0000;

/// The `bsdthread_register` feature word retrace reports to the guest.
///
/// **Synthesized, never the host's.** The host is retrace's own already-registered process, so its
/// answer describes retrace, not the guest — the same category error as M10's fd table and M11's
/// signal dispositions. A fixed constant is also what makes this deterministic for free: both runs
/// compute the identical value with nothing recorded (symmetry rule 2's argument, applied to a
/// return value rather than an instruction).
///
/// The value is not arbitrary. It is the SMALLEST word satisfying every gate measured in the
/// shipped binaries (see the test beside this constant for the four, each with its address).
/// Bit 7 is deliberately CLEAR: with it set, libdispatch additionally registers
/// `_dispatch_workloop_worker_thread`, and the workloop path is out of M18's scope.
pub const WORKQ_FEATURE_WORD: u32 = 0x4000_005E;

/// **The one derivation.** All four SCTLR install sites go through this (directly, or through the
/// two wrappers below). macOS enables pointer authentication per process, only for `arm64e` main
/// executables — a plain-`arm64` process sees `PAC*`/`AUT*` as NOPs and `BRAA`/`BLRAA` as
/// `BR`/`BLR`. retrace must match, or arm64e cache code and plain-arm64 client code disagree about
/// whether a pointer carries a signature (M7's wall).
pub fn pac_posture(cpusubtype: u32) -> bool {
    (cpusubtype & 0x00ff_ffff) == retrace_arch::CPU_SUBTYPE_ARM64E
}

/// SCTLR_EL1 for a guest with the given posture. Never build this value ad hoc.
fn sctlr_mmu_on(pac_enabled: bool) -> u64 {
    SCTLR_MMU_ON_BASE | if pac_enabled { SCTLR_PAC_EN } else { 0 }
}

/// Re-derive the posture from a snapshot's own memory — `restore()`'s only route, since its inputs
/// are `(regions, regs)` and `Regs` is `{x[31], pc, sp_el0, cpsr}`. Pure: `parse_macho` maps
/// `__TEXT` from `fileoff == 0`, so the mach header is genuinely in guest memory and therefore in
/// the snapshot, and record and replay cannot disagree about bytes the trace must contain anyway.
///
/// FAILS LOUD and NEVER defaults. A silent PAC-off fallback is indistinguishable from correct for
/// every guest this repo can build today (all plain arm64), so it would hide a broken derivation at
/// exactly the moment an arm64e guest arrives.
fn pac_posture_from_memory(regions: &[Region]) -> bool {
    let hdr = regions.iter()
        .find(|r| r.ipa <= EXE_BASE && EXE_BASE + 12 <= r.ipa + r.bytes.len() as u64)
        .unwrap_or_else(|| panic!(
            "no snapshot region covers EXE_BASE {EXE_BASE:#x}; refusing to guess a PAC posture"));
    let o = (EXE_BASE - hdr.ipa) as usize;
    let magic = u32::from_le_bytes(hdr.bytes[o..o+4].try_into().unwrap());
    assert_eq!(magic, 0xfeed_facf,
        "no MH_MAGIC_64 at EXE_BASE {EXE_BASE:#x} (found {magic:#x}) — refusing to guess a PAC posture");
    pac_posture(u32::from_le_bytes(hdr.bytes[o+8..o+12].try_into().unwrap()))
}

/// Re-derive the guest's stack geometry `(top, size)` from a snapshot's own memory — the twin of
/// `pac_posture_from_memory`, and for the same reason: `restore()` gets only `(regions, regs)`.
///
/// `restore()` rebuilds BOTH load paths (a static `record` trace AND a dynamic `record-dyn` one),
/// and their stacks differ — one granule below `STACK_TOP_IPA` vs `DYN_STACK_SIZE` below
/// `DYN_STACK_TOP`. Hardcoding either here would make the other path lie on replay, which under
/// M8-stack is not a cosmetic wrong answer: the `kern.usrstack64` replay mirror recomputes its reply
/// from this geometry and byte-compares it against the recording, so a wrong path would surface as a
/// divergence. The two stack backings sit at disjoint IPAs, so the region list names the path
/// unambiguously.
///
/// FAILS LOUD and NEVER defaults, exactly like `pac_posture_from_memory`: guessing a stack top is
/// how the guest ends up believing a lie about its own address space in the first place.
fn stack_geometry_from_memory(regions: &[Region]) -> (u64, u64) {
    let covers = |base: u64, size: u64| regions.iter()
        .any(|r| r.ipa == base && r.bytes.len() as u64 >= size);
    let dynamic = covers(DYN_STACK_TOP - DYN_STACK_SIZE, DYN_STACK_SIZE);
    let static_ = covers(STACK_TOP_IPA - GRANULE as u64, GRANULE as u64);
    match (dynamic, static_) {
        (true, false) => (DYN_STACK_TOP, DYN_STACK_SIZE),
        (false, true) => (STACK_TOP_IPA, GRANULE as u64),
        _ => panic!("cannot identify the guest's stack in the snapshot \
                     (dynamic={dynamic}, static={static_}); refusing to guess a stack geometry"),
    }
}
// CPACR_EL1.FPEN = 0b11 (bits [21:20]): EL0 and EL1 may use FP/SIMD without trapping. dyld's
// early code uses NEON (memcpy, hashing); without this an FP access traps EC=0x07.
const CPACR_FP_ON: u64 = 0x3 << 20;

// Software single-step (M3). Both bits must be set together to arm one step, and cleared after,
// so run()/forward never step: MDSCR_EL1.SS enables the step state machine, PSTATE.SS makes the
// next instruction the one that completes before the step exception fires.
const PSTATE_SS: u64 = 1 << 21; // PSTATE/SPSR software-step bit
const MDSCR_SS:  u64 = 1 << 0;  // MDSCR_EL1.SS

// Hardware instruction breakpoints (M3 debugger `continue`/reverse scans). MDSCR_EL1.MDE gates the
// whole HW breakpoint/watchpoint machine; DBGBCRn = 0x1E5 arms slot n: E=1 (bit0), PMC=0b10 (EL0,
// bits1-2), BAS=0b1111 (bits5-8) — matched empirically in the sstep spike (F3).
const MDSCR_MDE: u64 = 1 << 15;
const DBGBCR_ARM: u64 = 0x1E5;
// The 6 (DBGBVRn, DBGBCRn) comparator pairs, indexed by slot.
const HW_BREAKPOINT_SLOTS: [(hv_sys::SysReg, hv_sys::SysReg); 6] = [
    (sysreg::DBGBVR0_EL1, sysreg::DBGBCR0_EL1),
    (sysreg::DBGBVR1_EL1, sysreg::DBGBCR1_EL1),
    (sysreg::DBGBVR2_EL1, sysreg::DBGBCR2_EL1),
    (sysreg::DBGBVR3_EL1, sysreg::DBGBCR3_EL1),
    (sysreg::DBGBVR4_EL1, sysreg::DBGBCR4_EL1),
    (sysreg::DBGBVR5_EL1, sysreg::DBGBCR5_EL1),
];

// Hardware write-watchpoints (M5 debugger `watch`). DBGWCR_BASE = E=1 (bit0) | PAC=0b10 EL0-only
// (bits2:1) | LSC=0b10 store-only (bits4:3); the per-watch BAS byte-select mask goes in bits 12:5.
// 4 comparator pairs on this silicon (hvprobe), vs 6 breakpoints.
const DBGWCR_BASE: u64 = 0x15;
const HW_WATCHPOINT_SLOTS: [(hv_sys::SysReg, hv_sys::SysReg); 4] = [
    (sysreg::DBGWVR0_EL1, sysreg::DBGWCR0_EL1),
    (sysreg::DBGWVR1_EL1, sysreg::DBGWCR1_EL1),
    (sysreg::DBGWVR2_EL1, sysreg::DBGWCR2_EL1),
    (sysreg::DBGWVR3_EL1, sysreg::DBGWCR3_EL1),
];

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
// in block 0's free area [TSD_end 0x34000, PT_L3_BASE 0x800000) — clear of trampoline (0x4000),
// PT_L2 (0x8000), stack (0x1C000), TSD (0x30000), dyn stack (40 MiB), PT_L3 (0x800000+), segments (>=4GiB),
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

// --- Guest TLBI oracle (M9 t2) ---
// The TLBI stub page. W^X: RO + EL1-exec (ATTR_TRAMP) — `tlbi` is an EL1 instruction, so this
// CANNOT share the sign stub's ATTR_CODE page (ATTR_CODE sets PXN: EL0-exec, EL1 no-exec).
const TLBI_STUB_IPA: u64 = 0x0004_8000; // 288 KiB: a fresh IPA the guest never translates

// --- The VMM scratch window (M13 t7) ---
// The three scratch IPAs above (sign stub, sign table, TLBI stub) are contiguous, and every one of
// them is mapped LAZILY — on the first sign/authenticate or the first flush. Until then they are
// absent from `backings`, so `range_is_free` saw them as free and an ANYWHERE placement could be
// handed an extent straddling them; the later lazy `hv_vm_map` then failed with HV_ERROR because the
// guest already owned the IPA. That is not hypothetical: libsystem's thread-stack
// `mach_vm_allocate(ANYWHERE, 0x408000)` lands at 0x38000 on the dynamic path and covers all three.
// It stayed latent only because nothing forced the TLBI stub to be created AFTER such an allocation
// until M13 made `mprotect(PROT_NONE)` flush for real.
//
// Reserving the window as a forbidden range keeps the stubs lazy while making their addresses truly
// theirs, and it is a pure function of constants — identical on record and replay, so first-fit's
// determinism argument is unchanged.
const SCRATCH_RESERVED_START: u64 = SIGN_STUB_IPA;
const SCRATCH_RESERVED_END:   u64 = TLBI_STUB_IPA + GRANULE as u64; // 0x4C000, exclusive
// Safety net for the stub's own run loop: a correct stub reaches its terminating `hvc` in ONE
// hv_vcpu_run, so any higher count means a bug.
const TLBI_STUB_BOUND: u32 = 4;
// The TLBI stub, hand-assembled. Encodings verified with `clang -arch arm64 -c` + `otool -t`
// (Task 1's spike, spikes/tlbi.c):
//   tlbi vmalle1 ; dsb ish ; isb ; hvc #0
// Runs at EL1 (ATTR_TRAMP is EL1-exec) and ends with `hvc #0`, which from EL1 traps DIRECTLY to
// EL2 — no trampoline indirection, unlike the sign stub's EL0 `svc`.
// VMALLE1 (not VMALLE1IS): single-vCPU, so there is no other PE whose TLB needs invalidating.
const TLBI_STUB: [u32; 4] = [0xd508_871f, 0xd503_3b9f, 0xd503_3fdf, 0xd400_0002];
// EL1h with DAIF masked — the exception level `tlbi` requires.
const TLBI_STUB_CPSR: u64 = 0x3C5;

// The full architectural state save_state/restore_state capture around an in-guest stub run, so a
// mid-run caller sees no disturbance — shared by both guest oracles: sign_slots's EL0 `svc` (the
// cache pager's caller) and flush_guest_tlb's EL1 `hvc` (M9 t2). The sign stub's `svc` overwrites
// ELR/SPSR/ESR/FAR_EL1; ELR_EL1 & SPSR_EL1 are load-bearing there (set_x0_and_return resumes EL0
// from them). The TLBI stub's `hvc` traps EL1->EL2 directly and does NOT touch these EL1 sysregs at
// all, but they are saved/restored anyway for one shared list and future-proofing.
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
// M13: EL0 gets nothing (AP 0b00 leaves EL1 RW and EL0 with no access at all), and the page is
// non-executable at both ELs. `A_COMMON` sets AF, so an EL0 access takes a clean PERMISSION fault
// (DFSC 0x0f) rather than an access-flag fault — which is what routes it through the EL1 trampoline
// to `Stop::Fault` and M12's disposition check. Marking the descriptor INVALID instead would give a
// translation fault, making a protected page indistinguishable from an unmapped one; that
// distinction is what lets `commit_reserved_page` keep its stage-2 cases (see the M13 spec).
const ATTR_NONE: u64 = A_COMMON | UXN | PXN; // AP 0b00 (no bits to OR in): EL1-RW, EL0 none
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
    // PROT_NONE address-space reservations as (start, len), recorded by guest_vm_reserve and
    // demand-committed page-by-page on first touch by commit_reserved_page (the moral twin of the
    // cache demand-pager). Reset to empty in restore() alongside `mmap_next` so replay's address
    // space matches record. Plain Vec (its Drop only frees its own buffer), declared after
    // `backings` so the load-bearing vcpu-before-vm drop order is unaffected.
    reservations: Vec<(u64, u64)>,
    // M13: page-granular extents the guest has protected PROT_NONE, recorded by `protect_none` and
    // subtracted by `unprotect`/`guest_munmap`. Same shape and same treatment as `reservations`
    // above — a pure function of the guest's own mmap/mprotect sequence, so identical on record and
    // replay. INVARIANT: every page named here is backed (see `protect_none`).
    noaccess: Vec<(u64, u64)>,
    // Next fresh IPA for guest_mmap. Plain u64 (no Drop), declared after `backings` so the
    // load-bearing vcpu-before-vm drop order is unaffected.
    mmap_next: u64,
    // M2-xpcport: the name of a real kernel-valid send right minted in retrace's OWN IPC space (==
    // the guest's, since Mach traps forward through), handed back as the task_get_special_port(
    // BOOTSTRAP) reply's port name so libxpc's mach_port_mod_refs(SEND,+1) succeeds. Nondeterministic
    // (kernel-assigned) → recorded and replayed like task_self, never regenerated on replay (restore
    // leaves it None). Minted once and cached (idempotent). Plain Option<u32> (no Drop), so the
    // load-bearing vcpu-before-vm drop order is unaffected; retrace holds the receive right for the
    // process lifetime (the name stays valid), so the port is deliberately never deallocated.
    bootstrap_port: Option<u32>,
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
    // Re-fault loop guard (page_in_cache): the last already-mapped cache IPA that re-faulted and how
    // many times in a row, so an unfixable re-fault panics loudly instead of hanging the VM. Plain
    // u64s; declared before `cache`, so the vcpu-before-vm drop order is unaffected.
    cache_refault_ipa: u64,
    cache_refault_count: u64,
    // The dyld-shared-cache demand-pager's routing table (built by install_cache_pager on the #536
    // cache-mapping syscall; None until then). Holds the parsed subcache headers + open file
    // handles that page_in_cache reads pristine pages from. Its `File`s have Drop, but it is
    // declared after vcpu/vm, so the load-bearing vcpu-before-vm drop order is unaffected.
    cache: Option<CacheMeta>,
    // M5 watchpoint state. NOT captured in BoxState: checkpoints are only ever taken via
    // checkpointed_seek on freshly-seeked (unarmed) sessions, so armed state never needs to persist.
    bps_armed: bool,
    wps_armed: bool,
    watch_ranges: Vec<(u64, u64)>, // (va, len) armed write-watch ranges, for the software (syscall) check
    syscall_watch_hit: Option<(u64, u64)>, // (watched_va, write_ipa): first overlap this event
    // M7 t6: this guest's derived (or explicitly overridden) PAC posture. Plain bool (no Drop),
    // appended last so the load-bearing vcpu-before-vm drop order is unaffected. Cross-checked
    // against the live SCTLR_EL1 bits by dbg_pac_enabled().
    pac_enabled: bool,
    // M8-stack: the guest's OWN stack geometry, set at load. `kern.usrstack64` (and, from t4,
    // RLIMIT_STACK) are answered from these rather than forwarded, so the guest is never told its
    // stack lives at a HOST address. Path-aware by construction: the static path maps one granule
    // below STACK_TOP_IPA, the dynamic path maps DYN_STACK_SIZE below DYN_STACK_TOP — hardcoding
    // either constant at the answer site would make the other path lie. Plain u64s (no Drop),
    // appended last so the load-bearing vcpu-before-vm drop order is unaffected.
    stack_top: u64,
    stack_size: u64,
    // M9 t2: has ensure_tlbi_stub already mapped + promoted the TLBI stub page? Plain bool (no
    // Drop), appended last so the load-bearing vcpu-before-vm drop order is unaffected. Unlike the
    // sign stub's lazy-init check (a stage-2-backing lookup at a fixed IPA), this is a plain flag —
    // either works, but a flag makes ensure_tlbi_stub's early-return trivial to read.
    tlbi_stub_ready: bool,
    // M10: the guest's file-descriptor table. Before this existed the guest's fds WERE retrace's —
    // forward_and_diff issues a raw svc in retrace's own process, so a guest open returned a host fd
    // and a guest close(n) closed retrace's n. Carried in BoxState (see its field comment): a mid-run
    // capture cannot re-derive it. Has Drop (Vecs), but is declared after vcpu/vm, so the
    // load-bearing vcpu-before-vm drop order is unaffected.
    fds: FdTable,
    // M11: the guest's signal dispositions. Pure guest state — a function of the guest's own
    // sigaction/sigprocmask/sigaltstack calls, identical on record and replay, so it never enters
    // the trace (see `sig.rs`). Carried in BoxState for the same reason the fd slots are: a mid-run
    // capture cannot re-derive it. Has Drop (an array of Copy + an Option), declared after vcpu/vm,
    // so the load-bearing vcpu-before-vm drop order is unaffected.
    sigtable: SigTable,
    // M14: the guest's threads. A single-threaded guest has one entry and takes the pre-M14 path.
    // Declared after vcpu/vm, so the load-bearing vcpu-before-vm drop order is unaffected.
    threads: thread::ThreadTable,
    // M14 t7: the address the kernel enters a NEW thread at, learned from the guest's own
    // `bsdthread_register` call (x0) — every dynamic guest issues one at startup (measured, Task 2),
    // so `guest_bsdthread_create` never has to guess it. `None` until that call is observed.
    // Carried in BoxState (see its field comment): a mid-run capture cannot re-derive it, since the
    // registering syscall sits behind the checkpoint. Plain `Option<u64>` (no Drop), declared after
    // vcpu/vm, so the load-bearing vcpu-before-vm drop order is unaffected.
    thread_start_pc: Option<u64>,
    /// M18: the workqueue thread entry point from `bsdthread_register`'s `args[1]`. The address the
    /// kernel enters when it hands a worker thread to userspace. Captured in Stage 1, entered in
    /// Stage 2.
    wq_thread_pc: Option<u64>,
    /// M18: the guest's pthread struct size from `bsdthread_register`'s `args[2]`. The kernel — not
    /// the guest — allocates a workqueue thread's pthread struct, so Stage 2 needs this size.
    pthread_size: Option<u32>,
}

/// Byte offset of the thread's mach port name (the "kport") inside libpthread's `pthread` struct,
/// on macOS 26. **The kernel writes this during `bsdthread_create`, and `pthread_join` is unusable
/// without it** — see `Box_::guest_bsdthread_create` for the disassembly and the host probe.
const PTHREAD_KPORT_OFF: u64 = 0xf8;

/// Byte offset of the thread-specific-data base inside libpthread's `pthread` struct. The kernel
/// sets `TPIDRRO_EL0 = pthread + 0xe0` when it starts a thread; libpthread reads it back the other
/// way (`__pthread_join`: `mrs x23, TPIDRRO_EL0` / `sub x21, x23, #0xe0`). MEASURED on the live
/// host, 4/4 for main and child alike — see `Box_::guest_bsdthread_create`.
const PTHREAD_TSD_OFF: u64 = 0xe0;

/// `PTHREAD_START_TSD_BASE_SET` — the bit the kernel ORs into the flags word a new thread receives
/// in `w5`, asserting that it set that thread's TSD base. `__pthread_start`'s second instruction is
/// `tbz w5, #0x1c, ...` straight into `brk #0xb001` with the message "BUG IN LIBPTHREAD:
/// thread_set_tsd_base() wasn't called by the kernel", so a thread started without it dies
/// immediately. ORed onto the guest's own flags, never substituted for them.
const PTHREAD_START_TSD_BASE_SET: u64 = 0x1000_0000;

/// Base for the synthetic mach port names retrace hands guest threads, in the same obviously-fake
/// `0x0BAD….` shape as `SYNTHETIC_BOOTSTRAP_PORT`. The name is `BASE | tid`, so it is a pure
/// function of the guest's own syscall sequence: non-zero (what `join` actually tests), distinct per
/// thread, and identical on record and replay with nothing recorded.
///
/// A real kport would be a host-allocated name and therefore nondeterministic — the same reason
/// M2-xpcport had to take a deliberate record/replay asymmetry for its minted bootstrap port. This
/// needs no such exception: nothing outside the guest ever dereferences this name. libpthread uses
/// it only as the `__ulock_wait` comparison value at `pthread+0x34` and passes it back verbatim in
/// `bsdthread_terminate`'s `port` argument, both of which stay inside the box.
const GUEST_THREAD_PORT_BASE: u32 = 0x0BAD_7000;

/// The stack retrace gives a workqueue worker, in bytes. **This number is retrace's own choice**,
/// and uniquely among the constants around `guest_workq_reqthreads` it is not a measurement:
/// M18 Stage 2b Task 1 §2c states it outright — "the size is retrace's choice — nothing in the
/// entry path requires a particular value." The host was observed handing a worker `0x83000`
/// (524 KiB, and NOT page-aligned); this is the nearest round page-aligned size, which lets the
/// pthread struct sit on a page boundary. The floor is not zero: `_thread_chkstk_darwin` compares
/// `sp - framesize` against `[pthread+0xb8]` (= the `x2` this size derives), so a too-small stack
/// fails there rather than at entry.
const WQ_WORKER_STACK_SIZE: u64 = 0x8_0000;

/// The `x4` flags word retrace hands a **fresh** workqueue worker, minus its low-byte QoS index —
/// the whole word is `WQ_ENTRY_FLAGS_FRESH | qos_index`. Task 1 §1d decoded every bit from
/// `__pthread_wqthread`'s own `tbnz`/`tbz`; §5 item 4 is the form:
///
/// - **bit 21** (`0x200000`) — "the kernel called `thread_set_tsd_base()`". REQUIRED: clear takes
///   `0x3048 tbz w3, #0x15` into `brk #0xb001`. It is an *assertion*, tested against the flag and
///   never against the register, so setting it obliges `TPIDRRO_EL0` to be right — §2c: bit 21 set
///   with the register wrong is **silent** corruption with no `brk` to hunt for.
/// - **bit 18** (`0x40000`) — set in 13/13 observed entries and read by nothing on this path.
///   Carried to match the kernel; Task 1 records that either choice is untested.
/// - **bit 14** (`0x4000`) — "the low byte is a QoS index". REQUIRED with bit 15: both clear reaches
///   `"BUG IN LIBPTHREAD: Missing priority"` + `brk #0xb001`.
///
/// Two bits are deliberately CLEAR and are the reason this is a named constant rather than a
/// literal at the call site:
/// - **bit 17** (`0x20000`) selects *reused* over *fresh*. Clear is what makes libpthread call
///   `__pthread_wqthread_setup` and write its own struct — including its own `pacdb` signature —
///   which is what keeps M14's "retrace invents an address, not a layout" rule intact here.
/// - **bit 23** (`0x800000`) forces the priority to the constant `0x040008ff`, which is exactly the
///   value the guest's own REQTHREADS request carries and therefore looks like the right bit. It is
///   a **different thread role**: it also writes `pthread+0xa4 = 1` and bypasses the QoS arithmetic
///   entirely. Task 1 §5 item 3 names setting it as the precise mistake to avoid.
const WQ_ENTRY_FLAGS_FRESH: u64 = 0x24_4000;

/// Ceiling on the `numthreads` a single `REQTHREADS` may ask for. **Not a measurement** — like
/// `WQ_WORKER_STACK_SIZE` this is retrace's own number, and it exists only so that a garbage
/// `args[2]` becomes a named refusal instead of `N` × half a megabyte of silently reserved IPA
/// space. Every observed request asks for exactly `1` (M18 Task 6 / Stage 2a t10), and libdispatch
/// asks incrementally, so a request anywhere near this bound means the box's reading of `args[2]`
/// is wrong rather than that the guest wants that many workers — which is precisely what a
/// fail-loud ceiling is for. Raise it with a measurement, not with a guess.
const WQ_MAX_WORKERS_PER_REQUEST: u64 = 64;

#[derive(Debug)]
pub enum Stop { Syscall { num: u64, args: [u64;8] }, Fault { pc: u64, esr: u64, far: u64 }, Other { esr: u64 }, Step }

/// A complete, in-memory-only capture of `Box_`'s internal state at an ARBITRARY position — unlike
/// `Event::Snapshot` (the trace format), which is only correct to restore from at landmark 0.
/// Never persisted, never enters a trace file. See the M4 design spec for why each field is here.
/// `EBADF` — answered for a guest fd that is `Free` or `Closed`, with nothing forwarded.
pub const EBADF: u64 = 9;

/// One entry in the guest's descriptor space.
///
/// `Closed` is deliberately distinct from `Free`: both answer `EBADF`, but only `Free` is reusable
/// by `alloc`, and a checkpoint restore must be able to tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdSlot { Free, Open, Closed }

/// The guest's file-descriptor table.
///
/// **Split by design.** `slots` is guest-visible state: a pure function of the guest's own
/// open/dup/close sequence, identical on record and replay, and the sole authority on `EBADF`.
/// `host` is the record-only `guest_fd -> host_fd` map — replay executes no syscall and opens no host
/// fd, so it has nothing to map, and host fd numbers therefore never enter the trace. That matters:
/// a host fd is a function of how many files RETRACE happens to hold open, so recording one would
/// make the trace depend on the recorder rather than on the guest.
///
/// Before M10 the guest's fds simply WERE retrace's. `forward_and_diff` issues a raw `svc` in
/// retrace's own process, so a guest `open` returned a host fd — measured: `jq` saw 17-22, because
/// retrace holds 0-16 open — and a guest `close(n)` closed retrace's `n`. M9 fixed that for fd 0/1/2
/// by special case; this table fixes it for the rest.
#[derive(Debug, Clone, Default)]
pub struct FdTable {
    slots: Vec<FdSlot>,
    host: Vec<Option<i32>>,
}

impl FdTable {
    /// Fresh table with 0/1/2 open as the console, mapped **identically onto retrace's own 0/1/2**.
    ///
    /// The identity mapping is load-bearing and was not obvious. M9 intercepts console *writes*
    /// (mirrored into the trace) and console *closes* (faked) before forwarding is ever considered —
    /// but that is all it intercepts. Everything else libc does to fd 0/1/2 still forwards, and
    /// stdio does a lot of it: `fstat(1)` to pick a buffering mode, `ioctl(1)`/`fcntl(1)` to ask
    /// whether stdout is a tty. Leaving these unmapped answered EBADF for every one of them, which
    /// crashed `watch_dyn`'s guest — the identity mapping restores exactly the pre-M10 behaviour for
    /// the operations M9 does not intercept, while the dangerous two stay intercepted upstream.
    pub fn new() -> FdTable {
        FdTable { slots: vec![FdSlot::Open; 3], host: vec![Some(0), Some(1), Some(2)] }
    }

    fn grow_to(&mut self, gfd: usize) {
        if self.slots.len() <= gfd {
            self.slots.resize(gfd + 1, FdSlot::Free);
            self.host.resize(gfd + 1, None);
        }
    }

    /// Lowest descriptor >= 3 that is not currently open, POSIX-style. Deterministic, which is the
    /// whole point: it makes a recorded guest fd a function of the guest rather than of retrace's
    /// own open files.
    ///
    /// `Closed` slots are reusable — POSIX returns the lowest fd *not currently open*, so a number
    /// the guest just closed is handed straight back on the next `open`. `Closed` is still distinct
    /// from `Free` (a checkpoint restore must tell "the guest closed this" from "never used"), but
    /// the distinction does not gate reuse.
    pub fn alloc(&mut self) -> u64 {
        let gfd = (3..self.slots.len())
            .find(|&i| self.slots[i] != FdSlot::Open)
            .unwrap_or_else(|| self.slots.len().max(3));
        self.grow_to(gfd);
        self.slots[gfd] = FdSlot::Open;
        gfd as u64
    }

    pub fn bind(&mut self, gfd: u64, host_fd: i32) {
        self.grow_to(gfd as usize);
        self.host[gfd as usize] = Some(host_fd);
    }

    pub fn host(&self, gfd: u64) -> Option<i32> {
        self.host.get(gfd as usize).copied().flatten()
    }

    pub fn is_open(&self, gfd: u64) -> bool {
        self.slots.get(gfd as usize) == Some(&FdSlot::Open)
    }

    /// Mark closed and drop the host mapping. `false` means the guest closed something it did not
    /// have open — the caller answers `EBADF` and forwards nothing.
    pub fn close(&mut self, gfd: u64) -> bool {
        if !self.is_open(gfd) { return false; }
        self.slots[gfd as usize] = FdSlot::Closed;
        self.host[gfd as usize] = None;
        true
    }

    pub fn slots(&self) -> Vec<FdSlot> { self.slots.clone() }

    /// Rebuild guest-visible state — used by `from_checkpoint` and by replay.
    ///
    /// Deliberately carries no host mapping for guest-opened fds: those are record-only, and a
    /// restored box has none (replay executes no syscall). The console identity mapping IS rebuilt
    /// for whichever of 0/1/2 are still open, because it is a **constant**, not captured state —
    /// the M9 t3 lesson in the other direction: carry what cannot be derived, derive what can.
    /// Without this a restored box answers EBADF to `fstat(1)`, the same defect that crashed
    /// `watch_dyn` before the identity mapping existed.
    pub fn from_slots(slots: &[FdSlot]) -> FdTable {
        let mut host = vec![None; slots.len()];
        for (gfd, h) in host.iter_mut().enumerate().take(3) {
            if slots[gfd] == FdSlot::Open { *h = Some(gfd as i32); }
        }
        FdTable { slots: slots.to_vec(), host }
    }
}

#[derive(Clone)]
pub struct BoxState {
    pub regs: Regs,
    pub fp: [u128; 32],
    pub fpcr: u64,
    pub fpsr: u64,
    pub tpidr_el0: u64,
    // EL1 exception-return pair (ELR_EL1/SPSR_EL1). Live at a syscall-trap checkpoint — `position()`
    // reports ELR_EL1 and `set_x0_and_return` resumes off both — so a COMPLETE mid-run capture must
    // carry them (a fresh vcpu does not reproduce them, unlike landmark-0 where they are dead/0).
    pub elr: u64,
    pub spsr: u64,
    pub mem: Vec<Region>,
    pub reservations: Vec<(u64, u64)>,
    // M13: carried for the same reason as `reservations` — a mid-run capture cannot re-derive it.
    // The page-table STAMPS ride along inside `mem` (the L2/L3 tables are backings, exactly as M9's
    // ATTR_TRAMP note at from_checkpoint explains), but the MAP that `unprotect` and the fail-loud
    // asserts consult is box state, and a restore without it would leave the two disagreeing.
    pub noaccess: Vec<(u64, u64)>,
    // M14: carried for the same reason as `reservations` and `noaccess` — a mid-run capture cannot
    // re-derive it. Note this SUPERSEDES the struct's flat register fields as the authority for
    // non-current threads: `regs`/`elr`/`spsr` still describe the RUNNING thread (so every M0–M13
    // consumer is unchanged), while `threads` carries all of them including that one.
    pub threads: thread::ThreadTable,
    // M14 t7 fix round 1: carried for the same reason as `threads` above — a mid-run capture cannot
    // re-derive it. The guest's OWN `bsdthread_register` call — the only place this is learned —
    // sits BEHIND the checkpoint the moment one is taken after it, so a restored session replaying
    // FORWARD can never see that call again; without this field, a `bsdthread_create` on the
    // restored session hits the same fail-loud `.expect()` an unregistered guest hits, even though
    // this guest already registered before the capture.
    pub thread_start_pc: Option<u64>,
    // M18: carried for the same reason as `thread_start_pc` immediately above — the guest's own
    // `bsdthread_register` call sits behind the checkpoint the moment one is taken after it, so a
    // restored session replaying forward can never observe it again.
    pub wq_thread_pc: Option<u64>,
    pub pthread_size: Option<u32>,
    pub mmap_next: u64,
    pub bootstrap_port: Option<u32>,
    pub cache_installed: bool,
    pub last_far: u64,
    pub synthetic_tsc: u64,
    pub cache_refault_ipa: u64,
    pub cache_refault_count: u64,
    // M7 t6: captured because from_checkpoint()'s snapshot is mid-run — its header is NOT pristine
    // by construction, unlike restore()'s landmark-0 snapshot — so the posture cannot be re-derived
    // and must be carried instead.
    pub pac_enabled: bool,
    // M8-stack: carried for the same reason as `pac_enabled` — a mid-run capture must reproduce the
    // geometry exactly, and a seeked session must answer `kern.usrstack64` the way the record run did.
    pub stack_top: u64,
    pub stack_size: u64,
    // M10: carried for the same reason as `pac_enabled` and `stack_top` — a mid-run capture cannot
    // re-derive it. GUEST-VISIBLE slots only; the host map is record-only and a restored box has no
    // host fds to map (from_checkpoint is a replay-side operation, and replay executes no syscall).
    //
    // Defaulting this to a fresh table would make a seeked session believe every fd is Free, so a
    // post-seek guest pread returns EBADF and reverse execution silently diverges from the forward
    // run. That is the M9 t3 failure shape — from_checkpoint resetting a flag the restored state
    // contradicts — and this is the third field in this struct to exist for that reason.
    pub fd_slots: Vec<FdSlot>,
    // M11: carried for the same reason as `pac_enabled`, `stack_top`, and the fd slots — a mid-run
    // capture cannot re-derive it. Without this, a seek into a run that installed a disposition
    // restores a box that has forgotten it, and the next raise takes the wrong branch: an IGNORED
    // signal would terminate the guest. That divergence would read as a signal bug and actually be
    // a checkpoint bug. The fourth field in this struct to exist for exactly this reason.
    pub sigtable: SigTable,
}

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

// --- M2-xpcport: mint a real bootstrap send right in retrace's own IPC space ---
// mach_port_options_t (<mach/port.h>): { uint32_t flags; mach_port_limits_t mpl; uint64_t reserved[2] }
// where mach_port_limits_t = { mach_port_msgcount_t mpl_qlimit } (one u32). 24 bytes, repr(C).
#[repr(C)]
struct MachPortOptions { flags: u32, mpl_qlimit: u32, reserved: [u64; 2] }
const MPO_INSERT_SEND_RIGHT: u32 = 0x10; // <mach/port.h>
extern "C" {
    static mach_task_self_: u32; // mach_task_self() is a C macro for this global
    // kern_return_t mach_port_construct(ipc_space_t, mach_port_options_t*, mach_port_context_t, mach_port_name_t*)
    fn mach_port_construct(task: u32, options: *const MachPortOptions, context: u64, name: *mut u32) -> i32;
}

/// Subtract `[addr, addr+len)` from every overlapping extent in a page-granular `(start, len)`
/// table, rounding the cut OUT to whole pages the way the kernel does (start down, end up). A full
/// cover removes the entry; a head/tail overlap trims it; a strictly-interior punch SPLITS it into
/// two. Pure: identical input produces identical output on record and replay.
///
/// Shared by `reservations` (M2-carveout's hole punch) and `noaccess` (M13's protection map) so the
/// second table cannot drift from the arithmetic `carveout.rs` already pins for the first.
pub(crate) fn subtract_range(table: &mut Vec<(u64, u64)>, addr: u64, len: u64) {
    let g = GRANULE as u64;
    let s = addr & !(g - 1);                                 // trunc_page(addr)
    let e = (addr.saturating_add(len) + g - 1) & !(g - 1);   // round_page(addr + len)
    if e <= s { return; }
    let mut out = Vec::with_capacity(table.len() + 1);
    for &(start, rlen) in table.iter() {
        let end = start + rlen;
        if e <= start || s >= end {
            out.push((start, rlen));                         // disjoint: keep whole
            continue;
        }
        if s > start { out.push((start, s - start)); }       // head remnant below the cut
        if e < end   { out.push((e, end - e)); }             // tail remnant above the cut
        // [s, e) fully covers [start, end): push nothing (entry removed)
    }
    *table = out;
}

/// Test-only re-export of [`subtract_range`], so the integration test can exercise the shared
/// arithmetic directly without making it part of the crate's public surface.
#[doc(hidden)]
pub fn subtract_range_for_test(table: &mut Vec<(u64, u64)>, addr: u64, len: u64) {
    subtract_range(table, addr, len)
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
    // file-backed (SPTM). Returns (ttbr0 = PT_L1_IPA, l2_host, next_l3) so runtime promotion can
    // edit the live L2 and continue the same L3 allocation window.
    fn build_tables(backings: &mut Vec<Backing>, exec: &[(u64, u64, u64)]) -> (u64, *mut u8, u64) {
        assert!(exec.iter().all(|&(_, len, _)| len > 0), "exec ranges must be non-empty");
        let (l2_host, l2_len) = alloc_pages(GRANULE);
        let l2 = unsafe { std::slice::from_raw_parts_mut(l2_host as *mut u64, 2048) };
        for (i, e) in l2.iter_mut().enumerate() { *e = ((i as u64) * BLK) | ATTR_DATA | DESC_BLOCK; }
        backings.push(Backing { host: l2_host, ipa: PT_L2_IPA, len: l2_len });

        // 47-bit VA (T0SZ=17) is a 3-level walk: TTBR0 -> L1 -> L2 -> L3. Every mapped IPA is
        // < 2^36 (one L1 entry spans 64 GiB), so only L1[0] is valid; it points at the L2 above.
        // The wider VA moves the hardware PAC signature to bits [54:47], above objc's 47-bit
        // ISA_MASK, so a plain-arm64 isa strip is lossless. alloc_pages returns zeroed anon pages,
        // so L1[1..] are already invalid (a VA >= 2^36 faults).
        let (l1_host, l1_len) = alloc_pages(GRANULE);
        let l1 = unsafe { std::slice::from_raw_parts_mut(l1_host as *mut u64, 2048) };
        l1[0] = PT_L2_IPA | DESC_TABLE;
        backings.push(Backing { host: l1_host, ipa: PT_L1_IPA, len: l1_len });

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
        (PT_L1_IPA, l2_host, next_l3)
    }

    /// Runtime exec-mmap promotion: install RO+exec (`ATTR_CODE`) stage-1 pages for [ipa, ipa+len)
    /// by editing the LIVE page tables, so a `PROT_EXEC` mmap becomes executable under W^X. No TLB
    /// invalidation: mmap regions are freshly-mapped IPAs the guest has never translated before, so
    /// the first access does a fresh walk and sees ATTR_CODE. See
    /// [`set_region_attr`](Self::set_region_attr) for the parameterised form (M9 t2) that
    /// this now calls — `ATTR_CODE` (EL0-exec) is the right attribute for a guest code page, but the
    /// TLBI stub needs `ATTR_TRAMP` (EL1-exec) instead.
    pub fn set_region_exec(&mut self, ipa: u64, len: u64) {
        self.set_region_attr(ipa, len, ATTR_CODE);
    }

    /// The single stamp implementation shared by `set_region_exec` (`ATTR_CODE`, guest code / cache
    /// text), `ensure_tlbi_stub` (`ATTR_TRAMP`, the M9 TLBI stub — an EL1-exec page), and
    /// `protect_none`/`unprotect` (M13, `ATTR_NONE`/`ATTR_DATA` — NOT exec attributes, which is why
    /// this is no longer named for exec). Edits the LIVE page tables to install `attr` for
    /// [ipa, ipa+len): any newly-needed L3 tables are
    /// anon-allocated (SPTM: never file-backed), stage-2-mapped immediately (the walker must reach
    /// them) AND tracked as backings. No TLB invalidation here — callers on a block the guest MAY
    /// already have translated (M9 t3's live-backing case) are responsible for their own
    /// [`flush_guest_tlb`](Self::flush_guest_tlb) after promoting.
    pub fn set_region_attr(&mut self, ipa: u64, len: u64, attr: u64) {
        let l2_host = self.l2_host;
        assert!(!l2_host.is_null(), "set_region_attr: no live L2 table (restore had no PT_L2 region)");
        let l2 = unsafe { std::slice::from_raw_parts_mut(l2_host as *mut u64, 2048) };
        let mut next_l3 = self.next_l3;
        let created = {
            let mut alloc_l3 = || {
                assert!(next_l3 + GRANULE as u64 <= PT_L3_CEIL, "set_region_attr: too many exec blocks; L3 window exhausted");
                let (h, _) = alloc_pages(GRANULE);
                let a = next_l3; next_l3 += GRANULE as u64; (a, h)
            };
            Self::promote_and_set(l2, &self.backings, ipa, len, attr, &mut alloc_l3)
        };
        self.next_l3 = next_l3;
        // Register each new L3: stage-2-map it (freshly, so the walker reaches it) then track it.
        for bk in created {
            self.vm.map(bk.host, bk.ipa, bk.len, MemFlags::RWX).expect("hv_vm_map (set_region_attr l3)");
            self.backings.push(bk);
        }
    }

    fn set_pac_keys(vcpu: &Vcpu) {
        for (r, v) in PAC_KEYS { vcpu.set_sys(r, v).unwrap(); }
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

    /// Emulate a trapped Apple IMPDEF `MRS` that surfaces as an UNDEFINED instruction (EC=0x00,
    /// not the EC=0x18 sysreg-trap path) because HVF does not expose the register to the guest.
    /// Reads the instruction at ELR_EL1; if it is `MRS Xt, S3_6_C15_C1_5` (an Apple CPU
    /// feature/config register libdyld probes), writes a deterministic 0 into Xt (the guest tests a
    /// bit and, seeing it clear, takes its normal PAC-authenticated path) and skips the instruction.
    /// Returns true iff emulated. Deterministic and identical on record & replay (both re-execute
    /// the same probe), so no clock/CPU state leaks into memory. Any other undefined instruction
    /// returns false → surfaced as `Stop::Other` for diagnosis.
    fn try_emulate_undef_mrs(&mut self) -> bool {
        let elr = self.vcpu.get_sys(sysreg::ELR_EL1).unwrap();
        if self.host_span(elr).is_none() { return false; }
        let insn = u32::from_le_bytes(self.read_guest(elr, 4).try_into().unwrap());
        // MRS: bits[31:20] == 0xD53. Decode the sysreg selector (op0,op1,CRn,CRm,op2).
        if insn & 0xFFF0_0000 != 0xD530_0000 { return false; }
        let o0  = (insn >> 19) & 1;         // op0 = 0b10 | o0  => 2 or 3
        let op1 = (insn >> 16) & 0x7;
        let crn = (insn >> 12) & 0xf;
        let crm = (insn >> 8) & 0xf;
        let op2 = (insn >> 5) & 0x7;
        let rt  = insn & 0x1f;
        // S3_6_C15_C1_5: op0=3 (o0=1), op1=6, CRn=15, CRm=1, op2=5.
        let is_apple_feat = o0 == 1 && op1 == 6 && crn == 15 && crm == 1 && op2 == 5;
        if !is_apple_feat { return false; }
        if rt != 31 { self.vcpu.set_reg(reg::x(rt), 0).unwrap(); } // x31 = XZR: value discarded
        let spsr = self.vcpu.get_sys(sysreg::SPSR_EL1).unwrap();
        self.vcpu.set_reg(reg::PC, elr + 4).unwrap();
        self.vcpu.set_reg(reg::CPSR, spsr).unwrap();
        true
    }

    /// Emulate a B-family pointer authentication that FEAT_FPAC-faulted (ESR EC=0x1C). objc
    /// authenticates arm64e shared-cache pointers with the DATA-B/INSTR-B keys, but those cache
    /// pointers carry their host-process signature while the guest uses fixed guest keys, so
    /// `autdb`/`autib` fault; retrace's A-family cache re-signer can't reach them (v5 slide-info
    /// encodes only IA/DA). We trust the cache and emulate a *successful* authenticate: strip the
    /// aut*'s destination register to its canonical VA (the 47-bit strip proven by strip47) and
    /// skip the instruction. A pure function of the faulting register (deterministic) and below
    /// the record/replay layer, so both sides strip identically — nothing enters the trace.
    /// Returns false (→ Stop::Other) for any non-AUT-with-Rd instruction, so a combined
    /// auth+branch/load form fails loud rather than being silently mis-emulated.
    fn try_emulate_fpac_auth(&mut self) -> bool {
        let elr = self.vcpu.get_sys(sysreg::ELR_EL1).unwrap();
        if self.host_span(elr).is_none() { return false; }
        let insn = u32::from_le_bytes(self.read_guest(elr, 4).try_into().unwrap());
        let Some(rd) = retrace_arch::decode_aut_rd(insn) else { return false; };
        if rd != 31 {                                    // x31 = XZR: nothing to strip/write
            let signed = self.vcpu.get_reg(reg::x(rd)).unwrap();
            self.vcpu.set_reg(reg::x(rd), signed & 0x0000_7FFF_FFFF_FFFF).unwrap();
        }
        let spsr = self.vcpu.get_sys(sysreg::SPSR_EL1).unwrap();
        self.vcpu.set_reg(reg::PC, elr + 4).unwrap();
        self.vcpu.set_reg(reg::CPSR, spsr).unwrap();
        true
    }

    /// Load a static guest with the posture the real OS would give it (derived from the main
    /// executable's `cpusubtype`).
    pub fn load(loaded: &Loaded) -> Box_ { Self::load_with_pac(loaded, pac_posture(loaded.cpusubtype)) }

    /// `load` with an EXPLICIT PAC posture. **Test-only override.** Every guest this repo can build
    /// is plain arm64, so the PAC tests (`pac`, `sign_oracle`, `cache_pager`) would otherwise have
    /// no PAC-on box to assert against. NEVER use the override on a path whose trace is later
    /// replayed: replay re-derives the posture from the header, and an overridden record run against
    /// a derived replay run is a posture mismatch — which fails LATE (a divergence at the final
    /// memory compare), not loudly.
    pub fn load_with_pac(loaded: &Loaded, pac_enabled: bool) -> Box_ {
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
        let pac = pac_enabled;
        vcpu.set_sys(sysreg::SCTLR_EL1, sctlr_mmu_on(pac)).unwrap();   // was 0x30d00800 (MMU off)
        vcpu.set_sys(sysreg::VBAR_EL1, TRAMPOLINE_IPA).unwrap();
        // FPEN=0b11, matching load_dynamic/restore/from_checkpoint. Without it a STATIC guest
        // cannot execute FP/SIMD at all: the first NEON instruction takes EC=0x07 and the
        // recording dies with "non-syscall exit". This was an asymmetry, not merely a gap —
        // `restore` (which every REPLAY goes through) has always set it, so a static guest using
        // vector registers would fail to RECORD while replaying fine. No static fixture used a
        // vector register until M12's vecsurvive, which is why nothing caught it before.
        vcpu.set_sys(sysreg::CPACR_EL1, CPACR_FP_ON).unwrap();
        vcpu.set_trap_debug_exceptions(true).unwrap();          // route SS/breakpoint exits to the VMM (Box_::step)
        vcpu.set_sys(sysreg::SP_EL0, STACK_TOP_IPA).unwrap();
        vcpu.set_reg(reg::CPSR, 0x0).unwrap();                  // EL0t
        vcpu.set_reg(reg::PC, loaded.entry).unwrap();
        Box_ { vm, vcpu, backings, reservations: Vec::new(), noaccess: Vec::new(), mmap_next: MMAP_BASE, bootstrap_port: None, l2_host, next_l3, last_far: 0, synthetic_tsc: SYNTH_TSC_START, cache_refault_ipa: 0, cache_refault_count: 0, cache: None, bps_armed: false, wps_armed: false, watch_ranges: Vec::new(), syscall_watch_hit: None, pac_enabled: pac, stack_top: STACK_TOP_IPA, stack_size: GRANULE as u64, tlbi_stub_ready: false, fds: FdTable::new(), sigtable: SigTable::default(), threads: thread::ThreadTable::new(thread::ThreadCtx::zeroed()), thread_start_pc: None, wq_thread_pc: None, pthread_size: None }
    }

    pub fn sp(&self) -> u64 { self.vcpu.get_sys(sysreg::SP_EL0).unwrap() }

    /// Top of the guest's stack (exclusive) — what `kern.usrstack64` must report (M8-stack).
    pub fn stack_top(&self) -> u64 { self.stack_top }
    /// Size of the guest's stack in bytes — what `RLIMIT_STACK` must report (M8-stack).
    pub fn stack_size(&self) -> u64 { self.stack_size }

    /// `[start, end)` of the stack the guest BELIEVES it has but retrace does not back — reserved at
    /// load, grown page-by-page by `commit_reserved_page`. Excludes libstd's guard page by one granule
    /// at the bottom, and the really-backed stack at the top.
    pub fn believed_stack_window(&self) -> (u64, u64) { (GUARD_TOP, DYN_STACK_BOTTOM) }

    /// Re-sign a batch of shared-cache auth slots with the GUEST's fixed PAC keys, returning the
    /// signed pointers (in slot order). Each slot is signed in-guest with `pacia` (IA,
    /// `key_is_data == false`) or `pacda` (DA, `key_is_data == true`) — the guest's own keys sign by
    /// definition, so the result authenticates under the same keys the guest will use to load the
    /// cache. Does NOT disturb the caller's guest state: the stub runs on a dedicated scratch region
    /// and the full architectural state is saved and restored around it (see `run_pac_batch`).
    pub fn sign_slots(&mut self, slots: &[AuthSlot]) -> Vec<u64> {
        // PAC-off guest: the real OS gives a non-arm64e process a REBASE-ONLY cache — its `braa`s
        // are plain `br`s over raw pointers. `pacia`/`pacda` on the stub would NOP to the same
        // result, but making this an INTENDED mode rather than an accident is the point (and it
        // saves a vCPU round-trip per cache data page).
        if !self.pac_enabled { return slots.iter().map(|s| s.target_va).collect(); }
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

    /// Install the dyld-shared-cache demand-pager: parse the system cache (`DEFAULT_CACHE_PATH`) +
    /// its subcaches into an IPA routing table and keep the subcache files open for
    /// [`page_in_cache`](Self::page_in_cache) to `pread` pristine pages from. Called by the
    /// record/replay dispatch on the kernel cache-mapping syscall (#536) — on BOTH record and
    /// replay, so both sides page identical bytes. Idempotent (a second #536 is a no-op).
    pub fn install_cache_pager(&mut self) {
        if self.cache.is_none() {
            self.cache = Some(CacheMeta::load(DEFAULT_CACHE_PATH).expect("install_cache_pager: load dyld shared cache"));
        }
    }

    /// Demand-page one shared-cache page at the faulting IPA `ipa`, staging it into an anonymous
    /// guest page (SPTM: never a file-backed map). Returns `true` iff `ipa` fell inside a cache
    /// mapping and was serviced (or was already staged); `false` if no pager is installed or `ipa`
    /// is outside every cache mapping (a genuine fault for the caller to surface).
    ///
    /// A pure deterministic function of (file bytes, slide 0, the guest's fixed PAC keys), so the
    /// record/replay dispatch routes cache faults here on BOTH sides and NEVER writes the page
    /// bytes into the trace (they are regenerated). Order:
    /// - **DATA** (v5 slide-info): `pread` the pristine page → [`walk_page`] rebases regular slots
    ///   in place and collects the auth slots → [`sign_slots`](Self::sign_slots) re-signs them with
    ///   the guest keys → write each signed pointer back at its slot offset → map at the cache IPA
    ///   under the default RW+non-exec stage-1 (the identity block map already covers the window).
    /// - **TEXT** (no slide-info): `pread` → map + [`set_region_exec`](Self::set_region_exec)
    ///   (RO+exec `ATTR_CODE`), no fixups. Sound without a TLBI: the cache window's blocks are
    ///   pristine (never translated) until the guest first faults them in here.
    pub fn page_in_cache(&mut self, ipa: u64) -> bool {
        // Move the routing table out so its borrows don't collide with `&mut self` (sign_slots /
        // vm.map / set_region_exec); restore it before returning. `CacheMeta` never references
        // `self`, so this is a plain take/put.
        let Some(cache) = self.cache.take() else { return false }; // no pager installed
        let page_base = ipa & !(GRANULE as u64 - 1);
        // Safety net: an ALREADY-mapped cache page that keeps re-faulting is a bug the VMM cannot
        // fix by re-running (e.g. a stale-TLB permission fault), and would otherwise hang the VM.
        // Track consecutive re-faults of the same already-mapped IPA and panic loudly instead.
        if self.host_span(page_base).is_some() {
            if self.cache_refault_ipa == page_base {
                self.cache_refault_count += 1;
                assert!(self.cache_refault_count < 8,
                    "cache page {page_base:#x} re-faults while already mapped — likely a stale-TLB \
                     permission fault (stage-1 attr wrong / block promoted without TLBI)");
            } else {
                self.cache_refault_ipa = page_base;
                self.cache_refault_count = 1;
            }
        } else {
            self.cache_refault_ipa = 0;
            self.cache_refault_count = 0;
        }
        let mut page = [0u8; GRANULE];
        let handled = match cache.stage_page(ipa, &mut page) {
            None => {
                // Not one of the cache's file mappings. The one legitimate in-window address that
                // is not a file mapping is the kernel-created per-process DYNAMIC DATA region at the
                // top of the shared region. dyld REQUIRES it: `dynamicRegion()` checks a
                // `"dyld_data    v3"` magic (else "mapped cache does not contain dynamic config
                // data" → fatal) and `getDyldCacheFileID` needs a non-zero FileIdTuple. Stage an
                // anon page carrying the header (magic + the cache file's dev/ino) at the region
                // base. Deterministic; identical on record & replay. Any other in-window miss is a
                // genuine fault (returns false → surfaced loudly).
                match cache.dynamic_data_region() {
                    Some((dda, dds)) if (dda..dda.wrapping_add(dds)).contains(&page_base) => {
                        if self.host_span(page_base).is_none() {
                            let (host, rlen) = alloc_pages(GRANULE); // zero-filled
                            if page_base == dda {
                                let hdr = cache.dynamic_data_header();
                                unsafe { std::ptr::copy_nonoverlapping(hdr.as_ptr(), host, hdr.len()); }
                            }
                            self.vm.map(host, page_base, rlen, MemFlags::RWX).expect("hv_vm_map (cache dyn-data page)");
                            self.backings.push(Backing { host, ipa: page_base, len: rlen });
                        }
                        true
                    }
                    _ => false, // not a cache IPA — a genuine fault
                }
            }
            Some(region) => {
                if self.host_span(page_base).is_none() {
                    if region.is_exec {
                        // TEXT: stage pristine bytes (no fixups), then ensure RO+exec (`ATTR_CODE`)
                        // stage-1. In the dynamic path this is a NO-OP: load_dynamic pre-promotes
                        // every cache exec mapping to ATTR_CODE from the cache's OWN mappings BEFORE
                        // the guest translates them (so a text fault is a pure stage-2 fault with no
                        // stale-block-TLB gap, and the block is already an L3 table with the page
                        // already ATTR_CODE — set_region_exec finds it and changes nothing). It is
                        // load-bearing only for a pager driven without that pre-promotion (tests).
                        let (host, rlen) = alloc_pages(GRANULE);
                        unsafe { std::ptr::copy_nonoverlapping(page.as_ptr(), host, GRANULE); }
                        self.vm.map(host, page_base, rlen, MemFlags::RWX).expect("hv_vm_map (cache text page)");
                        self.backings.push(Backing { host, ipa: page_base, len: rlen });
                        self.set_region_exec(page_base, GRANULE as u64);
                    } else if let Some(si) = region.slide_info {
                        // DATA with fixups: rebase regulars in place, re-sign auth slots (guest keys).
                        let auth = walk_page(&mut page, si, region.page_index, 0 /* slide 0 */, region.mapping_base);
                        let signed = self.sign_slots(&auth);
                        for (slot, val) in auth.iter().zip(signed) {
                            page[slot.offset..slot.offset + 8].copy_from_slice(&val.to_le_bytes());
                        }
                        let (host, rlen) = alloc_pages(GRANULE);
                        unsafe { std::ptr::copy_nonoverlapping(page.as_ptr(), host, GRANULE); }
                        self.vm.map(host, page_base, rlen, MemFlags::RWX).expect("hv_vm_map (cache data page)");
                        self.backings.push(Backing { host, ipa: page_base, len: rlen });
                    } else {
                        // Read-only DATA with no slide-info (e.g. __LINKEDIT, read-only const with no
                        // pointers): stage the pristine bytes as-is — no rebase, no re-sign. Maps
                        // under the default RW+non-exec stage-1 (the identity block already covers it).
                        let (host, rlen) = alloc_pages(GRANULE);
                        unsafe { std::ptr::copy_nonoverlapping(page.as_ptr(), host, GRANULE); }
                        self.vm.map(host, page_base, rlen, MemFlags::RWX).expect("hv_vm_map (cache ro-data page)");
                        self.backings.push(Backing { host, ipa: page_base, len: rlen });
                    }
                }
                true
            }
        };
        self.cache = Some(cache);
        handled
    }

    /// Demand-commit one page inside a tracked PROT_NONE reservation on first touch — the moral
    /// twin of [`page_in_cache`](Self::page_in_cache), minus the file read and re-sign. On a
    /// stage-2 translation fault, if the faulting page base lies inside a reservation recorded by
    /// [`guest_vm_reserve`](Self::guest_vm_reserve) and is not already backed, back exactly that one
    /// page with a fresh zeroed anon page (`MemFlags::RWX`; stage-1 `ATTR_DATA` governs — a data
    /// page, W^X preserved) and return `true`. A fault outside every reservation (a genuine wild
    /// pointer) returns `false` and stays fatal: the committer must never materialize untracked
    /// memory. No TLBI — a fresh IPA the guest never translated (same soundness as the cache pager).
    ///
    /// Deterministic and trace-free: record and replay re-execute the guest's own stores, fault at
    /// the same IPAs in the same order, and commit identical all-zero pages, so nothing about a
    /// committed page enters the trace (same posture as the cache pager, timebase MRS, FPAC strip).
    pub fn commit_reserved_page(&mut self, ipa: u64) -> bool {
        let page_base = ipa & !(GRANULE as u64 - 1);
        // Strict gate: only pages inside a tracked reservation are demand-committable. Everything
        // else stays fatal (the dispatch surfaces it via describe_stop) — no wild materialization.
        if !self.reservations.iter().any(|&(start, len)| page_base >= start && page_base < start + len) {
            return false;
        }
        // Already backed (host_span hit): don't double-map. Returning false here is the refault
        // guard — unlike page_in_cache, which re-runs already-mapped cache pages and thus needs an
        // 8-strike anti-spin counter, the committer cannot spin: a re-fault on an already-committed
        // data page is an unfixable bug (stale-TLB / permission) that goes straight to the fatal
        // describe_stop path, so it fails loud rather than livelocking.
        if self.host_span(page_base).is_some() {
            return false;
        }
        let (host, rlen) = alloc_pages(GRANULE); // zero-filled
        self.vm.map(host, page_base, rlen, MemFlags::RWX).expect("hv_vm_map (commit reserved page)");
        self.backings.push(Backing { host, ipa: page_base, len: rlen });
        true
    }

    /// The faulting guest-physical address (FAR/IPA) of the most recent non-syscall VM exit
    /// (`Stop::Other`). The record/replay dispatch feeds this to [`page_in_cache`](Self::page_in_cache)
    /// to service cache-window stage-2 faults.
    pub fn fault_ipa(&self) -> u64 { self.last_far }

    /// The live stage-1 leaf descriptor for `ipa`, walking the L2/L3 the box maintains: an L3 page
    /// descriptor where the block has been promoted, otherwise the L2 block descriptor itself.
    /// `None` when there is no live L2, the IPA is outside the 36-bit space, or the promoted L3 is
    /// not among the tracked backings. Shared by the attribute observables below.
    fn leaf_desc(&self, ipa: u64) -> Option<u64> {
        if self.l2_host.is_null() { return None; }
        let bi = (ipa / BLK) as usize;
        if bi >= 2048 { return None; }
        let l2 = unsafe { std::slice::from_raw_parts(self.l2_host as *const u64, 2048) };
        if l2[bi] & 0x3 == DESC_TABLE {
            let l3_ipa = l2[bi] & !(GRANULE as u64 - 1);
            let host = self.backings.iter().find(|b| b.ipa == l3_ipa).map(|b| b.host)?;
            let l3 = unsafe { std::slice::from_raw_parts(host as *const u64, 2048) };
            let idx = ((ipa - bi as u64 * BLK) / GRANULE as u64) as usize;
            Some(l3[idx])
        } else {
            Some(l2[bi]) // block descriptor: identity data block, never executable
        }
    }

    /// Test/diagnostic observable: is the stage-1 leaf mapping for `ipa` executable at EL0
    /// (`ATTR_CODE`: UXN clear)? A default data block (or a promoted `ATTR_DATA` page) is
    /// non-exec; only a `set_region_exec` page (guest code, the sign stub, or a paged-in cache
    /// TEXT page) is.
    pub fn ipa_is_exec(&self, ipa: u64) -> bool {
        let Some(leaf) = self.leaf_desc(ipa) else { return false };
        leaf & 0x3 != 0 && leaf & UXN == 0
    }

    /// Make `[ipa, ipa+len)` inaccessible to EL0 — the guest asked for `PROT_NONE`, so the page
    /// tables say so and the hardware enforces it. Page-granular: the range is rounded out to whole
    /// pages, since a stage-1 leaf is the finest thing that can carry an attribute.
    ///
    /// **Requires every page in the range to be backed, and asserts otherwise.** That is M13's
    /// central invariant, and it is what makes the fault unambiguous: a protected page is backed, so
    /// an EL0 access takes a stage-1 PERMISSION fault via the EL1 trampoline and arrives as
    /// `Stop::Fault` for M12's disposition check; a reserved-but-uncommitted page is unbacked, so its
    /// access takes a stage-2 TRANSLATION fault direct to EL2 and arrives as `Stop::Other` for
    /// `commit_reserved_page`. Two exception routes, two `Stop` variants — the hardware separates
    /// "reserved and committable" from "protected, must fault", and no software gate has to.
    ///
    /// **The TLBI is a correctness requirement here, not a precaution.** Every other caller of
    /// `set_region_attr` stamps a fresh IPA the guest has never translated — the sign stub, the TLBI
    /// stub, a cache page, a fresh exec mmap — and each documents that as its soundness argument.
    /// This one stamps a page the guest is actively using (libstd's guard page lives inside the stack
    /// it is running on). A missing flush leaves a stale PERMISSIVE entry, so the guard silently
    /// fails to guard: the precise class of quiet wrong answer this milestone exists to remove.
    ///
    /// Deterministic and trace-free: a pure function of the guest's own protection calls, which are
    /// already recorded syscalls that replay re-dispatches through this same method.
    pub fn protect_none(&mut self, ipa: u64, len: u64) {
        let g = GRANULE as u64;
        let start = ipa & !(g - 1);
        let end = (ipa.saturating_add(len) + g - 1) & !(g - 1);
        if end <= start { return; }
        let mut p = start;
        while p < end {
            assert!(self.host_span(p).is_some(),
                "protect_none: no backing for {p:#x} (in [{start:#x},{end:#x})). M13 models \
                 no-access only on BACKED pages: an unbacked protected page would fault at stage 2, \
                 where commit_reserved_page would silently materialize it instead of faulting. \
                 Model the reservation-protect case deliberately before a guest needs it.");
            p += g;
        }
        self.set_region_attr(start, end - start, ATTR_NONE);
        self.noaccess.push((start, end - start));
        // The guest may already hold a translation for these pages — unlike every other stamp site.
        self.flush_guest_tlb();
    }

    /// Restore ordinary EL0 read/write access to `[ipa, ipa+len)` and drop it from the protection
    /// map. The mirror of [`protect_none`](Self::protect_none), and it needs the same flush for the
    /// same reason in the opposite direction: a stale RESTRICTIVE entry would keep faulting on a page
    /// the guest has legitimately unprotected.
    ///
    /// Restores `ATTR_DATA` unconditionally rather than whatever the page held before it was
    /// protected. Today those are the same thing — only `ATTR_DATA` pages are ever protected, since
    /// code pages are read-only and no guest protects them — and the choice is documented rather than
    /// incidental: a general protection map would have to remember the prior attribute instead.
    pub fn unprotect(&mut self, ipa: u64, len: u64) {
        let g = GRANULE as u64;
        let start = ipa & !(g - 1);
        let end = (ipa.saturating_add(len) + g - 1) & !(g - 1);
        if end <= start { return; }
        self.set_region_attr(start, end - start, ATTR_DATA);
        subtract_range(&mut self.noaccess, start, end - start);
        self.flush_guest_tlb();
    }

    /// Drop any no-access protection covering `[ipa, ipa+len)` as part of tearing that range down.
    ///
    /// Both teardown paths need this, and neither may use a bare `subtract_range`: stage-1 leaves
    /// live in the box's OWN page tables and survive a stage-2 unmap ("the stage-1 identity block
    /// stays" — [`guest_munmap`](Self::guest_munmap)), so dropping only the MAP would leave the
    /// hardware still denying EL0 at an IPA the guest has legitimately released. The next mapping
    /// there would then fault on a protection its guest never asked for and cannot see or undo —
    /// the same silent wrong answer, one layer down, that M13 exists to remove.
    ///
    /// [`unprotect`](Self::unprotect) resets the leaf, subtracts the range and flushes; the overlap
    /// test keeps the common never-protected teardown free of a needless guest TLBI.
    fn drop_protection(&mut self, ipa: u64, len: u64) {
        let end = ipa.saturating_add(len);
        if self.noaccess.iter().any(|&(s, l)| ipa < s + l && s < end) {
            self.unprotect(ipa, len);
        }
    }

    /// The tracked no-access extents as `(start, len)` (test/diagnostic observability, the twin of
    /// [`reservations`](Self::reservations)).
    pub fn noaccess(&self) -> &[(u64, u64)] { &self.noaccess }

    /// Test/diagnostic observable: does the stage-1 leaf for `ipa` deny EL0 all access (`ATTR_NONE`:
    /// AP bits `0b00`)? The AP field uniquely identifies the four attributes this box installs —
    /// `ATTR_NONE` `0x00`, `ATTR_DATA` `0x40`, `ATTR_TRAMP` `0x80`, `ATTR_CODE` `0xC0` — so testing
    /// it is exact rather than heuristic.
    pub fn ipa_is_noaccess(&self, ipa: u64) -> bool {
        let Some(leaf) = self.leaf_desc(ipa) else { return false };
        leaf & 0x3 != 0 && leaf & 0xC0 == 0x00
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

    /// Invalidate the guest's stage-1 TLB by running `tlbi vmalle1` **on the guest vCPU itself** —
    /// the VMM cannot issue a guest TLBI, and this project never emulates an instruction the guest
    /// can run (the same rule that makes the PAC oracle run real `pac*`/`aut*`).
    ///
    /// Required whenever a stage-1 attribute changes on a range the guest may already have
    /// translated. `set_region_exec`/`set_region_attr` alone are sound only on a pristine
    /// block; this is what makes a promotion sound anywhere else.
    ///
    /// Does NOT disturb the caller's guest state: the stub runs on a dedicated scratch page and the
    /// full architectural state is saved and restored around it (`save_state`/`restore_state`, the
    /// same pair `sign_slots` uses), so a mid-run caller sees nothing.
    ///
    /// Below the trace: called from paths shared by record and replay, so it fires identically on
    /// both sides (symmetry rule 2) and never surfaces to the record/replay loop.
    pub fn flush_guest_tlb(&mut self) {
        self.ensure_tlbi_stub();
        let saved = self.save_state();
        self.vcpu.set_reg(reg::PC, TLBI_STUB_IPA).expect("set PC (tlbi stub)");
        self.vcpu.set_reg(reg::CPSR, TLBI_STUB_CPSR).expect("set CPSR (tlbi stub)");
        self.run_tlbi_stub();
        self.restore_state(&saved);
    }

    /// Lazy-init the TLBI scratch on first use: one stub CODE page at a fixed reserved IPA, RO +
    /// EL1-exec (`ATTR_TRAMP` — `tlbi` is an EL1 instruction, so this cannot share the sign stub's
    /// `ATTR_CODE` page, which sets PXN: EL0-exec, EL1 no-exec). W^X — it is written once, before it
    /// is ever promoted, and never written again.
    fn ensure_tlbi_stub(&mut self) {
        if self.tlbi_stub_ready { return; }
        let (host, rlen) = alloc_pages(GRANULE);
        unsafe {
            std::ptr::copy_nonoverlapping(
                TLBI_STUB.as_ptr() as *const u8, host, std::mem::size_of_val(&TLBI_STUB));
        }
        self.vm.map(host, TLBI_STUB_IPA, rlen, MemFlags::RWX).expect("hv_vm_map (tlbi stub)");
        self.backings.push(Backing { host, ipa: TLBI_STUB_IPA, len: rlen });
        // No TLBI needed for this promotion itself: TLBI_STUB_IPA is a fresh IPA the guest has
        // never translated (same soundness argument as the sign stub and the cache pager).
        self.set_region_attr(TLBI_STUB_IPA, GRANULE as u64, ATTR_TRAMP);
        self.tlbi_stub_ready = true;
    }

    /// Run the TLBI stub to its terminating `hvc`. From EL1 an `hvc` traps straight to EL2, so the
    /// terminating exit is a plain `Ec::Hvc` — no ESR_EL1 indirection (contrast `run_sign_stub`,
    /// whose EL0 `svc` arrives via the guest's EL1 trampoline).
    fn run_tlbi_stub(&mut self) {
        for _ in 0..TLBI_STUB_BOUND {
            let e = self.vcpu.run().expect("hv_vcpu_run (tlbi stub)");
            if e.reason != EXIT_EXCEPTION { continue; }
            match ec_of(e.syndrome) {
                Ec::Hvc => return, // the stub's terminating hvc: the flush is done
                other => panic!(
                    "tlbi stub faulted at EL1: EC={other:?} (syndrome={:#x} far={:#x}) — bad \
                     encoding, a non-EL1-exec stub page (ATTR_TRAMP required), or CPSR not EL1h",
                    e.syndrome, e.virtual_address),
            }
        }
        panic!("tlbi stub did not reach its terminating hvc within {TLBI_STUB_BOUND} runs");
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
    ///
    /// `argv` is the guest's full argument vector, `argv[0]` first (the exe path — what the kernel
    /// passes and what dyld's `executable_path=` is derived from). M9 widened this from a lone
    /// `argv0`; an empty slice yields `argc=0`, which no caller does but the layout handles.
    pub fn load_dynamic(exe: &Loaded, dyld: &Loaded, argv: &[String]) -> Box_ {
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
        // Thread-pointer TSD region (see TSD_IPA): a zeroed region spanning below+above the thread
        // pointer (TPIDRRO_EL0 = TSD_IPA sits in its middle) so both TSD-key and pthread-field accesses land in it.
        map(&vm, &mut backings, TSD_REGION_BASE, &[], TSD_REGION_SIZE as usize);

        // The main executable's mach-header address (dyld4's KernelArgs.mainExecutable): the exe
        // segment whose bytes begin with MH_MAGIC_64. dyld dereferences this to parse the main exe.
        let main_hdr = exe.segments.iter()
            .find(|s| s.data.len() >= 4 && s.data[0..4] == [0xcf, 0xfa, 0xed, 0xfe])
            .map(|s| s.vaddr)
            .expect("load_dynamic: no exe segment carries the Mach-O header (MH_MAGIC_64)");
        // Build the XNU start stack in the (already-mapped, zeroed) stack backing; get guest SP.
        let sp = Self::build_start_stack(&backings[stack_idx], argv, main_hdr);

        // W^X exec ranges: trampoline + exe exec segs (unslid) + dyld exec segs (slid) + the shared
        // cache's executable regions. The cache text stage-1 MUST be ATTR_CODE BEFORE the guest ever
        // translates it, so demand-paging a text page is a pure stage-2 fault needing no runtime
        // stage-1 change. Otherwise a cache page first read as data would translate its covering
        // 32 MiB block as a default RW/UXN block (caching that block-granule TLB entry); a later
        // runtime data→exec promotion of a page in that block cannot be made visible (the VMM cannot
        // issue a guest TLBI), so the instruction fetch keeps hitting the stale non-exec entry and
        // re-faults forever. Pre-setting the real per-page attrs from the cache's OWN mappings (at
        // slide 0, exactly where page_in_cache maps them) closes that gap. `CacheMeta` is loaded here
        // and kept as the pager so #294/#536 need only confirm it.
        let cache_meta = CacheMeta::load(DEFAULT_CACHE_PATH).expect("load_dynamic: load dyld shared cache");
        let mut exec = vec![(TRAMPOLINE_IPA, 0x800u64, ATTR_TRAMP)];
        for s in &exe.segments  { if s.exec { exec.push((s.vaddr,             s.memsz as u64, ATTR_CODE)); } }
        for s in &dyld.segments { if s.exec { exec.push((s.vaddr + DYLD_BASE, s.memsz as u64, ATTR_CODE)); } }
        for (addr, size) in cache_meta.exec_mappings() {
            exec.push((addr, size, ATTR_CODE));
        }
        let pt_start = backings.len();
        let (ttbr0, l2_host, next_l3) = Self::build_tables(&mut backings, &exec);
        for bk in &backings[pt_start..] { vm.map(bk.host, bk.ipa, bk.len, MemFlags::RWX).expect("hv_vm_map (pt)"); }

        Self::set_pac_keys(&vcpu);
        vcpu.set_sys(sysreg::VBAR_EL1, TRAMPOLINE_IPA).unwrap();
        vcpu.set_trap_debug_exceptions(true).unwrap();          // route SS/breakpoint exits to the VMM (Box_::step)
        vcpu.set_sys(sysreg::MAIR_EL1,  MAIR_EL1_V).unwrap();
        vcpu.set_sys(sysreg::TCR_EL1,   TCR_EL1_V).unwrap();
        vcpu.set_sys(sysreg::TTBR0_EL1, ttbr0).unwrap();
        vcpu.set_sys(sysreg::CPACR_EL1, CPACR_FP_ON).unwrap(); // FPEN=0b11: EL0/EL1 FP/SIMD (dyld uses NEON)
        vcpu.set_sys(sysreg::TPIDRRO_EL0, TSD_IPA).unwrap();   // thread pointer (kernel-provided TSD)
        // TPIDR_EL0 is NOT a second TSD pointer: macOS 26 reads the current CPU number from
        // TPIDR_EL0[11:0] and the cluster number from TPIDR_EL0[>=12] (_os_cpu_number /
        // _os_cpu_cluster_number). A single-vCPU guest is always cpu 0 / cluster 0, so TPIDR_EL0
        // must be 0 -- TSD_IPA (0x30000) would read as cluster 48 and blow libmalloc xzone's
        // per-cluster segment-group index out of bounds (M2-cpuid).
        vcpu.set_sys(sysreg::TPIDR_EL0,   0).unwrap();
        // Derived from the MAIN executable's cpusubtype, not dyld's — dyld is itself arm64e, and
        // deriving from it would recreate the bug this task fixes (M7's wall).
        let pac = pac_posture(exe.cpusubtype);
        vcpu.set_sys(sysreg::SCTLR_EL1, sctlr_mmu_on(pac)).unwrap();
        vcpu.set_sys(sysreg::SP_EL0, sp).unwrap();
        vcpu.set_reg(reg::CPSR, 0).unwrap();                        // EL0t
        vcpu.set_reg(reg::PC, dyld.entry + DYLD_BASE).unwrap();     // dyld's SLID entry
        let mut b = Box_ { vm, vcpu, backings, reservations: Vec::new(), noaccess: Vec::new(), mmap_next: MMAP_BASE, bootstrap_port: None, l2_host, next_l3, last_far: 0, synthetic_tsc: SYNTH_TSC_START, cache_refault_ipa: 0, cache_refault_count: 0, cache: Some(cache_meta), bps_armed: false, wps_armed: false, watch_ranges: Vec::new(), syscall_watch_hit: None, pac_enabled: pac, stack_top: DYN_STACK_TOP, stack_size: DYN_STACK_SIZE, tlbi_stub_ready: false, fds: FdTable::new(), sigtable: SigTable::default(), threads: thread::ThreadTable::new(thread::ThreadCtx::zeroed()), thread_start_pc: None, wq_thread_pc: None, pthread_size: None };
        b.reserve_believed_stack();
        // M14: thread 0's context was zeroed above (the table exists before the vCPU does); overwrite
        // it with the real startup state just written to the vCPU so it reflects reality from the
        // first `switch_to_thread` rather than an all-zero placeholder.
        let ctx0 = b.save_ctx();
        *b.threads.ctx_mut(0) = ctx0;
        b
    }

    // Build dyld4's process-start stack (its `KernelArgs`) in the zeroed stack backing; return SP.
    // `__dyld_start` does `x0 = sp; b start`, and dyld reads the struct at SP as:
    //   [0]  mainExecutable  (pointer to the main exe's mach_header)
    //   [8]  argc
    //   [16] argv[0..argc], NULL
    //        envp[..], NULL          (empty: dyld uses the standard shared region, demand-paged)
    //        apple[..], NULL
    // (mainExecutable is the dyld4 addition over the classic XNU `argc,argv,envp,apple` frame.)
    //
    // The `apple[]` array carries the kernel-provided launch tokens that libSystem's initializers
    // parse. libpthread REQUIRES `ptr_munge=` (its pointer-mangling cookie) — a zero token makes
    // its init `brk` with "BUG IN LIBPTHREAD: Token from the kernel is 0". libc reads `stack_guard=`
    // (the `__stack_chk_guard` canary) and libmalloc `malloc_entropy=`. We synthesize fixed,
    // non-zero values (identical on record & replay — deterministic; these are opaque cookies the
    // guest only XORs/stores, never checks against anything external). `executable_path=` gives dyld
    // the main exe's path.
    fn build_start_stack(stack: &Backing, argv: &[String], main_hdr: u64) -> u64 {
        let base_ipa = stack.ipa;
        let top = stack.ipa + stack.len as u64;
        let argv0 = argv.first().map(String::as_str).unwrap_or("");
        // strings[0..argc] = argv; the rest are apple[] entries (order irrelevant — parsed by key).
        let mut strings: Vec<Vec<u8>> = Vec::new();
        let push = |s: String, out: &mut Vec<Vec<u8>>| { let mut v = s.into_bytes(); v.push(0); out.push(v); };
        for a in argv { push(a.clone(), &mut strings); }                         // argv[0..argc]
        push(format!("executable_path={argv0}"), &mut strings);                  // apple[0]
        push("ptr_munge=0x1a2b3c4d5e6f7a8b".to_string(), &mut strings);          // libpthread cookie (nonzero)
        push("stack_guard=0x000a0b0c0d0e0f00".to_string(), &mut strings);        // __stack_chk_guard (low byte 0)
        push("malloc_entropy=0x00112233445566778899aabbccddeeff".to_string(), &mut strings); // libmalloc 2x64 entropy
        let argc = argv.len();
        let n_apple = strings.len() - argc;

        // Lay the strings down from the top of the stack; record each one's guest address.
        let mut p = top;
        let mut addr = vec![0u64; strings.len()];
        for (i, s) in strings.iter().enumerate() {
            p -= s.len() as u64;
            addr[i] = p;
            unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), stack.host.add((p - base_ipa) as usize), s.len()); }
        }
        // KernelArgs words: mainExecutable, argc, argv[0..argc], NULL, NULL(envp), apple[0..], NULL.
        let mut words = vec![main_hdr, argc as u64];
        words.extend((0..argc).map(|i| addr[i]));
        words.push(0);                                          // argv terminator
        words.push(0);                                          // envp terminator (empty)
        words.extend((0..n_apple).map(|i| addr[argc + i]));
        words.push(0); // apple[] terminator
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
            // Kernel-faithful VM_FLAGS_ANYWHERE-with-hint: search FORWARD from a non-zero hint for
            // the first free gap, treating reservations as occupied (what vm_map_enter does). When
            // the hint's own range is free, first_fit returns the hint verbatim (the common case);
            // when it collides — e.g. libmalloc's guarded-metadata commit whose hint is a reserved
            // band with an interior carveout — it lands in the first free gap (the hole). A zero hint
            // or no fit falls back to the deterministic bump allocator.
            match if addr != 0 { self.first_fit(addr, rlen as u64) } else { None } {
                Some(a) => {
                    // A first-fit hit may land at/above the bump cursor (a hinted commit past every
                    // reservation); float mmap_next past it so no later bump hands out an overlapping
                    // IPA. A no-op in the common case (hint honored below the cursor, e.g. the xzone
                    // carveout hole). Pure max of reset-on-restore state — deterministic.
                    self.mmap_next = self.mmap_next.max((a + rlen as u64 + (GRANULE as u64 - 1)) & !(GRANULE as u64 - 1));
                    a
                }
                None => {
                    if exec { self.mmap_next = (self.mmap_next + (BLK - 1)) & !(BLK - 1); }
                    let a = self.mmap_next; self.mmap_next += rlen as u64; a
                }
            }
        } else {
            // FIXED (dyld/libmalloc often pass VM_FLAGS_OVERWRITE): the guest may be replacing a
            // region it previously bump-allocated, so classify the overlap exactly as the BSD mmap
            // path does — `place_fixed` is shared with `map_mmap_region`. A request CONTAINED in a
            // live backing reuses it in place and is already complete (returning early is safe: on
            // that path `place_fixed` itself promotes-and-flushes when `exec` is set — M9 — so the
            // `set_region_exec` below, which never runs on this path, is already done). Anything
            // else leaves the range clear for the fresh stage-2 map, which hv_vm_map would otherwise
            // reject for overlapping.
            if let Some(a) = self.place_fixed(host, rlen, addr, exec) { return a; }
            addr
        };
        self.vm.map(host, ipa, rlen, MemFlags::RWX).expect("hv_vm_map (guest_vm_map)");
        self.backings.push(Backing { host, ipa, len: rlen });
        if exec { self.set_region_exec(ipa, size); }
        ipa
    }

    /// Service a PROT_NONE address-space RESERVATION (mach_vm_map with cur_protection == 0):
    /// bookkeeping only — no host allocation, no stage-2 map. libmalloc's nano allocator reserves
    /// a large "pointer range" (observed: a FIXED 24 GiB region) this way and commits sub-ranges
    /// later with a real-protection mach_vm_map; eagerly backing the whole reservation would be
    /// infeasible and serves no purpose. FIXED honors the requested base; ANYWHERE hands out a
    /// fresh deterministic bump address (advancing `mmap_next` so nothing later collides).
    /// Deterministic: identical call sequence => identical returned address on replay.
    pub fn guest_vm_reserve(&mut self, addr: u64, size: u64, anywhere: bool) -> u64 {
        // Page-granular extent: commit_reserved_page backs whole pages, so track whole pages.
        let rounded = (size + GRANULE as u64 - 1) & !(GRANULE as u64 - 1);
        let base = if anywhere {
            let end = self.mmap_next + rounded;
            assert!(end <= (1u64 << 36),
                "guest_vm_reserve ANYWHERE overflowed 36-bit IPA space: {end:#x}");
            let a = self.mmap_next;
            self.mmap_next = end;
            a
        } else {
            // Defensive: if the bump cursor sits inside a FIXED reservation, jump it past the
            // reserved band so no later ANYWHERE bump can hand out an IPA the guest believes is
            // its private reserved range. (With MMAP_BASE above the nano band this never fires for
            // that band, but it keeps guest_vm_reserve sound for any FIXED reservation.)
            let end = addr.saturating_add(size);
            if self.mmap_next >= addr && self.mmap_next < end {
                self.mmap_next = end;
            }
            addr
        };
        // Record the extent so commit_reserved_page can demand-commit its pages on first touch.
        // Deterministic: the same call sequence records the same (base, rounded) on record & replay.
        self.reservations.push((base, rounded));
        base
    }

    /// Is `[ipa, ipa+len)` free of any tracked backing AND any PROT_NONE reservation, clear of the
    /// shared-cache window, and within the 36-bit IPA space? A real `vm_map_entry` (a reservation)
    /// occupies its VA on the kernel, so an ANYWHERE placement can never land inside one; excluding
    /// `reservations` here makes the emulated VM agree. (Used to decide ANYWHERE hint placement.)
    fn range_is_free(&self, ipa: u64, len: u64) -> bool {
        let end = ipa.saturating_add(len);
        if end > (1u64 << 36) { return false; }                            // out of 36-bit IPA space
        if ipa < SHARED_REGION_END && SHARED_REGION_START < end { return false; } // shared-cache window
        // The VMM's lazily-mapped scratch pages: forbidden even while still absent from `backings`.
        if ipa < SCRATCH_RESERVED_END && SCRATCH_RESERVED_START < end { return false; }
        if self.backings.iter().any(|b| ipa < b.ipa + b.len as u64 && b.ipa < end) { return false; }
        if self.reservations.iter().any(|&(s, l)| ipa < s + l && s < end) { return false; }
        true
    }

    /// Kernel-faithful `VM_FLAGS_ANYWHERE`-with-hint placement: the lowest address `a >=`
    /// page-rounded `hint` such that `[a, a+len)` clears every backing, reservation, and forbidden
    /// window (`range_is_free`). The optimal `a` is always either the hint itself or the page-rounded
    /// end of some occupied extent (slide right until clear stops exactly at an extent's end), so we
    /// test just those candidates in address order and take the first that fits — a deterministic
    /// gap-edge first-fit. `None` if nothing fits below the 36-bit ceiling (caller falls back to the
    /// bump allocator). Pure function of (hint, len, backings, reservations) — identical on record &
    /// replay, so a first-fit-placed returned address is recomputed identically and byte-checked by
    /// the replay oracle (no new mirror needed; symmetry is structural).
    fn first_fit(&self, hint: u64, len: u64) -> Option<u64> {
        let g = GRANULE as u64;
        let base = hint & !(g - 1);
        let round_up = |x: u64| (x + g - 1) & !(g - 1);
        let mut cands = vec![base];
        for b in &self.backings {
            let end = round_up(b.ipa + b.len as u64);
            if end > base { cands.push(end); }
        }
        for &(s, l) in &self.reservations {
            let end = round_up(s + l);
            if end > base { cands.push(end); }
        }
        if SHARED_REGION_END > base { cands.push(SHARED_REGION_END); }
        if SCRATCH_RESERVED_END > base { cands.push(SCRATCH_RESERVED_END); }
        cands.sort_unstable();
        cands.dedup();
        cands.into_iter().find(|&a| self.range_is_free(a, len))
    }

    /// Place a FIXED request (`MAP_FIXED` on the BSD path, `VM_FLAGS_OVERWRITE` on the Mach one) at
    /// `addr`, classifying it against the live backings. **Shared by `map_mmap_region` and
    /// `guest_vm_map`** — the two FIXED paths must not drift apart, which a second copy of this
    /// logic is exactly how they would.
    ///
    /// Returns `Some(addr)` when the request was satisfied by REUSING an existing backing: the
    /// caller must return that address immediately, having mapped nothing (`host` is already freed).
    /// Returns `None` when the caller should go on to stage-2 map `host` at `addr` as usual.
    ///
    /// Three cases:
    /// 1. **Fully covers** every backing it touches (including touching none) → drop them and let
    ///    the caller install `host`. The original behaviour, and the only case `unmap_overlapping`'s
    ///    wholesale drop is sound for.
    /// 2. **Fully contained** in ONE backing → reuse it in place: copy `host`'s pages over
    ///    `[addr, addr+rlen)` and keep the rest of the backing intact. `alloc_pages` maps with
    ///    `MAP_ANON`, which `mmap(2)` guarantees zero-filled, so an anonymous FIXED request gets
    ///    exactly the fresh zero pages the kernel would give it; on the file-backed path the same
    ///    copy carries the staged bytes. This is the case that matters: libstd's
    ///    `install_main_guard` mmaps at `usrstack64 - RLIMIT_STACK`, wholly inside the 256 KiB
    ///    dynamic-stack backing, and dyld/libmalloc pass `VM_FLAGS_OVERWRITE` on the Mach path.
    ///    Loaded image segments, the L1/L2 page tables and the PAC sign stub/table are one backing
    ///    apiece and equally destroyable.
    /// 3. **True partial straddle** → fail loud. Nothing exercises it; guessing at split semantics
    ///    is worse than refusing.
    ///
    /// `exec` on the containment path (case 2, below) is honored by promoting the reused backing to
    /// RO+exec (`set_region_exec`) and then invalidating the guest's stale TLB entry for it with the
    /// guest-side TLBI oracle (`flush_guest_tlb`) — a block the guest has already translated can carry
    /// a stale RW/UXN entry, so promotion alone would leave the guest running on it. This is dyld's
    /// non-cache-dylib strategy: reserve the image's span, touch it, then `MAP_FIXED` each segment
    /// in with its own protections (M9).
    fn place_fixed(&mut self, host: *mut u8, rlen: usize, addr: u64, exec: bool) -> Option<u64> {
        // Every FIXED path funnels through here, so validate the address HERE: a caller that forgets
        // cannot silently hand `hv_vm_map` an address it rejects. The BSD path checks `fixed_fits`
        // itself first and answers the guest EINVAL, so it never trips this; the Mach path has no
        // errno channel plumbed to its four call sites and no guest exercises it, so it fails loud
        // with a diagnosis instead of an opaque `HvError(0xfae94003)` recorder abort.
        assert!(Self::fixed_fits(addr, rlen),
            "FIXED map at {addr:#x}..{:#x} is not 16 KiB-aligned or lies outside the guest's 36-bit \
             IPA space", addr.saturating_add(rlen as u64));
        let end = addr + rlen as u64;
        let covers_all = self.backings.iter()
            .filter(|b| addr < b.ipa + b.len as u64 && b.ipa < end)   // the overlapping ones
            .all(|b| addr <= b.ipa && b.ipa + b.len as u64 <= end);   // ...each wholly inside
        if covers_all {
            self.unmap_overlapping(addr, rlen as u64);                // case 1
            return None;
        }
        if let Some((bhost, bipa)) = self.backings.iter()
            .find(|b| addr >= b.ipa && end <= b.ipa + b.len as u64)
            .map(|b| (b.host, b.ipa))
        {
            // Case 2. SAFETY: `[addr, end)` lies wholly inside the backing, so the destination is
            // in-bounds; `host` is a distinct live allocation of `rlen` bytes (no overlap), and it is
            // dead after the copy, so release it here — otherwise every guard-page install and every
            // OVERWRITE commit would leak an allocation.
            unsafe {
                std::ptr::copy_nonoverlapping(host, bhost.add((addr - bipa) as usize), rlen);
                libc::munmap(host as *mut _, rlen);
            }

            // M9: an exec FIXED map contained in a LIVE backing is dyld's non-cache-dylib strategy
            // (reserve the image's span, then MAP_FIXED each segment with its own protections).
            // The block may already be translated, so promotion alone would leave the guest running
            // on a stale RW/UXN entry — promote, then invalidate on the guest itself.
            //
            // Idempotent with the caller's own set_region_exec (retrace-core's mmap dispatch calls
            // it on both record and replay): a range that is already ATTR_CODE is found and left
            // unchanged. Doing it here keeps the flush adjacent to the reason for it.
            if exec {
                self.set_region_exec(addr, rlen as u64);
                self.flush_guest_tlb();
            }
            return Some(addr);
        }
        // Case 3: overlaps something but is neither contained nor covering.
        panic!("FIXED map at {addr:#x}..{end:#x} partially straddles a live backing: splitting a \
                backing is unimplemented (no guest exercises it) and dropping it wholesale would \
                destroy guest memory the kernel keeps");
    }

    /// Can the guest's address space hold a FIXED request for `[addr, addr+rlen)`? The guest IPA
    /// space is 36-bit and the stage-1 tables are 16 KiB-granule, so a request that is misaligned,
    /// overflows, or ends above the ceiling is one the real kernel refuses with `EINVAL` — and one
    /// `hv_vm_map` refuses with `HV_BAD_ARGUMENT`, which retrace `expect`s on, converting a guest
    /// error into a recorder abort.
    ///
    /// Not academic: libstd's `install_main_guard` mmaps `MAP_FIXED` at
    /// `pthread_get_stackaddr_np() - pthread_get_stacksize_np()`, and macOS 26's libpthread reports
    /// a constant 8 MiB-minus-a-page size for the main thread regardless of `RLIMIT_STACK`, so
    /// against the box's smaller stack that subtraction underflows to a wild address.
    ///
    /// A pure function of `(addr, rlen)` and the fixed IPA geometry — record and replay classify
    /// identically, so the rejection needs no mirror (symmetry is structural).
    fn fixed_fits(addr: u64, rlen: usize) -> bool {
        addr & (GRANULE as u64 - 1) == 0
            && addr.checked_add(rlen as u64).is_some_and(|end| end <= 1u64 << 36)
    }

    /// Free every tracked backing that overlaps `[ipa, ipa+len)` (stage-2 unmap + release the anon
    /// host allocation), for a FIXED/OVERWRITE remap. A backing is removed WHOLESALE, so this is
    /// only correct when every overlapping backing lies entirely inside `[ipa, ipa+len)`.
    ///
    /// [`place_fixed`](Self::place_fixed) guarantees that for both FIXED paths: it only reaches here
    /// in the fully-covering case. Dropping a straddled backing would destroy guest memory the real
    /// kernel keeps, and "it's deterministic" is no defence: determinism only makes replay match the
    /// recording, so retrace would faithfully record a WRONG execution with no divergence and
    /// nothing flagged.
    fn unmap_overlapping(&mut self, ipa: u64, len: u64) {
        // A FIXED/OVERWRITE remap over a protected range must not leave the old protection behind,
        // for the same reason munmap must not. `place_fixed` only reaches here in the fully-covering
        // case, so `[ipa, ipa+len)` spans every backing about to be dropped.
        self.drop_protection(ipa, len);
        let end = ipa + len;
        let mut i = 0;
        while i < self.backings.len() {
            let b = &self.backings[i];
            let bend = b.ipa + b.len as u64;
            if b.ipa < end && ipa < bend {
                let bk = self.backings.remove(i);
                let _ = self.vm.unmap(bk.ipa, bk.len);
                unsafe { libc::munmap(bk.host as *mut _, bk.len); }
            } else {
                i += 1;
            }
        }
    }

    /// Is `ipa` inside any tracked guest backing? (For a handler that must not write through a
    /// null/unmapped pointer arg.)
    pub fn is_mapped(&self, ipa: u64) -> bool { self.host_span(ipa).is_some() }

    /// Read an 8-byte little-endian word from guest memory (for reading in/out pointer args).
    pub fn read_u64(&self, ipa: u64) -> u64 {
        u64::from_le_bytes(self.read_guest(ipa, 8).try_into().unwrap())
    }

    /// Mint (once, then cache) a real kernel-valid send right in retrace's OWN IPC space — which is
    /// the guest's space (Mach traps forward through), so the guest's forwarded
    /// `mach_port_mod_refs(SEND, +1)` on this name succeeds. Handed back as the synthetic
    /// `task_get_special_port(BOOTSTRAP)` reply's port name (M2-xpcport). The name is nondeterministic
    /// (kernel-assigned), so record records it and replay applies it verbatim — the `task_self`
    /// posture — never regenerated on replay. retrace holds the receive right for the process lifetime
    /// (the name stays valid); the port is deliberately never deallocated.
    pub fn mint_bootstrap_port(&mut self) -> u32 {
        if let Some(name) = self.bootstrap_port { return name; }
        let opts = MachPortOptions { flags: MPO_INSERT_SEND_RIGHT, mpl_qlimit: 0, reserved: [0, 0] };
        let mut name: u32 = 0;
        // SAFETY: a plain Mach call in retrace's own task; MPO_INSERT_SEND_RIGHT yields a name holding
        // a receive right (we keep it) AND a send right (for the guest's forwarded mod_refs).
        let kr = unsafe { mach_port_construct(mach_task_self_, &opts, 0, &mut name) };
        assert_eq!(kr, 0, "mach_port_construct failed: kr={kr:#x}");
        self.bootstrap_port = Some(name);
        name
    }

    /// Special case for anonymous mmap: allocate host pages, map them at a deterministic guest IPA,
    /// track as a backing, return the guest IPA. Same call sequence => same IPAs on replay.
    ///
    /// M8-stack: `addr`/`flags` now reach this function. Previously it took only a length and always
    /// bump-allocated, so an anonymous MAP_FIXED request silently landed at `mmap_next` — libstd's
    /// guard-page install checks `result != stackptr` and panics with errno untouched
    /// ("os error 0"). Placement is delegated to `map_mmap_region`, which the file-backed path
    /// already uses, so both paths now share one FIXED implementation.
    pub fn guest_mmap(&mut self, addr: u64, len: u64, prot: u64, flags: u64) -> Result<u64, u64> {
        let (host, rlen) = alloc_pages(len as usize);
        self.map_mmap_region(host, rlen, addr, prot, flags)
    }

    const MAP_FIXED: u64 = 0x10;
    const PROT_EXEC: u64 = 0x4;
    /// Address + stage-2-map an anon backing for an mmap. FIXED → `addr`; else bump `mmap_next`.
    /// Identical on record and replay. Returns the chosen guest IPA.
    ///
    /// **Takes ownership of `host`**: it is either installed as a new backing, or — on the
    /// containment path below — copied into the existing backing and then `munmap`ed. Callers must
    /// not read `host` afterwards (read the guest through `read_guest` instead).
    ///
    /// Step 3c (TLB-gap fix): a non-FIXED `PROT_EXEC` mmap is placed in a FRESH, block-exclusive
    /// 32 MiB block — round `mmap_next` up to the next `BLK` boundary before choosing the IPA.
    /// `set_region_exec` promotes an entire 32 MiB block from a data BLOCK to an L3 TABLE without
    /// TLB invalidation, which is only sound if that block was never translated before. Data mmaps
    /// pack normally and never promote; keeping exec regions block-exclusive guarantees promotion
    /// always hits a pristine block. (A MAP_FIXED exec mmap onto a touched block DOES need a TLBI —
    /// M9 added the guest-side oracle, `flush_guest_tlb`, and `place_fixed` now promotes-then-flushes
    /// on that path. Block-exclusive placement for non-FIXED exec mmaps is therefore no longer a
    /// correctness requirement, just an optimisation: it is a flush avoided, not a hazard avoided.)
    ///
    /// M8-stack — a FIXED request is classified against the live backings into three cases:
    /// 1. **Fully covers** every backing it touches → drop them and install `host` (the original
    ///    behaviour, and the only one `unmap_overlapping` is safe for).
    /// 2. **Fully contained** in ONE backing → reuse that backing in place: copy `host`'s pages over
    ///    `[addr, addr+rlen)` inside it and return `addr`, leaving `backings` untouched. For an anon
    ///    mmap `host` is fresh zeroed pages, so this zeroes exactly the requested range — which is
    ///    what the kernel does — while the REST of the backing keeps its contents. This is the case
    ///    that matters: libstd's `install_main_guard` mmaps `MAP_FIXED` at
    ///    `usrstack64 - RLIMIT_STACK`, wholly inside the 256 KiB dynamic-stack backing, so the old
    ///    wholesale drop would have unmapped the stack the guest is running on. Loaded image
    ///    segments, the L1/L2 page tables and the PAC sign stub/table are all likewise one backing
    ///    apiece and equally destroyable.
    /// 3. **True partial straddle** (overlaps, neither contained nor covering) → `assert!` fail-loud.
    ///    No guest exercises it; fail-loud beats guessing at split semantics (this project's posture).
    fn map_mmap_region(&mut self, host: *mut u8, rlen: usize, addr: u64, prot: u64, flags: u64)
        -> Result<u64, u64> {
        if flags & Self::MAP_FIXED == 0 && prot & Self::PROT_EXEC != 0 {
            self.mmap_next = (self.mmap_next + (BLK - 1)) & !(BLK - 1);
        }
        let ipa = if flags & Self::MAP_FIXED != 0 {
            // An address the guest's own space cannot hold is the GUEST's error, not retrace's:
            // answer EINVAL like the kernel would, rather than carrying it down to hv_vm_map and
            // aborting the recorder. Checked BEFORE `place_fixed`, whose overlap arithmetic assumes
            // a representable range. Nothing has been mapped yet, so releasing `host` (which this
            // function owns) is the whole of the cleanup: a rejected request is a no-op, leaving
            // `backings` and the `mmap_next` cursor untouched so later placements are unaffected.
            if !Self::fixed_fits(addr, rlen) {
                // SAFETY: `host` is this function's freshly-allocated `rlen`-byte mapping, never
                // published to `backings` or the guest, and unreachable after this return.
                unsafe { libc::munmap(host as *mut _, rlen); }
                return Err(retrace_arch::EINVAL);
            }
            // Classify the overlap (shared with guest_vm_map's FIXED branch). A contained request
            // reuses the existing backing and is already complete.
            //
            // M13: a PROT_NONE MAP_FIXED mmap landing wholly inside an existing backing returns
            // through HERE, not through the tail, so the protection hook must be at both exits or a
            // contained request is silently left accessible. (Measured in Task 2: libstd's guard is
            // NOT installed this way — it mmaps RW MAP_FIXED and then mprotects — so this path is
            // covered for completeness, not because a live gate takes it.)
            if let Some(a) = self.place_fixed(host, rlen, addr, prot & Self::PROT_EXEC != 0) {
                if prot == 0 { self.protect_none(a, rlen as u64); }
                return Ok(a);
            }
            addr
        } else { self.mmap_next };
        self.vm.map(host, ipa, rlen, MemFlags::RWX).expect("hv_vm_map (mmap region)");
        self.backings.push(Backing { host, ipa, len: rlen });
        if flags & Self::MAP_FIXED == 0 { self.mmap_next += rlen as u64; }
        // M13: a PROT_NONE mmap is protected once its backing exists — which is why the invariant
        // "no-access => backed" costs nothing on this path.
        if prot == 0 { self.protect_none(ipa, rlen as u64); }
        Ok(ipa)
    }
    /// RECORD: anon-alloc, stage the fd's bytes into it (SPTM: never map the file page itself), map,
    /// return (ipa, staged bytes to record so replay needs no file). Primary path is `pread`; if that
    /// fails (e.g. a POSIX shared-memory object — `com.apple.featureflags.shm` — which supports only
    /// `mmap`, not `pread`), fall back to mapping the fd read-only in RETRACE's own address space and
    /// copying the bytes out (a deterministic snapshot, captured in the trace). Either way the guest
    /// gets an anon page, never a file/shm page.
    pub fn guest_mmap_file(&mut self, addr: u64, len: u64, prot: u64, flags: u64, fd: i32, off: u64)
        -> Result<(u64, Vec<Region>), u64> {
        // M10: `fd` arrives as a GUEST descriptor. This is the second consumer of a guest fd (the
        // other is forward_and_diff): the mmap arm is special-cased in retrace-core and preads here
        // directly, so it never passes through forwarding and must translate for itself.
        let fd = if fd < 0 { fd } else { self.fds.host(fd as u64).ok_or(EBADF)? };
        let (host, rlen) = alloc_pages(len as usize);
        let n = unsafe { libc::pread(fd, host as *mut _, rlen, off as libc::off_t) };
        if n < 0 {
            // pread unsupported on this fd: snapshot via a host mmap + copy. Clamp the copy to the
            // object's size (rounded up to a page) so we never read past its end (SIGBUS).
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            let sz = if unsafe { libc::fstat(fd, &mut st) } == 0 { st.st_size as usize } else { rlen };
            let copy_len = rlen.min(((sz + GRANULE - 1) & !(GRANULE - 1)).max(GRANULE));
            let src = unsafe { libc::mmap(std::ptr::null_mut(), copy_len, libc::PROT_READ, libc::MAP_SHARED, fd, off as libc::off_t) };
            assert!(src != libc::MAP_FAILED, "guest_mmap_file: neither pread nor mmap works on fd {fd}");
            unsafe { std::ptr::copy_nonoverlapping(src as *const u8, host, copy_len); libc::munmap(src, copy_len); }
        }
        // `?`: a rejected FIXED address frees `host` inside map_mmap_region, so there is nothing to
        // clean up here — the staged bytes simply never reach the guest, exactly as with a real
        // kernel that refuses the mapping.
        let ipa = self.map_mmap_region(host, rlen, addr, prot, flags)?;
        // Read the staged bytes back out of the GUEST, not out of `host`: map_mmap_region owns
        // `host` and frees it on the containment path (where the bytes were copied into the existing
        // backing instead), so touching `host` here would be a use-after-free. read_guest is correct
        // for every case and is what actually lands in the trace.
        let bytes = self.read_guest(ipa, rlen);
        Ok((ipa, vec![Region { ipa, bytes }]))
    }
    /// REPLAY: anon-alloc (zeroed), address identically (no file access); caller applies the
    /// recorded writes to fill it. Returns the chosen IPA (must equal the recorded `ret`). `prot`
    /// must match record so the exec block-alignment in `map_mmap_region` chooses the same IPA.
    pub fn guest_mmap_replay(&mut self, addr: u64, len: u64, prot: u64, flags: u64)
        -> Result<u64, u64> {
        let (host, rlen) = alloc_pages(len as usize);
        self.map_mmap_region(host, rlen, addr, prot, flags)
    }

    /// Subtract the deallocated range `[addr, addr+len)` from every overlapping PROT_NONE reservation
    /// (kernel `mach_vm_deallocate` rounds the range out to whole pages: start down, end up). A full
    /// cover removes the entry; a head/tail overlap trims it; a strictly-interior punch SPLITS it into
    /// two entries — libmalloc's guarded-metadata carveout. The punched pages become genuinely
    /// free-and-unreserved: [`commit_reserved_page`](Self::commit_reserved_page) no longer materializes
    /// them (a touch there is fatal again, matching deallocated address space) and first-fit placement
    /// stops treating them as occupied. Deterministic: an identical `(addr, len)` sequence rebuilds the
    /// identical table on record & replay.
    fn subtract_reservations(&mut self, addr: u64, len: u64) {
        subtract_range(&mut self.reservations, addr, len);
    }

    /// Honor munmap (debt #2): punch the deallocated range out of any overlapping reservation
    /// (`subtract_reservations` — the carveout), then drop the backing covering `ipa` and
    /// `hv_vm_unmap` its stage-2 range, releasing the anon host allocation. The reservation subtract
    /// runs even when nothing is backed (the carveout case: a PROT_NONE reservation has no backing).
    pub fn guest_munmap(&mut self, ipa: u64, len: u64) {
        self.subtract_reservations(ipa, len);
        // M13: the pages are gone, so the protection goes with them — otherwise the next mapping at
        // this address inherits a no-access extent its guest never asked for. Runs BEFORE the
        // stage-2 unmap below so the pages are still backed while their leaves are reset.
        self.drop_protection(ipa, len);
        if let Some(pos) = self.backings.iter().position(|b| ipa >= b.ipa && ipa < b.ipa + b.len as u64) {
            let bk = self.backings.remove(pos);
            let _ = self.vm.unmap(bk.ipa, bk.len);       // stage-1 identity block stays; stage-2 removed
            // SAFETY: the anon host backing is no longer mapped into the guest; release it.
            unsafe { libc::munmap(bk.host as *mut _, bk.len); }
            let _ = len; // whole-backing unmap for M2's page-granular guests
        }
    }

    /// Honor `mprotect`. **Only `prot == 0` is modelled**, and that is a decision rather than an
    /// omission: dyld issues `mach_vm_protect` RW→RO during fixups and then writes through the
    /// result, so honoring the read-only bit would break the loader. No-access is what a guard page
    /// needs and is the only protection change with no blast radius on the dynamic gates. Every
    /// other `prot` keeps the pre-M13 behavior — a best-effort stage-2 re-protect to `RWX`, since
    /// our security boundary is the VMM and stage-1 W^X is already correct.
    ///
    /// A non-zero `prot` over a range that is currently no-access RESTORES it (`unprotect`), which
    /// is how a guest takes its guard page back. Shared by the `mprotect`(74) and
    /// `mach_vm_protect`(−14) dispatch arms on both record and replay, so there is one
    /// implementation and no mirror to keep in step.
    pub fn guest_mprotect(&mut self, ipa: u64, len: u64, prot: u64) {
        if prot == 0 {
            self.protect_none(ipa, len);
            return;
        }
        let end = ipa.saturating_add(len);
        if self.noaccess.iter().any(|&(s, l)| ipa < s + l && s < end) {
            self.unprotect(ipa, len);
            return;
        }
        let _ = self.vm.protect(ipa, len as usize, MemFlags::RWX);
    }

    /// Sum of tracked backing lengths (test observability: proves mmap grows / munmap shrinks
    /// the map set without exposing `backings` itself).
    pub fn mapped_len(&self) -> usize { self.backings.iter().map(|b| b.len).sum() }

    /// The tracked PROT_NONE reservation extents as `(start, len)` (test/diagnostic observability:
    /// proves a `mach_vm_deallocate` hole-punch splits/trims the table with exact bounds).
    pub fn reservations(&self) -> &[(u64, u64)] { &self.reservations }

    pub fn run(&mut self) -> Stop {
        // M14 Task 9: switch threads only at a CLEAN STOP BOUNDARY — here, on entry, where the
        // previous stop has been fully handled by the dispatch loop and the guest is about to be
        // resumed. Switching mid-`mach_msg2` or mid-demand-page would leave in-flight box state
        // describing a thread that is no longer running (risk R3); neither of those re-enters
        // `run()` with the current thread blocked, so neither can reach this.
        //
        // Entry, not exit, because the blocking happens AFTER `run()` returns: the dispatch arm for
        // 515/361 is what marks the thread `Blocked`/`Exited`, and (for 515) `set_x0_err_and_return`
        // then leaves the vCPU holding that thread's resume-ready EL0 state — exactly what
        // `switch_to_thread` must save before loading the next thread's.
        //
        // Below the trace on purpose (symmetry rule 2): `run()` is shared by record and replay, so
        // the schedule fires identically on both sides and nothing about it is recorded. A
        // single-threaded guest has a one-entry table whose only thread is `Runnable`, so
        // `needs_reschedule()` is false and every M0–M13 path is untouched.
        if self.threads.needs_reschedule() {
            self.schedule_after_block();
        }
        // M15 R1: everything M15 says about "the thread at (N, K)" depends on the switch having
        // already happened before the first instruction of this window retires. Pin it: after this
        // point, no path may change `current` until the next run()/step() entry.
        debug_assert!(!self.threads.needs_reschedule(),
            "M15 R1: a reschedule is still pending after schedule_after_block — a mid-window switch \
             would make position->thread ambiguous");
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
                        // An undefined-instruction trap (EC=0x00) that is really an Apple IMPDEF
                        // `MRS` HVF won't expose: emulate + skip it (like the timebase), staying in
                        // lockstep without surfacing to the record/replay loop.
                        Ec::Other(0) if self.try_emulate_undef_mrs() => continue,
                        // A B-family pointer-auth (autdb/autib) that FEAT_FPAC-faulted: objc auths
                        // arm64e cache pointers the A-family re-signer can't reach. Emulate the
                        // authenticate by stripping the aut* destination + skip, like the MRS
                        // emulations above — below the record/replay loop, so both stay in lockstep.
                        Ec::Other(0x1C) if self.try_emulate_fpac_auth() => continue,
                        // M6: a lower-EL (EL0) data/instruction abort is a GUEST CRASH — a
                        // recordable stop, not a retrace bug. EC bit 0 distinguishes lower-EL
                        // (0x24/0x20, guest code faulted) from same-EL (0x25/0x21, the trampoline
                        // itself faulted — that stays in the fail-loud arm below). The faulting
                        // EL0 pc is ELR_EL1 (the vCPU's live PC is parked in the trampoline).
                        Ec::DataAbort | Ec::InstrAbort if (esr1 >> 26) & 1 == 0 => {
                            let far = self.vcpu.get_sys(sysreg::FAR_EL1).unwrap();
                            let pc = self.vcpu.get_sys(sysreg::ELR_EL1).unwrap();
                            self.last_far = far;
                            return Stop::Fault { pc, esr: esr1, far };
                        }
                        _ => {
                            self.last_far = self.vcpu.get_sys(sysreg::FAR_EL1).unwrap();
                            return Stop::Other { esr: esr1 };
                        }
                    }
                    let num = self.vcpu.get_reg(reg::x(16)).unwrap();
                    let mut args = [0u64;8];
                    for (i, a) in args.iter_mut().enumerate() { *a = self.vcpu.get_reg(reg::x(i as u32)).unwrap(); }
                    return Stop::Syscall { num, args };
                }
                // A software-step exception delivers direct to EL2 (F1: EC2=0x32/0x33), so it lands
                // on THIS outer match — never inside the Ec::Hvc trampoline arm. run() never arms SS
                // (only Box_::step does, disarming after), so reaching here means SS leaked; fail loud.
                Ec::SoftStep => panic!(
                    "software-step exception outside Box_::step() — SS leaked; pc=0x{:x}",
                    self.vcpu.get_reg(reg::PC).unwrap()),
                _ => {
                    // A stage-2 abort taken by the hypervisor (not via the guest VBAR). Cache-window
                    // faults are serviced by the record/replay dispatch via `page_in_cache` (file →
                    // walk → re-sign → map, identical on record and replay), so surface the fault IPA
                    // as `Stop::Other` for it to route.
                    self.last_far = e.virtual_address;
                    return Stop::Other { esr: e.syndrome };
                }
            }
        }
    }

    /// Advance the guest by exactly one instruction. Returns `Stop::Step` on a clean single-step or
    /// when one below-the-trace emulation (timebase/undef-MRS/FPAC) stands in for the instruction;
    /// `Stop::Syscall` if the next instruction is the window-ending trap (NOT consumed — the caller
    /// forwards/records it); `Stop::Other` for a fault (nothing retired — the caller pages in and
    /// retries the same step). Arms both step bits, runs one classification, then disarms both so
    /// `run()`/forward paths never step.
    pub fn step(&mut self) -> Stop {
        // M14 Task 9 fix round 1, I-3: `step()` is the SECOND entry into the guest, and it needs
        // the same reschedule `run()` does. `ReplaySession`'s instruction machinery (`step_insns`,
        // `window_len_here`) drives the guest exclusively through here, so a session parked on a
        // thread that just blocked would otherwise single-step that BLOCKED thread — measuring a
        // window on the wrong thread and letting `checkpointed_seek` memoize the wrong length.
        // Silently: this is below the trace, so no `Divergence` could ever fire on it. Latent until
        // a threaded trace reaches the debug CLI, but M3/M4/M5 are shipped features and threaded
        // traces now exist, so it is fixed rather than parked.
        //
        // **THE PLACEMENT IS LOAD-BEARING: this must stay ABOVE the SS arming below.** A reschedule
        // pushed down into `run_one_for_step` would run `load_ctx` -> `write_regs` ->
        // `set_reg(reg::CPSR, ctx.regs.cpsr)`, wiping the `PSTATE_SS` bit just armed. MDSCR_EL1.SS
        // would still be set, leaving the step state machine active-PENDING rather than
        // active-not-pending, so the exception fires BEFORE the next instruction instead of after
        // it: `step()` returns `Stop::Step` having retired nothing, and `step_insns` reports a
        // window that ends after 0 instructions. It looks like it works.
        // `step_reschedules_off_a_blocked_thread_before_arming_the_step_bit` asserts the retired PC
        // precisely so that variant fails.
        if self.threads.needs_reschedule() {
            self.schedule_after_block();
        }
        // M15 R1: everything M15 says about "the thread at (N, K)" depends on the switch having
        // already happened before the first instruction of this window retires. Pin it: after this
        // point, no path may change `current` until the next run()/step() entry.
        debug_assert!(!self.threads.needs_reschedule(),
            "M15 R1: a reschedule is still pending after schedule_after_block — a mid-window switch \
             would make position->thread ambiguous");
        let mdscr = self.vcpu.get_sys(sysreg::MDSCR_EL1).unwrap();
        self.vcpu.set_sys(sysreg::MDSCR_EL1, mdscr | MDSCR_SS).unwrap();
        let cpsr = self.vcpu.get_reg(reg::CPSR).unwrap();
        self.vcpu.set_reg(reg::CPSR, cpsr | PSTATE_SS).unwrap();
        let stop = self.run_one_for_step();
        // Disarm: clear SS from both MDSCR_EL1 and the live PSTATE so nothing steps outside step().
        let mdscr = self.vcpu.get_sys(sysreg::MDSCR_EL1).unwrap();
        self.vcpu.set_sys(sysreg::MDSCR_EL1, mdscr & !MDSCR_SS).unwrap();
        let cpsr = self.vcpu.get_reg(reg::CPSR).unwrap();
        self.vcpu.set_reg(reg::CPSR, cpsr & !PSTATE_SS).unwrap();
        stop
    }

    /// One `hv_vcpu_run` classification for `step()`. Mirrors `run()`, but keyed on the DIRECT-EL2
    /// step exit (F1: `Ec::SoftStep` on the outer syndrome, not the `Ec::Hvc` trampoline). A clean
    /// step lands with the guest at EL0 and PC already advanced. If the stepped instruction itself
    /// trapped to EL1 (F2), it does NOT retire: it surfaces as a step exit with the guest parked at
    /// EL1 and `ESR_EL1`/`ELR_EL1` holding the real trap — so dispatch off `ESR_EL1` exactly like
    /// `run()`'s inner match (an emulation stands in for the step and returns `Stop::Step`; the
    /// window-ending svc is returned unconsumed as `Stop::Syscall`).
    fn run_one_for_step(&mut self) -> Stop {
        loop {
            let e = self.vcpu.run().expect("hv_vcpu_run");
            if e.reason != EXIT_EXCEPTION { continue; }
            match ec_of(e.syndrome) {
                Ec::SoftStep => {
                    // Guest still at EL0 => the instruction retired cleanly; PC is already at the
                    // next instruction (step() disarms SS). EL1 => it trapped without retiring.
                    let cpsr = self.vcpu.get_reg(reg::CPSR).unwrap();
                    if (cpsr >> 2) & 3 == 0 { return Stop::Step; }
                    let esr1 = self.vcpu.get_sys(sysreg::ESR_EL1).unwrap();
                    match ec_of(esr1) {
                        // The window-ending svc: return it unconsumed (PC still at the trap, exactly
                        // as run() leaves a syscall) for the caller to forward/record.
                        Ec::Svc => {}
                        // The same below-the-trace emulations run() does, but here each one IS the
                        // step: emulate + resume EL0 at ELR+4 (the helpers read ELR_EL1/SPSR_EL1
                        // themselves, identical to the trampoline path) and return Stop::Step.
                        Ec::SysReg if self.try_emulate_timebase(esr1) => return Stop::Step,
                        Ec::Other(0) if self.try_emulate_undef_mrs() => return Stop::Step,
                        Ec::Other(0x1C) if self.try_emulate_fpac_auth() => return Stop::Step,
                        // M6: mirror of run()'s crash arm — a stepped instruction that faults does not retire.
                        Ec::DataAbort | Ec::InstrAbort if (esr1 >> 26) & 1 == 0 => {
                            let far = self.vcpu.get_sys(sysreg::FAR_EL1).unwrap();
                            let pc = self.vcpu.get_sys(sysreg::ELR_EL1).unwrap();
                            self.last_far = far;
                            return Stop::Fault { pc, esr: esr1, far };
                        }
                        _ => {
                            self.last_far = self.vcpu.get_sys(sysreg::FAR_EL1).unwrap();
                            return Stop::Other { esr: esr1 };
                        }
                    }
                    let num = self.vcpu.get_reg(reg::x(16)).unwrap();
                    let mut args = [0u64;8];
                    for (i, a) in args.iter_mut().enumerate() { *a = self.vcpu.get_reg(reg::x(i as u32)).unwrap(); }
                    return Stop::Syscall { num, args };
                }
                // A stage-2 abort (e.g. a cache-window fault) taken direct to EL2 while stepping: the
                // instruction did not retire. Surface it for the caller to page in and re-step.
                _ => {
                    self.last_far = e.virtual_address;
                    return Stop::Other { esr: e.syndrome };
                }
            }
        }
    }

    /// Arm hardware instruction breakpoint slot `slot` (0..=5) to match guest VA `va`, and enable
    /// the debug machine (MDSCR_EL1.MDE). A match surfaces from `run()` as `Stop::Other` with an
    /// ESR_EL2 breakpoint class (EC=0x30) at pc==va, before the instruction retires (spike F3). The
    /// M3 debugger arms these only around `advance()`/`run()` scans — NEVER while single-stepping
    /// (a BP fires before retire and would corrupt step counts), which `step()` never touches.
    pub fn arm_hw_breakpoint(&mut self, slot: usize, va: u64) {
        let (bvr, bcr) = HW_BREAKPOINT_SLOTS.get(slot)
            .copied()
            .unwrap_or_else(|| panic!("HW breakpoint slot {slot} out of range (0..=5)"));
        self.vcpu.set_sys(bvr, va).unwrap();
        self.vcpu.set_sys(bcr, DBGBCR_ARM).unwrap();
        self.bps_armed = true;
        self.sync_mde();
    }

    /// Disarm every hardware breakpoint slot and drop MDSCR_EL1.MDE, returning the vcpu to a clean
    /// (breakpoint-free) state safe to single-step from.
    pub fn clear_hw_breakpoints(&mut self) {
        for (bvr, bcr) in HW_BREAKPOINT_SLOTS {
            self.vcpu.set_sys(bvr, 0).unwrap();
            self.vcpu.set_sys(bcr, 0).unwrap();
        }
        self.bps_armed = false;
        self.sync_mde();
    }

    /// Arm hardware write-watchpoint slot `slot` (0..=3) over `[va, va+len)`, len ∈ {1,2,4,8},
    /// va len-aligned (so the range sits inside one BAS doubleword — one watch, one slot). A watched
    /// EL0 store surfaces from `run()` as `Stop::Other` with an ESR_EL2 watchpoint class (EC=0x34)
    /// and FAR in `last_far`, before the store retires (spike F4). Armed only around
    /// `advance()`/`run()` scans — NEVER while single-stepping (same discipline as breakpoints).
    pub fn arm_hw_watchpoint(&mut self, slot: usize, va: u64, len: u64) {
        assert!(matches!(len, 1 | 2 | 4 | 8), "watch len must be 1/2/4/8, got {len}");
        assert_eq!(va % len, 0, "watch va {va:#x} must be {len}-aligned");
        let (wvr, wcr) = HW_WATCHPOINT_SLOTS.get(slot)
            .copied()
            .unwrap_or_else(|| panic!("HW watchpoint slot {slot} out of range (0..=3)"));
        let bas = ((1u64 << len) - 1) << (va & 7);
        self.vcpu.set_sys(wvr, va & !7).unwrap();
        self.vcpu.set_sys(wcr, DBGWCR_BASE | (bas << 5)).unwrap();
        self.watch_ranges.push((va, len));
        self.wps_armed = true;
        self.sync_mde();
    }

    /// Disarm every hardware watchpoint slot and forget the watch ranges, recomputing MDE (which is
    /// shared with breakpoints — clearing one side must not disarm the other).
    pub fn clear_hw_watchpoints(&mut self) {
        for (wvr, wcr) in HW_WATCHPOINT_SLOTS {
            self.vcpu.set_sys(wvr, 0).unwrap();
            self.vcpu.set_sys(wcr, 0).unwrap();
        }
        self.watch_ranges.clear();
        self.syscall_watch_hit = None;
        self.wps_armed = false;
        self.sync_mde();
    }

    /// MDSCR_EL1.MDE gates breakpoints AND watchpoints; keep it set iff either side is armed.
    fn sync_mde(&mut self) {
        let mdscr = self.vcpu.get_sys(sysreg::MDSCR_EL1).unwrap();
        let v = if self.bps_armed || self.wps_armed { mdscr | MDSCR_MDE } else { mdscr & !MDSCR_MDE };
        self.vcpu.set_sys(sysreg::MDSCR_EL1, v).unwrap();
    }

    /// Test-only: arm `MDSCR_EL1.SS` + `PSTATE.SS` the way `step()` does, then leave — so a
    /// following `run()` drives the vcpu with SS armed and must hit the fail-loud `Ec::SoftStep`
    /// arm. Proves SS never silently leaks past `step()`.
    #[doc(hidden)]
    pub fn dbg_leak_ss(&mut self) {
        let mdscr = self.vcpu.get_sys(sysreg::MDSCR_EL1).unwrap();
        self.vcpu.set_sys(sysreg::MDSCR_EL1, mdscr | MDSCR_SS).unwrap();
        let cpsr = self.vcpu.get_reg(reg::CPSR).unwrap();
        self.vcpu.set_reg(reg::CPSR, cpsr | PSTATE_SS).unwrap();
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
        vcpu.set_sys(sysreg::TTBR0_EL1, PT_L1_IPA).unwrap();
        vcpu.set_sys(sysreg::CPACR_EL1, CPACR_FP_ON).unwrap(); // match load: EL0/EL1 FP/SIMD enabled
        vcpu.set_sys(sysreg::TPIDRRO_EL0, TSD_IPA).unwrap();   // match load: thread pointer (harmless for M1)
        vcpu.set_sys(sysreg::TPIDR_EL0,   0).unwrap();         // match load: cpu 0 / cluster 0 (M2-cpuid)
        Self::set_pac_keys(&vcpu);
        let pac = pac_posture_from_memory(regions);
        // M8-stack: DERIVED, not hardcoded — restore() rebuilds the static and the dynamic path
        // alike, and the replay mirror byte-compares the reply it recomputes from this geometry.
        let (stack_top, stack_size) = stack_geometry_from_memory(regions);
        vcpu.set_sys(sysreg::SCTLR_EL1, sctlr_mmu_on(pac)).unwrap(); // MMU on (tables from snapshot)
        vcpu.set_sys(sysreg::VBAR_EL1, TRAMPOLINE_IPA).unwrap();
        vcpu.set_trap_debug_exceptions(true).unwrap();          // route SS/breakpoint exits to the VMM (Box_::step)
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
        // reservations reset to empty here (mirroring mmap_next: MMAP_BASE) so replay's demand-commit
        // address sequence matches record's from a clean slate.
        Box_ { vm, vcpu, backings, reservations: Vec::new(), noaccess: Vec::new(), mmap_next: MMAP_BASE, bootstrap_port: None, l2_host, next_l3, last_far: 0, synthetic_tsc: SYNTH_TSC_START, cache_refault_ipa: 0, cache_refault_count: 0, cache: None, bps_armed: false, wps_armed: false, watch_ranges: Vec::new(), syscall_watch_hit: None, pac_enabled: pac, stack_top, stack_size, tlbi_stub_ready: false, fds: FdTable::new(), sigtable: SigTable::default(), threads: thread::ThreadTable::new(thread::ThreadCtx::zeroed()), thread_start_pc: None, wq_thread_pc: None, pthread_size: None }
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

    /// Complete a serviced syscall AND bring the saved trap state (SPSR_EL1) along, for the one
    /// caller that needs it: a signal delivered before the guest resumes from that syscall.
    ///
    /// `deliver_signal` reads the frame's GPRs live but its PSTATE from **SPSR_EL1**, which
    /// `set_x0_err_and_return` never touches — it writes `reg::CPSR`, the register the vCPU
    /// actually resumes from. On every other path that gap is invisible: nothing reads SPSR_EL1
    /// before the next trap overwrites it. On the caught-raise path it is the difference between
    /// a frame that says the syscall succeeded and one that says it failed.
    ///
    /// Measured against the real kernel (`spikes/sigraisex0.c`), not inferred: a process that
    /// raises a signal on itself enters its handler with x0 = the syscall's RETURN value (0), not
    /// the pid it passed, and with PSTATE.C **clear**. The probe set C=1 and Z=1 immediately
    /// before `kill()`; the frame came back `0x40000000` — Z survived, C did not. The kernel
    /// snapshots the context *after* completing the return, so retrace must too, or the guest
    /// resumes from its own successful raise reading it as a failure.
    pub fn complete_syscall_before_delivery(&mut self, ret: u64, err: bool) {
        self.set_x0_err_and_return(ret, err);
        let spsr = self.vcpu.get_sys(sysreg::SPSR_EL1).unwrap();
        let spsr = (spsr & !retrace_arch::PSTATE_C) | if err { retrace_arch::PSTATE_C } else { 0 };
        self.vcpu.set_sys(sysreg::SPSR_EL1, spsr).unwrap();
    }

    /// `complete_syscall_before_delivery`'s sibling for the one caller that needs the SAME
    /// correction applied to a thread that is not live: M17's wake-materialised delivery. That
    /// arm builds the receiver's frame from its *saved* context, never the vCPU — the receiver is
    /// Blocked, not current — so there is no `reg::CPSR`/`SPSR_EL1` to patch, only `ctx.spsr`.
    ///
    /// Measured, not inferred, on both ends of the gap this closes. The real kernel: the
    /// `sigraisex0` probe (cited above) delivered `0x40000000` for a successful self-raise — C
    /// **clear** — because it snapshots PSTATE *after* completing the return, not before. Task 4b
    /// measured what a Wait-blocked thread's saved SPSR holds instead: `0x60000000`
    /// (`crates/retrace/tests/blockedctx.rs`) — C **set**, on a thread whose saved x0 is already 0,
    /// i.e. a successful `__ulock_wait`. That saved SPSR is untouched raw exception-entry state
    /// (nothing on the wake path patches it), so it disagrees with the completed return sitting
    /// right next to it in x0. Left alone, `sigreturn` would restore that carry bit and the guest
    /// would resume its successful wait reading it as a failure — the same bug
    /// `complete_syscall_before_delivery` exists to prevent on the live path, here on the saved one.
    ///
    /// No `x0` write, deliberately: unlike the live version, this has no `set_x0_err_and_return`
    /// call. Task 1 already measured the receiver's saved x0 as 0 — the completed return value —
    /// so only the PSTATE axis is wrong; touching x0 again would be redundant at best.
    pub fn complete_saved_syscall_before_delivery(&mut self, tid: usize, err: bool) {
        let ctx = self.threads.ctx_mut(tid);
        ctx.spsr = (ctx.spsr & !retrace_arch::PSTATE_C) | if err { retrace_arch::PSTATE_C } else { 0 };
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
    /// Rewrite every guest fd operand of `num` in `args` to its host fd, in place.
    ///
    /// Called from the TWO places that consume a guest fd — here in `forward_and_diff` and in
    /// `guest_mmap_file`. It is deliberately not "a single choke point inside forward_and_diff":
    /// file-backed mmap is special-cased upstream in retrace-core and preads from its fd without
    /// ever reaching forward_and_diff, so a one-site design would leak exactly that one.
    ///
    /// `Err(EBADF)` means the guest named an fd it does not have open. The caller forwards NOTHING —
    /// the whole point is that the number may be a live descriptor of retrace's own.
    pub fn translate_fds(&self, num: u64, args: &mut [u64; 8]) -> Result<(), u64> {
        for &i in retrace_arch::fd_operands(num) {
            let v = args[i];
            // AT_FDCWD (-2) and friends are sentinels, not descriptors.
            if (v as i64) < 0 { continue; }
            match self.fds.host(v) {
                Some(h) => args[i] = h as u64,
                // Console fds (0/1/2) have no host mapping and never reach here: retrace-core
                // mirrors their writes and fakes their close before forwarding is considered.
                None => return Err(EBADF),
            }
        }
        Ok(())
    }

    /// Bind an `allocates_fd` syscall's host return value to a fresh guest slot, returning the
    /// GUEST fd — the number that goes into the trace and back to the guest.
    pub fn bind_returned_fd(&mut self, num: u64, host_ret: u64) -> u64 {
        debug_assert!(retrace_arch::allocates_fd(num), "syscall {num} does not return an fd");
        let gfd = self.fds.alloc();
        self.fds.bind(gfd, host_ret as i32);
        gfd
    }

    pub fn fds(&self) -> &FdTable { &self.fds }
    pub fn fds_mut(&mut self) -> &mut FdTable { &mut self.fds }
    pub fn sigtable(&self) -> &SigTable { &self.sigtable }
    pub fn sigtable_mut(&mut self) -> &mut SigTable { &mut self.sigtable }

    /// `map_with_linking_np`'s fd lives INSIDE guest memory, not in a register: x0 points at a
    /// `struct mwl_region[]` (x1 = count) whose first field is `mwlr_fd`. No operand index can name
    /// it, so `fd_operands` returns nothing for 550 and this handles it instead.
    ///
    /// The array is `const` — the kernel reads it and never writes it — so translation copies the
    /// regions into a HOST-side buffer and points the forwarded x0 at that copy. Guest memory is
    /// never mutated, which is what keeps a host fd from leaking into the trace as recorded data.
    /// Returns the buffer (kept alive across the syscall by the caller) or `Err(EBADF)`.
    fn translate_mwl_regions(&self, args: &[u64; 8]) -> Result<Vec<u8>, u64> {
        let count = args[1].min(retrace_arch::MWL_MAX_REGION_COUNT);
        let stride = retrace_arch::MWL_REGION_STRIDE;
        let mut buf = self.read_guest(args[0], count as usize * stride);
        for r in 0..count as usize {
            let off = r * stride;
            let gfd = i32::from_le_bytes(buf[off..off + 4].try_into().unwrap());
            if gfd < 0 { continue; }
            let h = self.fds.host(gfd as u64).ok_or(EBADF)?;
            buf[off..off + 4].copy_from_slice(&h.to_le_bytes());
        }
        Ok(buf)
    }

    /// Forward one guest syscall to the host kernel and capture what it wrote into guest memory.
    ///
    /// **M10 makes this the whole fd contract, deliberately.** It takes GUEST descriptors, and the
    /// `(u64, bool, Vec<Region>)` it returns already carries a GUEST descriptor when the syscall
    /// produced one. Translation-in and binding-out used to live on opposite sides of a crate
    /// boundary (translation here, binding in retrace-core's dispatch), which meant every other
    /// caller had to remember the second half — and `memdiff`'s mini record loop did not, so its
    /// guest's `open` returned an unbound host fd and its `read` came back EBADF. One function owns
    /// both halves now; a caller cannot hold it wrong.
    pub fn forward_and_diff(&mut self, num: u64, args: [u64;8]) -> (u64, bool, Vec<Region>) {
        // The guest's own view of the operands, kept for the fd bookkeeping below: `args` is about
        // to be rewritten to host descriptors, and `close` must retire the GUEST slot.
        let gargs = args;
        // M10: translate guest fds to host fds BEFORE anything else. A guest fd that reaches the
        // host kernel untranslated acts on RETRACE's descriptor of that number.
        let mut args = args;
        if let Err(e) = self.translate_fds(num, &mut args) {
            return (e, true, Vec::new());
        }
        // map_with_linking_np: fd inside a guest struct; forward a translated host-side copy.
        let mwl = if num == retrace_arch::SYS_MAP_WITH_LINKING_NP {
            match self.translate_mwl_regions(&args) {
                Ok(b) => Some(b),
                Err(e) => return (e, true, Vec::new()),
            }
        } else { None };
        let mut windows: Vec<(u64, usize, Vec<u8>)> = Vec::new(); // (guest_ipa, len, pre-image)
        let mut hargs = [0i64; 8];
        for i in 0..8 {
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
        //
        // M10: `read_nocancel` (396) belongs here too and was missing — the same plain-vs-_nocancel
        // trap as M9's console bug, in the clamp rather than in a predicate. `jq` reads through 396
        // and never through 3, so before M10 its reads were forwarded UNCLAMPED.
        if num == retrace_arch::SYS_READ || num == retrace_arch::SYS_PREAD
            || num == retrace_arch::SYS_READ_NOCANCEL {
            let count = args[2] as usize;
            hargs[2] = match self.host_span(args[1]) {
                Some((_, avail)) => Self::clamp_count(avail, count) as i64,
                None => count as i64,
            };
        }
        // M10: forward the TRANSLATED copy of map_with_linking_np's region array, not the guest's
        // (whose mwlr_fd fields still hold guest fds, and must keep holding them).
        if let Some(b) = mwl.as_ref() { hargs[0] = b.as_ptr() as i64; }
        // Forward via a raw `svc #0x80` shim (not `libc::syscall`, which narrows the return
        // toward 32 bits and hides the BSD carry flag). `hargs` is [i64;8] (x0..x7); no more x7
        // padding.
        let mut sa = [0u64; 8];
        for i in 0..8 { sa[i] = hargs[i] as u64; }
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
        // M10 fd bookkeeping — the other half of the contract, deliberately here rather than in the
        // caller (see this function's doc comment).
        //
        // An fd-producing syscall returned a HOST descriptor: bind it to a guest slot and hand back
        // the GUEST number, so what reaches both the guest and the trace is a function of the
        // guest's own open/close sequence rather than of how many files retrace holds open.
        let ret = if !err && retrace_arch::allocates_fd(num) {
            self.bind_returned_fd(num, ret)
        } else { ret };
        // A successful close retires the guest's slot, so a later use of that number is EBADF
        // instead of reaching whatever retrace has open there. fd 0/1/2 never arrive here —
        // is_console_close fakes them upstream in retrace-core.
        if !err && (num == retrace_arch::SYS_CLOSE || num == retrace_arch::SYS_CLOSE_NOCANCEL) {
            self.fds.close(gargs[0]);
        }
        (ret, err, writes)
    }

    /// Replay-side: apply recorded writes to guest memory, then resume. Never executes a syscall.
    pub fn apply_and_return(&mut self, ret: u64, err: bool, writes: &[Region]) {
        for w in writes {
            // M5: watched-range intersection (observation only — the copy below is unconditional).
            // Empty watch_ranges on record and plain replay => this is a no-op is_empty check there.
            if self.syscall_watch_hit.is_none() && !self.watch_ranges.is_empty() {
                let end = w.ipa + w.bytes.len() as u64;
                if let Some(&(va, _)) = self.watch_ranges.iter().find(|&&(va, len)| {
                    // M6: translate the armed VA through the guest's own tables (identity when the
                    // MMU is off); an unmapped VA translates to None and cannot match.
                    self.va_to_ipa(va).is_some_and(|ipa| w.ipa < ipa + len && ipa < end)
                })
                {
                    self.syscall_watch_hit = Some((va, w.ipa));
                }
            }
            self.write_guest(w.ipa, &w.bytes);
        }
        self.set_x0_err_and_return(ret, err);
    }

    /// Copy `bytes` into guest memory at `ipa`. The write path `apply_and_return` uses, minus the
    /// syscall return — a delivered signal is not a syscall and must not set `x0`.
    ///
    /// Deliberately does NOT do the M5 watched-range check: `syscall_watch_hit` names the
    /// syscall-write path specifically, and a signal frame is not a guest store. Delivery landing on
    /// a watched range is a separate question from "which syscall wrote here", and conflating them
    /// would make a watchpoint fire with a syscall's provenance for a write no syscall made.
    fn write_guest(&mut self, ipa: u64, bytes: &[u8]) {
        let (hp, avail) = self.host_span(ipa)
            .unwrap_or_else(|| panic!("write_guest: ipa {ipa:#x} outside any mapped region"));
        assert!(bytes.len() <= avail,
            "write_guest at {ipa:#x} ({} bytes) overruns backing ({avail} avail)", bytes.len());
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), hp, bytes.len()); }
    }

    /// Is thread `tid` currently executing on ITS OWN alternate signal stack?
    ///
    /// Same current/non-current discipline as `pthread_of` and `dbg_regs_of`: the running thread's
    /// stack pointer is live in `SP_EL0` and its table entry is stale between switches, so only a
    /// NON-current thread is read from the table. That distinction is load-bearing rather than
    /// stylistic — inside `deliver_signal_to` the current thread's ctx has just been saved, so the
    /// table would be right there, but `on_altstack()` called from anywhere else has had no such save
    /// and reading the table unconditionally would answer from stale state.
    pub fn on_altstack_of(&self, tid: usize) -> bool {
        let sp = if tid == self.threads.current() {
            self.vcpu.get_sys(sysreg::SP_EL0).unwrap()
        } else {
            self.threads.ctx_of(tid).regs.sp_el0
        };
        matches!(self.threads.altstack_of(tid), Some((base, size, _)) if sp >= base && sp < base + size)
    }

    /// Is the RUNNING thread's stack pointer inside its own alternate stack? The `_of` form takes
    /// any tid; this one is the common case. Callers use it to decide whether a further
    /// `SA_ONSTACK` delivery would re-enter a stack the guest is already on, which POSIX says it
    /// must not — see `choose_frame_base`, which is the only real consumer.
    pub fn on_altstack(&self) -> bool { self.on_altstack_of(self.threads.current()) }

    /// The saved PSTATE at the current trap (SPSR_EL1) — the sibling of `position()`'s ELR_EL1.
    pub fn spsr(&self) -> u64 { self.vcpu.get_sys(sysreg::SPSR_EL1).unwrap() }

    /// Test/diagnostic accessors for vector and general registers. Present so the M12 delivery
    /// tests can clobber and observe state without a running guest.
    pub fn vcpu_set_x(&mut self, n: u32, v: u64) { self.vcpu.set_reg(reg::x(n), v).unwrap(); }
    pub fn vcpu_get_x(&self, n: u32) -> u64 { self.vcpu.get_reg(reg::x(n)).unwrap() }
    /// Test-only: `reg::FP`/`reg::LR` straight off the vCPU, so a test can pin that they alias X29/X30.
    #[doc(hidden)]
    pub fn dbg_fp_lr(&self) -> (u64, u64) {
        (self.vcpu.get_reg(reg::FP).unwrap(), self.vcpu.get_reg(reg::LR).unwrap())
    }
    pub fn vcpu_set_q(&mut self, n: u32, v: u128) { self.vcpu.set_simd(simd::q(n), v).unwrap(); }
    pub fn vcpu_get_q(&self, n: u32) -> u128 { self.vcpu.get_simd(simd::q(n)).unwrap() }
    /// M14 t5: setters for the sysregs a context switch must move, paired with the existing
    /// getters `position()` (ELR_EL1), `spsr()` (SPSR_EL1), and `tpidrro_el0()` below.
    pub fn set_elr(&mut self, v: u64) { self.vcpu.set_sys(sysreg::ELR_EL1, v).unwrap(); }
    pub fn set_spsr(&mut self, v: u64) { self.vcpu.set_sys(sysreg::SPSR_EL1, v).unwrap(); }
    pub fn set_tpidrro_el0(&mut self, v: u64) { self.vcpu.set_sys(sysreg::TPIDRRO_EL0, v).unwrap(); }
    /// Write raw bytes into guest memory (tests only — the production path is `write_guest`).
    pub fn poke_guest(&mut self, ipa: u64, bytes: &[u8]) { self.write_guest(ipa, bytes) }

    /// The inverse of [`deliver_signal`](Self::deliver_signal): read the mcontext back out of guest
    /// memory and resume the interrupted context. Called by BOTH record and replay.
    ///
    /// Resumes via `reg::PC`/`reg::CPSR`, exactly as `deliver_signal` enters — the vCPU is parked at
    /// the `sigreturn` trap and resumes from those registers. A caller must therefore NOT follow
    /// this with `set_x0_err_and_return`: `sigreturn` does not return a value, and doing so would
    /// overwrite the `x0` just restored from the frame.
    pub fn sigreturn_restore(&mut self, uctx_ipa: u64, token: u64) {
        let expected = sigreturn_token(uctx_ipa);
        assert_eq!(token, expected,
            "sigreturn token mismatch: got {token:#x}, expected {expected:#x} for uctx {uctx_ipa:#x}. \
             The token is a pure function of the ucontext address, so a mismatch means a corrupted \
             frame or a sigreturn the guest reached without a delivery.");

        let uc = self.read_guest(uctx_ipa, 56);
        let mask = u32::from_le_bytes(uc[4..8].try_into().unwrap());
        let mc_ipa = u64::from_le_bytes(uc[48..56].try_into().unwrap());
        let mc = self.read_guest(mc_ipa, 816);
        let ts = &mc[16..];

        for i in 0..29u32 {
            let o = i as usize * 8;
            self.vcpu.set_reg(reg::x(i), u64::from_le_bytes(ts[o..o + 8].try_into().unwrap())).unwrap();
        }
        self.vcpu.set_reg(reg::FP, u64::from_le_bytes(ts[232..240].try_into().unwrap())).unwrap();
        self.vcpu.set_reg(reg::LR, u64::from_le_bytes(ts[240..248].try_into().unwrap())).unwrap();
        self.vcpu.set_sys(sysreg::SP_EL0, u64::from_le_bytes(ts[248..256].try_into().unwrap())).unwrap();
        let pc = u64::from_le_bytes(ts[256..264].try_into().unwrap());
        let cpsr = u32::from_le_bytes(ts[264..268].try_into().unwrap()) as u64;

        let ns = &mc[288..];
        for i in 0..32u32 {
            let o = i as usize * 16;
            self.vcpu.set_simd(simd::q(i), u128::from_le_bytes(ns[o..o + 16].try_into().unwrap())).unwrap();
        }
        self.vcpu.set_reg(reg::FPSR, u32::from_le_bytes(ns[512..516].try_into().unwrap()) as u64).unwrap();
        self.vcpu.set_reg(reg::FPCR, u32::from_le_bytes(ns[516..520].try_into().unwrap()) as u64).unwrap();

        self.vcpu.set_reg(reg::PC, pc).unwrap();
        self.vcpu.set_reg(reg::CPSR, cpsr & PSTATE_USER_MASK).unwrap();
        // M16: the RETURNING (current) thread's mask — sigreturn always resumes the thread it was
        // called on.
        let cur = self.threads.current();
        self.threads.set_mask_of(cur, retrace_arch::SIG_SETMASK, mask);
    }

    /// Can `tid` receive a signal frame? `Ok(())`, or `Err(diagnostic)` saying why not.
    ///
    /// A `Blocked` thread's ctx is the saved state its blocking syscall must resume through, so
    /// redirecting it overwrites that resume point out from under the syscall waiting on it. An
    /// `Exited` thread's ctx is a dead entry with nothing left to resume.
    ///
    /// Fast-follow: split out of `deliver_signal_to` for the same reason as `try_thread_of_port`.
    /// Record wants the panic — a target that cannot run means retrace's model is wrong. Replay
    /// wants a named `Divergence`, because there the same condition is the signature of a schedule
    /// divergence. One definition and two callers, so the two sides cannot drift on what
    /// "deliverable" means; `deliver_signal_to` itself still calls this and still panics, so its
    /// behaviour and its message are unchanged.
    pub fn check_deliverable(&self, tid: usize) -> Result<(), String> {
        match self.threads.state_of(tid) {
            thread::ThreadState::Runnable => Ok(()),
            thread::ThreadState::Blocked(reason) => Err(format!(
                "thread {tid} is Blocked({reason:?}), not Runnable; deliver_signal_to would \
                 overwrite the saved context its blocking syscall must resume through. Wake or \
                 skip it instead of redirecting a thread that cannot run yet.")),
            thread::ThreadState::Exited(code) => Err(format!(
                "thread {tid} has Exited({code}), not Runnable; it has no saved context left to \
                 resume into a handler. deliver_signal_to must not target a dead thread.")),
        }
    }

    /// M17: fail loud if the guest is exiting with a signal pended on a thread that can never take
    /// it. Pend-until-wake delivers a pended signal at the wake that makes its thread runnable — so
    /// a thread still `Blocked` at exit, holding a signal its mask does not block, never got it.
    ///
    /// This is the semantic gap between pend-until-wake and POSIX, which would interrupt the wait.
    /// Named rather than hidden: exiting 0 while swallowing a signal makes record and replay agree
    /// with each other and both be wrong, which is the one failure a determinism oracle cannot see.
    ///
    /// Clean-exit path only. A guest that is already crashing must be diagnosed by its crash, not
    /// by a secondary guard firing on top of it.
    pub fn assert_no_stranded_signals(&self) {
        for tid in 0..self.threads.len() {
            if !matches!(self.threads.state_of(tid), thread::ThreadState::Blocked(_)) { continue; }
            if let Some(sig) = self.threads.peek_deliverable(tid) {
                panic!("thread {tid} is exiting still Blocked with pending signal {sig}, which it \
                        can therefore never take: M17 materialises a pended signal at the WAKE, and \
                        this thread was never woken. Either the guest deadlocked, or the signal \
                        needs the wait to be interrupted (EINTR) rather than deferred — which M17 \
                        deliberately does not model. State: {:?}", self.threads.state_of(tid));
            }
        }
    }

    /// M17: should a raise targeting `tid` PEND rather than deliver?
    ///
    /// Two reasons, and they are independent. The mask reason is M16's and unchanged. The state
    /// reason is M17's: a BLOCKED target cannot be redirected into a handler yet — its saved
    /// context is the resume point its blocking syscall owes a return through — so the signal waits
    /// on its pending set and is materialised at the wake instead.
    ///
    /// `Blocked(_)` specifically, NOT "anything but `Runnable`". An `Exited` target must keep
    /// reaching `check_deliverable`'s refusal: the spec keeps that arm a panic because a signal to
    /// a dead thread is a modelling bug rather than a schedule divergence, and there is no wake to
    /// materialise it at. Pending on a dead thread would swallow the signal in silence instead —
    /// `assert_no_stranded_signals` scans `Blocked` threads only — which is the one failure shape a
    /// determinism oracle cannot see.
    ///
    /// This is the predicate BOTH dispatch loops consult, written once so they cannot drift on the
    /// pend-vs-deliver decision while both stayed green. `take_deliverable` already filters by
    /// mask, so a signal pended for both reasons is released only when both have cleared.
    ///
    /// Note the relationship to `check_deliverable`: this decides whether to pend, that decides
    /// whether a delivery may proceed. They agree on `Runnable` by construction — which is why the
    /// raise path can no longer reach that guard's refusal for a merely-blocked target.
    pub fn should_pend_for(&self, tid: usize, sig: u64) -> bool {
        self.threads.is_blocked_for(tid, sig)
            || matches!(self.threads.state_of(tid), thread::ThreadState::Blocked(_))
    }

    /// Deliver `sig` to thread `tid`, which need NOT be the running one.
    ///
    /// Returns `(frame writes, resume_pc)`. Called by BOTH record and replay — that is what makes
    /// "both sides recompute the same frame" true by construction rather than by discipline.
    ///
    /// `esr`/`far` are the guest's own fault syndrome for a fault-derived signal, and 0 for a
    /// self-raise (no hardware fault happened, and inventing one would be the same lie M11 refused
    /// when it kept `Event::Signal` out of `Event::Crash`).
    ///
    /// The table is the authority for EVERY thread, including the running one whose entry is stale
    /// between switches — so the current context is saved into it first and reloaded at the end. That
    /// is what lets one code path serve a self-signal and a cross-thread signal identically, instead
    /// of two paths that drift (M13 Task 8 shipped a test that checked only one of a mirrored pair).
    pub fn deliver_signal_to(
        &mut self, tid: usize, sig: u64, si_code: u64, si_addr: u64, esr: u64, far: u64,
    ) -> (Vec<Region>, u64) {
        let act = self.sigtable.action(sig);
        let cur = self.threads.current();
        *self.threads.ctx_mut(cur) = self.save_ctx();

        // M16 fix round 1 (review finding 5): the target must be schedulable. A Blocked thread's
        // ctx is the saved state its blocking syscall must resume through — redirecting it
        // overwrites that resume point out from under the syscall that is waiting on it. An
        // Exited thread's ctx is a dead entry with nothing left to resume. Neither is reachable
        // today (no product caller passes tid != cur), but the next task makes both reachable and
        // the failure mode would be silent corruption, not a panic — so check now, not then.
        self.check_deliverable(tid).unwrap_or_else(|d| panic!("{d}"));

        // M16: a second signal to a thread already redirected into an un-run handler would stack a
        // frame the kernel's queueing semantics would order — unmodelled, so fail loud rather than
        // guess. A self-signal (tid == cur) runs its handler immediately, so there is no un-run
        // frame to stack on and this does not apply.
        if tid != cur {
            assert!(!self.threads.is_redirected(tid),
                "thread {tid} was already redirected into a handler and has not run since; a \
                 second signal here would stack frames without the kernel's queueing semantics. \
                 Nested delivery is unmodelled — implement queueing before a guest needs this.");
        }

        let ctx = self.threads.ctx_of(tid).clone();
        let mut x = [0u64; 29];
        x.copy_from_slice(&ctx.regs.x[..29]);
        let ts = ThreadState {
            x,
            fp: ctx.regs.x[29],
            lr: ctx.regs.x[30],
            // ELR, not regs.pc: regs.pc is where the vCPU RESUMES (the trampoline, at a trap), while
            // ELR_EL1 is the guest's own next instruction — `position()`'s source, and what sigreturn
            // must come back to. Likewise spsr, not regs.cpsr.
            sp: ctx.regs.sp_el0,
            pc: ctx.elr,
            cpsr: ctx.spsr,
        };
        let ns = NeonState { v: ctx.fp, fpsr: ctx.fpsr as u32, fpcr: ctx.fpcr as u32 };

        // The TARGET's alt stack, and whether the TARGET is already on it — not the running thread's.
        let alt = self.threads.altstack_of(tid);
        let (frame_base, on_alt) = choose_frame_base(ts.sp, act, alt, self.on_altstack_of(tid));

        // `on_alt` is FED BACK from `choose_frame_base` rather than recomputed here, so the frame's
        // `uc_onstack` cannot disagree with the stack the frame was actually placed on — this is
        // the whole reason `choose_frame_base` returns a second value at all.
        let inp = FrameInput {
            sig, si_code, si_addr, esr, far, ts, ns,
            mask: self.threads.mask_of(tid),   // the PRE-signal mask: what sigreturn restores
            act, frame_base, on_alt,
        };
        let (bytes, entry) = build_frame(&inp);
        self.write_guest(frame_base, &bytes);

        // Block for the handler's duration, unless SA_NODEFER — on the RECEIVING thread.
        let mut newmask = self.threads.mask_of(tid) | act.mask;
        if act.flags & retrace_arch::SA_NODEFER == 0 { newmask |= 1 << (sig - 1); }
        self.threads.set_mask_of(tid, retrace_arch::SIG_SETMASK, newmask);
        // SA_RESETHAND changes a DISPOSITION, which stays process-global.
        if act.flags & retrace_arch::SA_RESETHAND != 0 {
            self.sigtable.set_action(sig, SigAction { disp: Disposition::Dfl, ..act });
        }

        {
            let c = self.threads.ctx_mut(tid);
            for (i, xi) in entry.x.iter().enumerate() { c.regs.x[i] = *xi; }
            c.regs.sp_el0 = entry.sp;
            // The vCPU resumes at reg::PC, which load_ctx writes from regs.pc — so the trampoline
            // address goes THERE, exactly as the pre-M16 code wrote reg::PC and not ELR_EL1.
            c.regs.pc = entry.pc;
            // CPSR comes from ctx.spsr (SPSR_EL1), not ctx.regs.cpsr, so the handler runs at EL0.
            // This is now MORE subtle than the pre-M16 line it replaced: for a non-current target,
            // ctx.regs.cpsr is that thread's own EL1 TRAP pstate (the vCPU's live CPSR while the
            // box was handling the trap that captured this context), not the guest's EL0 state —
            // using that field here would resume the handler at EL1.
            c.regs.cpsr = ctx.spsr;
        }
        self.threads.set_redirected(tid, tid != cur);

        let back = self.threads.ctx_of(cur).clone();
        self.load_ctx(&back);

        (vec![Region { ipa: frame_base, bytes }], ts.pc)
    }

    /// M11/M12's entry point, unchanged for every existing caller: deliver to the running thread.
    pub fn deliver_signal(
        &mut self, sig: u64, si_code: u64, si_addr: u64, esr: u64, far: u64,
    ) -> (Vec<Region>, u64) {
        self.deliver_signal_to(self.threads.current(), sig, si_code, si_addr, esr, far)
    }

    /// Take (and clear) the syscall-write watch hit recorded by `apply_and_return` this event.
    pub fn take_syscall_watch_hit(&mut self) -> Option<(u64, u64)> { self.syscall_watch_hit.take() }

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

    /// Like `read_guest`, but returns None instead of panicking when the full `[ipa, ipa+len)` span
    /// does not fit inside a single backing — deterministic all-or-nothing (no clamping, no partial
    /// read). For callers (the M3 debugger's memory reads) that must tolerate unmapped/partial spans;
    /// `read_guest`'s panic stays load-bearing fail-loud for internal callers.
    pub fn read_guest_checked(&self, ipa: u64, len: usize) -> Option<Vec<u8>> {
        for bk in &self.backings {
            if ipa >= bk.ipa && ipa + len as u64 <= bk.ipa + bk.len as u64 {
                let off = (ipa - bk.ipa) as usize;
                return Some(unsafe { std::slice::from_raw_parts(bk.host.add(off), len) }.to_vec());
            }
        }
        None
    }

    /// The kernel writes `sysctl({CTL_KERN, KERN_USRSTACK64})` would perform, computed from the
    /// guest's OWN stack top instead of retrace's (M8-stack). PURE — it only builds the regions;
    /// the caller applies them with `apply_and_return`, exactly like the `shared_region_check_np`
    /// reply. That is what lets replay recompute and byte-compare BEFORE touching guest memory.
    ///
    /// `args` is the raw syscall frame: `sysctl(name, namelen, oldp, oldlenp, newp, newlen)`.
    /// `oldp == 0` is a size probe — only `*oldlenp` is reported. A `NULL` `oldlenp` (the guest
    /// wants nothing back) yields no regions at all.
    pub fn usrstack64_reply(&self, args: [u64; 8]) -> Vec<Region> {
        let (oldp, oldlenp) = (args[2], args[3]);
        let mut out = Vec::new();
        if oldp != 0 { out.push(Region { ipa: oldp, bytes: self.stack_top.to_le_bytes().to_vec() }); }
        if oldlenp != 0 { out.push(Region { ipa: oldlenp, bytes: 8u64.to_le_bytes().to_vec() }); }
        out
    }

    /// Answer `getrlimit(RLIMIT_STACK)` from the guest's own stack size. `struct rlimit` is two
    /// u64s: `{ rlim_cur, rlim_max }`. Both report the real mapped size — retrace does not grow the
    /// guest stack on demand, so a larger `rlim_max` would be a lie the guest could act on (libstd
    /// subtracts this from `kern.usrstack64` to locate its guard page, so the two must describe the
    /// SAME stack). Pure builder: the caller applies via `apply_and_return`, like `usrstack64_reply`.
    pub fn rlimit_stack_reply(&self, args: [u64; 8]) -> Vec<Region> {
        let rlp = args[1];
        if rlp == 0 { return Vec::new(); }
        let mut bytes = self.stack_size.to_le_bytes().to_vec();
        bytes.extend_from_slice(&self.stack_size.to_le_bytes());
        vec![Region { ipa: rlp, bytes }]
    }

    /// Read-only stage-1 walk of the guest's OWN page tables: VA -> IPA. MMU off => identity.
    /// None if unmapped at any level or beyond the 47-bit VA space — an unmapped VA cannot be
    /// the destination of an applied syscall write, so the watch check treats None as no-match.
    /// (Today every retrace mapping is identity; this makes the software watch check sound by
    /// construction rather than by that accident. M6.)
    ///
    /// The index shifts mirror `build_tables` exactly: TCR_EL1.T0SZ=17 (47-bit VA) + 16 KiB granule
    /// is a 3-level walk of 2048-entry tables — L1 [46:36] (one entry per 64 GiB), L2 [35:25] (one
    /// per `BLK` = 32 MiB), L3 [24:14] (one per `GRANULE`). With a 16 KiB granule a BLOCK descriptor
    /// is architecturally permitted only at L2, so L1 must be a table.
    ///
    /// TBI0 is set in `TCR_EL1_V`, but this walk does NOT strip a top-byte tag: a tagged VA has
    /// bits above 47 and returns None. Conservative (a spurious no-match, never a spurious match),
    /// and no caller produces one — the debugger's `watch` takes a plain address.
    pub fn va_to_ipa(&self, va: u64) -> Option<u64> {
        const PT_ADDR: u64 = 0x0000_FFFF_FFFF_C000; // descriptor output-address bits 47:14
        let sctlr = self.vcpu.get_sys(sysreg::SCTLR_EL1).unwrap();
        if sctlr & 1 == 0 { return Some(va); }
        if va >> 47 != 0 { return None; }
        let l1e = self.pt_entry(PT_L1_IPA, (va >> 36) & 0x7FF)?;
        if l1e & 0x3 != DESC_TABLE { return None; }
        let l2e = self.pt_entry(l1e & PT_ADDR, (va >> 25) & 0x7FF)?;
        match l2e & 0x3 {
            DESC_BLOCK => Some((l2e & PT_ADDR & !(BLK - 1)) | (va & (BLK - 1))),
            DESC_TABLE => {
                let l3e = self.pt_entry(l2e & PT_ADDR, (va >> 14) & 0x7FF)?;
                if l3e & 0x3 != DESC_PAGE { return None; }
                Some((l3e & PT_ADDR) | (va & (GRANULE as u64 - 1)))
            }
            _ => None,
        }
    }

    /// One 64-bit descriptor from a guest page table (None if the table page is not backed).
    fn pt_entry(&self, table_ipa: u64, idx: u64) -> Option<u64> {
        let bytes = self.read_guest_checked(table_ipa + idx * 8, 8)?;
        Some(u64::from_le_bytes(bytes.try_into().unwrap()))
    }

    /// Read-only capture of the architectural GPR/PC/SP_EL0/CPSR state — the same register set
    /// `snapshot()` and `checkpoint()` embed. Test/diagnostic accessor (M9 t2): e.g.
    /// `flush_guest_tlb`'s test proves this is byte-identical across a stub run.
    pub fn regs_snapshot(&self) -> Regs {
        let mut x = [0u64; 31];
        for (i, xi) in x.iter_mut().enumerate() { *xi = self.vcpu.get_reg(reg::x(i as u32)).unwrap(); }
        Regs {
            x, pc: self.vcpu.get_reg(reg::PC).unwrap(),
            sp_el0: self.vcpu.get_sys(sysreg::SP_EL0).unwrap(),
            cpsr: self.vcpu.get_reg(reg::CPSR).unwrap(),
        }
    }

    /// Capture the running thread's context off the vCPU.
    ///
    /// Same save/restore discipline `flush_guest_tlb` (M9) and the PAC signing oracle already use;
    /// a context switch is that operation with the restore aimed at a DIFFERENT thread.
    pub fn save_ctx(&self) -> thread::ThreadCtx {
        let mut fp = [0u128; 32];
        for (i, f) in fp.iter_mut().enumerate() { *f = self.vcpu.get_simd(simd::q(i as u32)).unwrap(); }
        thread::ThreadCtx {
            regs: self.regs_snapshot(),
            fp,
            fpcr: self.vcpu.get_reg(reg::FPCR).unwrap(),   // Reg, not SysReg
            fpsr: self.vcpu.get_reg(reg::FPSR).unwrap(),   // Reg, not SysReg
            tpidrro_el0: self.vcpu.get_sys(sysreg::TPIDRRO_EL0).unwrap(),
            elr: self.vcpu.get_sys(sysreg::ELR_EL1).unwrap(),
            spsr: self.vcpu.get_sys(sysreg::SPSR_EL1).unwrap(),
        }
    }

    /// The inverse of `regs_snapshot`. The only new low-level helper M14 needs.
    fn write_regs(&mut self, r: &Regs) {
        for (i, xi) in r.x.iter().enumerate() { self.vcpu.set_reg(reg::x(i as u32), *xi).unwrap(); }
        self.vcpu.set_reg(reg::PC, r.pc).unwrap();
        self.vcpu.set_sys(sysreg::SP_EL0, r.sp_el0).unwrap();
        self.vcpu.set_reg(reg::CPSR, r.cpsr).unwrap();
    }

    /// Install a thread's context onto the vCPU.
    ///
    /// `TPIDR_EL0` is deliberately NOT touched: macOS 26 reads the guest's CPU number from its low
    /// bits and the cluster from the rest, so it must stay 0 for every thread. Writing a per-thread
    /// value there is how M2-cpuid's OOB seg-group index bug comes back.
    pub fn load_ctx(&mut self, ctx: &thread::ThreadCtx) {
        self.write_regs(&ctx.regs);
        for (i, f) in ctx.fp.iter().enumerate() { self.vcpu.set_simd(simd::q(i as u32), *f).unwrap(); }
        self.vcpu.set_reg(reg::FPCR, ctx.fpcr).unwrap();   // Reg, not SysReg
        self.vcpu.set_reg(reg::FPSR, ctx.fpsr).unwrap();   // Reg, not SysReg
        self.vcpu.set_sys(sysreg::TPIDRRO_EL0, ctx.tpidrro_el0).unwrap();
        self.vcpu.set_sys(sysreg::ELR_EL1, ctx.elr).unwrap();
        self.vcpu.set_sys(sysreg::SPSR_EL1, ctx.spsr).unwrap();
    }

    /// The guest's thread table. Read-only: the RUNNING thread's context here is stale between
    /// switches (the vCPU is the authority for that one), so consult `save_ctx` for live registers.
    pub fn threads(&self) -> &thread::ThreadTable { &self.threads }

    /// Mutable access, for the dispatch arms that spawn/block/exit a thread (Tasks 7–9) and for the
    /// tests. Does not touch the vCPU: changing `current` here without a `load_ctx` would leave the
    /// table and the hardware disagreeing — use `switch_to_thread` to move the vCPU.
    pub fn threads_mut(&mut self) -> &mut thread::ThreadTable { &mut self.threads }

    /// The guest VA of `tid`'s `pthread` struct, derived from its thread pointer.
    ///
    /// `TPIDRRO_EL0 = pthread + PTHREAD_TSD_OFF` is the kernel's convention, measured 4/4 for main and
    /// child alike (see `guest_bsdthread_create`). The CURRENT thread's table entry is stale between
    /// switches, so its thread pointer comes off the vCPU — the split `dbg_regs_of` established.
    pub fn pthread_of(&self, tid: usize) -> Option<u64> {
        if tid >= self.threads.len() { return None; }
        let tp = if tid == self.threads.current() {
            self.vcpu.get_sys(sysreg::TPIDRRO_EL0).unwrap()
        } else {
            self.threads.ctx_of(tid).tpidrro_el0
        };
        tp.checked_sub(PTHREAD_TSD_OFF)
    }

    /// `tid`'s mach-port name, read back out of its own `pthread` struct at `+0xf8`.
    ///
    /// READ, not reconstructed. For a thread retrace spawned this returns the
    /// `GUEST_THREAD_PORT_BASE | tid` it wrote itself; for main it returns what libpthread's
    /// `__pthread_main_thread_init` stored. Reading is what makes main need no special case.
    pub fn kport_of(&self, tid: usize) -> Option<u32> {
        let va = self.pthread_of(tid)?;
        let ipa = self.va_to_ipa(va + PTHREAD_KPORT_OFF)?;
        let b = self.read_guest_checked(ipa, 4)?;
        Some(u32::from_le_bytes(b.try_into().unwrap()))
    }

    /// Which thread owns `port`.
    ///
    /// FAIL-LOUD, not `Option`: the alternative is defaulting to the current thread, which is the
    /// exact latent defect M16 exists to close — `pthread_kill(child, sig)` running MAIN's handler
    /// on MAIN's stack, silently. A port retrace cannot place is a modelling gap that must surface
    /// as a panic.
    /// The fallible form: `Ok(tid)`, or `Err(diagnostic)` naming every thread searched.
    ///
    /// Fast-follow: this exists so the two dispatch loops can share the RESOLUTION while differing
    /// in how they react to it failing. On the record side a port that resolves to nothing means
    /// retrace's own model is wrong, so `thread_of_port` below still panics. On the replay side the
    /// same failure is the signature of a SCHEDULE divergence, and M16 prefers a named `Divergence`
    /// at a landmark to a process abort — the `pthread_kill` landmark's own `verify_thread` checks
    /// the CALLER, not the target, so it would not catch such a divergence first. Symmetry rule 1
    /// still holds where it bites: both sides run this identical scan with identical arguments, and
    /// only the reaction differs.
    pub fn try_thread_of_port(&self, port: u32) -> Result<usize, String> {
        // Exited threads are included in the diagnostic dump too (fix round 1, review finding 2),
        // tagged, so a port that belonged to a thread that has SINCE exited reads differently in
        // the diagnostic from a port retrace never issued at all — two different bugs to go looking
        // for. They are still excluded from MATCHING (`!exited &&` below): only a live thread can
        // claim a port. That acceptance behaviour is unchanged; only the diagnostic widened, per
        // the brief's own "measure before widening this" caution, which stays about what this
        // function accepts.
        let mut seen = Vec::new();
        for tid in 0..self.threads.len() {
            let exited = matches!(self.threads.state_of(tid), thread::ThreadState::Exited(_));
            let p = self.kport_of(tid);
            if !exited && p == Some(port) { return Ok(tid); }
            seen.push(format_seen_entry(tid, p, exited));
        }
        Err(format!("__pthread_kill names mach port {port:#x}, which belongs to no live guest \
                     thread (searched [{}]). Either the guest holds a port retrace never issued, \
                     or its pthread struct moved — measure before widening this.", seen.join(", ")))
    }

    /// Resolve a mach port to a thread, or panic. The RECORD-side form — see
    /// `try_thread_of_port` for why replay takes the fallible one instead.
    pub fn thread_of_port(&self, port: u32) -> usize {
        self.try_thread_of_port(port).unwrap_or_else(|d| panic!("{d}"))
    }

    /// The address the kernel enters a NEW thread at, learned from the guest's own
    /// `bsdthread_register` call — `None` until that call is observed.
    pub fn thread_start_pc(&self) -> Option<u64> { self.thread_start_pc }

    /// Record the trampoline address the guest registered with `bsdthread_register`. Called by
    /// retrace-core's dispatch (record AND replay — see the M14 Task 7 symmetry note on
    /// `guest_bsdthread_create`), which is why this is `pub`: retrace-box cannot call itself here.
    pub fn set_thread_start_pc(&mut self, pc: u64) { self.thread_start_pc = Some(pc); }

    /// `bsdthread_register(threadstart, wqthread, pthsize, …)`, emulated.
    ///
    /// **Never forwarded, as of M18.** Two independent reasons, and the second is the one that
    /// makes this urgent rather than tidy:
    ///
    /// 1. The host is retrace's OWN process, which registered its own libpthread at startup.
    ///    MEASURED (Task 1): forwarding this call returns `ret=0x16 err=true` — EINVAL, a genuine
    ///    host-call FAILURE, not merely a wrong-but-successful answer — because the host kernel
    ///    refuses a second registration for an already-registered process. So the guest's
    ///    `__pthread_supported_features` never gets set and the first `dispatch_async` trips a
    ///    `brk`. Third instance of one recurring bug: the guest's fds were retrace's (M10), its
    ///    dispositions were retrace's (M11), its pthread registration was retrace's (here).
    /// 2. `args[0]`/`args[1]` are thread ENTRY POINTS. Forwarding this call hands GUEST addresses
    ///    to the host kernel as **retrace's own** process's thread-start functions — the same
    ///    whole-process-fatal class as forwarding `bsdthread_create`. It has been harmless only
    ///    because it fails. Latent since M14; closed here.
    ///
    /// Does not replace `set_thread_start_pc`/`thread_start_pc` above: `retrace-core`'s dispatch
    /// and a dozen existing `retrace-box` tests still construct a box's thread-start state through
    /// that direct setter, and this method only WIDENS what a real `bsdthread_register` trap
    /// captures. Task 5 wires this method into the dispatch loops in its place.
    pub fn guest_bsdthread_register(&mut self, args: [u64; 8]) -> u64 {
        self.thread_start_pc = Some(args[0]);
        self.wq_thread_pc = Some(args[1]);
        self.pthread_size = Some(args[2] as u32);
        WORKQ_FEATURE_WORD as u64
    }

    /// M18 Stage 2 reads these; Stage 1 only captures them.
    pub fn wq_thread_pc(&self) -> Option<u64> { self.wq_thread_pc }
    pub fn pthread_size(&self) -> Option<u32> { self.pthread_size }

    /// `workq_open()` — the guest asks the kernel to bring up its workqueue.
    ///
    /// **Emulated, never forwarded.** Forwarding it brings up a real kernel workqueue for
    /// RETRACE's own process; combined with the `REQTHREADS` that follows, the host then creates a
    /// real worker thread inside the recorder and enters it at `start_wqthread`, which jumps
    /// through a dispatch function pointer that is NULL in this process and dies at address 0.
    /// That is measured, not theorised — M18 Task 6 caught it in a crash report
    /// (`.superpowers/sdd/2026-08-20-retrace-m18-workq/stage2-measurements.md` §3).
    ///
    /// This is the fourth instance of one recurring bug: the guest's fds were retrace's fds (M10),
    /// the guest's signal dispositions were retrace's (M11), the guest's pthread registration was
    /// retrace's (M18 Stage 1), and the guest's workqueue was retrace's (here).
    ///
    /// Returns 0. libdispatch reads a failure as "no workqueue at all", which would put it straight
    /// back on the path Stage 1 existed to clear.
    ///
    /// **There is deliberately no "open before kernreturn" assert.** The measured order is
    /// `kernreturn(0x400)` -> `open` -> `kernreturn(0x20)`: the first `workq_kernreturn` fires
    /// BEFORE `workq_open`. A plausible-looking ordering assert would fire on the real sequence.
    pub fn guest_workq_open(&mut self, _args: [u64; 8]) -> u64 {
        assert!(self.wq_thread_pc.is_some(),
            "M18 Stage 2a: workq_open before bsdthread_register — refusing to bring up a workqueue \
             with no registered wqthread entry point. Every dynamic guest registers one at startup \
             (measured, M14 Task 2), so this means the guest took a path no measurement covers.");
        0
    }

    /// `workq_kernreturn(op, arg2, arg3, arg4)` — the workqueue's whole control surface.
    ///
    /// **Emulated, never forwarded**, for the reason `guest_workq_open` documents. Dispatches on
    /// `args[0]`, the opcode, and takes the `guest_ulock_wake` fail-loud posture
    /// (`Box_::guest_ulock_wake`): every operation word this box has not measured is refused BY
    /// VALUE, so the panic names what to go measure.
    ///
    /// The first two opcodes below were measured M18 Task 6 (Stage 2a) as the only ones a guest had
    /// ever reached (`.superpowers/sdd/2026-08-20-retrace-m18-workq/stage2-measurements.md` §2),
    /// and this comment called that "a floor, not a ceiling: the park/return opcodes a RUNNING
    /// worker issues cannot be enumerated until Stage 2b makes a worker run". Stage 2b Task 1 read
    /// one of them straight off `__pthread_wqthread` before any worker ran, so **THREAD_RETURN is
    /// now measured too** (§3d of
    /// `docs/superpowers/specs/2026-08-23-retrace-m18-stage2b-wqthread-measurements.md`, which every
    /// §-citation below is to). The list is still a floor — `x5 == -1` at entry selects
    /// `__pthread_wqthread_exit`, a second teardown path this box has never seen — which is why
    /// every other value is still refused BY VALUE rather than guessed at.
    ///
    /// The XNU names in the constants are attributed from public libpthread sources and are NOT
    /// verified on this machine — `pthread/workqueue_private.h` ships in neither `/usr/include` nor
    /// the Xcode SDK. The raw values are the measurement; the names are a lead.
    pub fn guest_workq_kernreturn(&mut self, args: [u64; 8]) -> u64 {
        // libdispatch configuring the workqueue for dispatch. Carries a guest pointer in `args[1]`
        // (measured `0x27ff6a8`) that Stage 2b needs and Stage 2a only has to not forward.
        const WQOPS_SETUP_DISPATCH: u64 = 0x400;
        // libdispatch asking for worker threads. Stage 2b's entry point; Stage 2a's wall.
        const WQOPS_QUEUE_REQTHREADS: u64 = 0x20;
        // A worker parking once its dispatch callback has returned. MEASURED by Stage 2b Task 1
        // (§3d) two ways: `__pthread_wqthread` at `0x2e84 mov w0, #0x4` … `bl ___workq_kernreturn`,
        // and a live host breakpoint on the last call before exit showing `x0=4, x1=x2=x3=0`.
        const WQOPS_THREAD_RETURN: u64 = 0x4;

        match args[0] {
            WQOPS_SETUP_DISPATCH => 0,
            WQOPS_QUEUE_REQTHREADS => self.guest_workq_reqthreads(args),
            WQOPS_THREAD_RETURN => self.guest_workq_park(args),
            other => panic!(
                "M18 Stage 2b: unmeasured workq_kernreturn opcode {other:#x} — only \
                 SETUP_DISPATCH ({WQOPS_SETUP_DISPATCH:#x}), REQTHREADS \
                 ({WQOPS_QUEUE_REQTHREADS:#x}) and THREAD_RETURN ({WQOPS_THREAD_RETURN:#x}) have \
                 ever been observed (M18 Task 6; Stage 2b Task 1 §3d). Measure what \
                 issues this one before modelling it; a guessed opcode silently corrupts the \
                 guest's workqueue state. args={args:#x?}"),
        }
    }

    /// `workq_kernreturn(WQOPS_THREAD_RETURN, 0, 0, 0)` — a worker parking after its dispatch
    /// callback returned, asking the kernel for more work. **Emulated, never forwarded**, for the
    /// reason `guest_workq_open` documents.
    ///
    /// **This call must NOT return, and libpthread says so at the top of its voice.** §3d
    /// disassembled what follows the `bl`: `ldrsw x8, [x19, #0xac]`, then an `adrp`/`add` pair onto
    /// the literal `"BUG IN LIBPTHREAD: __workq_kernreturn returned"`, which stores the crash
    /// message and falls off the end into `_start_wqthread`'s `brk #0x1`. A park that returns kills
    /// the run right after the worker's useful work.
    ///
    /// Two things make that unreachable, and they are separate:
    ///
    /// 1. **The worker ends up non-runnable**, so `pick_next` hands the vCPU to main and the guest
    ///    can finish. `BlockReason::Parked` rather than `Exited`: a parked workqueue thread is
    ///    ALIVE — the real kernel can re-enter it with new work — and `Exited` carries a return
    ///    value it does not have. Stage 2b models no thread reuse, so nothing wakes it; that is
    ///    expected, not a leak. See `BlockReason::Parked`.
    /// 2. **The saved context is rewound onto the `svc` itself.** This is the sharp edge, and it is
    ///    not optional: both dispatch arms call `set_x0_err_and_return(rc, false)` *after* this
    ///    method returns, unconditionally, which sets `PC = ELR_EL1` — the instruction AFTER the
    ///    `svc`, i.e. exactly the return libpthread `brk`s on. `guest_bsdthread_terminate`'s arms
    ///    solve their version of this by omitting that call, but they can: they are their own
    ///    dispatch arms. This is one opcode inside an arm shared with SETUP_DISPATCH and REQTHREADS,
    ///    both of which do return. So instead of removing the write, this moves what it writes: back
    ///    up one A64 instruction, onto the `svc`. The parked context is then not resumable into a
    ///    return at all.
    ///
    /// What a resume WOULD do, stated honestly rather than left to be discovered: it re-executes the
    /// `svc` with `x0` clobbered to 0 by that same `set_x0_err_and_return`, so it re-enters this
    /// dispatch and hits the refuse-by-value panic above naming opcode `0x0`. Loud and named, which
    /// is the right answer for a thread nothing in Stage 2b intends to resume — and strictly better
    /// than the alternative it replaces, which is the guest `brk`ing inside libpthread. Re-entering
    /// at `_start_wqthread` instead (what the real kernel does for a REUSED worker) is deliberately
    /// NOT done here: that path needs a fresh register block with flags bit 17 set, which no
    /// measurement covers, and inventing one is the invention M14's rule forbids.
    ///
    /// **There is no "only a worker may park" assert.** This table does not distinguish workqueue
    /// threads from any other kind, so such a check would be inventing a distinction to enforce it;
    /// and the harmful case — the last runnable thread parking — is already loud, as
    /// `schedule_after_block`'s deadlock panic.
    ///
    /// Deterministic and below the trace: nothing here reads guest memory, both dispatch arms reach
    /// this through the same `guest_workq_kernreturn(args)` call, and the ELR rewind is arithmetic
    /// on state both sides already hold identically. Symmetry rule 1 holds by construction.
    fn guest_workq_park(&mut self, args: [u64; 8]) -> u64 {
        // The measured park carries `(0, 0, 0)` — §3d twice over: `__pthread_wqthread` sets
        // `x1/w2/w3` to zero with three unconditional `mov`s immediately before the `bl`, and the
        // live host breakpoint read them back as zero. This refuses anything else BY VALUE for a
        // concrete reason, not as hygiene: XNU's THREAD_RETURN also carries a kevent list
        // (`workq_kernreturn(THREAD_RETURN, eventlist, nevents, ...)`), and §5 item 8's "nothing
        // here requires a kevent list" holds only because those arguments are dead on the path this
        // guest takes. A kevent-carrying park means "hand these events back AND park" — parking
        // while dropping the events on the floor is exactly the silent corruption the refuse-by-
        // value posture exists to prevent.
        assert_eq!((args[1], args[2], args[3]), (0, 0, 0),
            "M18 Stage 2b: workq_kernreturn THREAD_RETURN with unmeasured arguments \
             {:#x?} — the only measured park is (0x4, 0, 0, 0) (Task 1 §3d). Non-zero args here \
             are XNU's kevent-carrying variant, which must return events rather than merely park; \
             measure it before modelling it. args={args:#x?}",
            (args[1], args[2], args[3]));
        self.threads.block(thread::BlockReason::Parked);
        // A64 instructions are 4 bytes, and `ELR_EL1` on an SVC trap holds the address of the one
        // AFTER the `svc` — so `- 4` is the `svc` itself. `wrapping_sub` because an ELR this low is
        // a broken box, not a case for this arm to police.
        let elr = self.vcpu.get_sys(sysreg::ELR_EL1).unwrap();
        self.vcpu.set_sys(sysreg::ELR_EL1, elr.wrapping_sub(4)).unwrap();
        0
    }

    /// `workq_kernreturn(WQOPS_QUEUE_REQTHREADS, 0, numthreads, priority)` — libdispatch asking the
    /// kernel for worker threads. **Emulated, never forwarded**, for the reason `guest_workq_open`
    /// documents.
    ///
    /// Stage 2a parked a deliberate panic here because the kernel allocates a workqueue thread's
    /// stack and pthread struct and enters `wqthread` with a register contract nobody had measured.
    /// M18 Stage 2b Task 1 measured it; every citation below is to
    /// `docs/superpowers/specs/2026-08-23-retrace-m18-stage2b-wqthread-measurements.md`.
    ///
    /// **This differs from `bsdthread_create` in who owns the memory.** There the guest had already
    /// mapped the stack and populated the struct, so M14 seeded registers and touched nothing else.
    /// Here the kernel owns both, so the box must place them — but it places an ADDRESS, not a
    /// LAYOUT: on the fresh path (flags bit 17 clear) `__pthread_wqthread_setup` writes the struct
    /// itself — 19 fields in its first 60 instructions, including its own `pacdb` signature and the
    /// TSD self-pointers, and reading only `+0x31` and `+0x4e` first, both read-modify-write on
    /// zeroed memory (§4, verified instruction by instruction). M14's no-invention rule survives
    /// without an exception rather than being carved into.
    ///
    /// `args[2]` is `numthreads` — MEASURED (§5 item 5) from libpthread's own caller,
    /// `__pthread_workqueue_addthreads`, whose `mov x2, x0` puts its `numthreads` argument there.
    /// Only `1` has ever been observed; the loop honours whatever the guest asks rather than
    /// building one worker and leaving libdispatch waiting forever for the rest, which is the
    /// failure a hardcoded `1` would produce silently.
    ///
    /// Deterministic with nothing recorded: `place_worker_stack` bumps the same `mmap_next` cursor
    /// every other emulated placement uses, so it is a pure function of the guest's syscall
    /// sequence, and both dispatch arms reach this through the same `guest_workq_kernreturn(args)`
    /// call — symmetry rule 1 holding by construction, exactly as it does for `bsdthread_create`.
    fn guest_workq_reqthreads(&mut self, args: [u64; 8]) -> u64 {
        let entry = self.wq_thread_pc.expect(
            "M18 Stage 2b: REQTHREADS with no registered wqthread entry point — refusing to build \
             a worker that would enter an invented address. Every dynamic guest registers one at \
             startup (measured, M14 Task 2), so this means the guest took a path no measurement \
             covers.");
        let pthsize = self.pthread_size.expect(
            "M18 Stage 2b: REQTHREADS with no registered pthread size — captured by the same \
             bsdthread_register that captures the entry point, so this cannot happen without the \
             assert above firing first.") as u64;
        let count = args[2];
        assert!((1..=WQ_MAX_WORKERS_PER_REQUEST).contains(&count),
            "M18 Stage 2b: REQTHREADS for {count} threads, outside 1..={WQ_MAX_WORKERS_PER_REQUEST}. \
             `args[2]` is numthreads (§5 item 5). Zero is meaningless — libdispatch would wait \
             forever on work no worker was built to run — and anything near the ceiling means this \
             box is reading the wrong argument, not that the guest wants that many workers; each \
             one costs a stack reservation. Every observed request asks for 1. args={args:#x?}");
        // The flags word is the same for every worker in one request: it encodes the request's own
        // QoS, and `args[3]` is a property of the request, not of the individual thread.
        let flags = WQ_ENTRY_FLAGS_FRESH | Self::wq_qos_index(args[3]);

        for _ in 0..count {
            // Stack and struct, in ONE reservation, so their relative placement is what §2c
            // measured rather than two independent bumps that could drift apart.
            let (stack_base, stack_top, pthread) = self.place_worker_stack(pthsize);

            // `spawn` appends, so the index it will return is the table's length right now. Asserted
            // below rather than assumed, because the kport MUST be part of the register block: it is
            // derived from the index, and splitting it out to a `ctx_mut` write after the spawn
            // would scatter one measured contract across two places.
            let tid = self.threads.len();

            let mut ctx = thread::ThreadCtx::zeroed();
            ctx.elr = entry;
            ctx.regs.pc = entry;
            // The creating thread's EL0 PSTATE, NOT zeroed()'s 0 — M14's lesson, restated at
            // `guest_bsdthread_create` in full: 0 is EL0t with DAIF clear, which looks like it
            // works right up until the mask bits matter. `regs.cpsr` is the one HVF resumes from.
            ctx.spsr = self.spsr();
            ctx.regs.cpsr = ctx.spsr;

            // ---- THE WQTHREAD ENTRY CONTRACT, MEASURED (§1c's table) ------------------------
            // `_start_wqthread` is three instructions and moves NO register (§1a), so the kernel's
            // entry contract is exactly `__pthread_wqthread`'s argument registers. Each line below
            // names the instruction that READS it. Registers §1 does not show being read are left
            // at `zeroed()`'s 0 — seeding one the code never reads is invention that happens to be
            // harmless today.
            //
            // x0 — the pthread struct pointer. `0x2dbc mov x19, x0`; everything downstream writes
            // through x19 (`str x9,[x19,#0x100]`, `stp x8,x0,[x19,#0x90]`, …).
            ctx.regs.x[0] = pthread;
            // x1 — the thread's mach port name. `0x302c str w1, [x0, #0xf8]` inside
            // `__pthread_wqthread_setup` — the SAME `pthread + 0xf8` M14 measured for a
            // `bsdthread_create` child, which is why this reuses `GUEST_THREAD_PORT_BASE | tid`
            // rather than minting a new scheme. Uniqueness is load-bearing beyond the `brk` below:
            // `Box_::thread_of_port` resolves a port back to a thread by reading that field out of
            // guest memory, and M16's signal delivery routes on the answer, so a worker sharing a
            // kport with another thread would misroute signals.
            //
            // Unlike M14's silent `pthread_join` failure, a bad value here is LOUD: `0x30c8 ldr
            // w8,[x19,#0xf8]; add w9,w8,#1; cmp w9,#1; b.ls` fires `brk #0xb001` with "BUG IN
            // CLIENT OF LIBPTHREAD: Unable to allocate thread port" for BOTH `0` and `-1` (§4).
            ctx.regs.x[1] = (GUEST_THREAD_PORT_BASE | tid as u32) as u64;
            // x2 — the LOW end of the stack region. `0x2fe0 sub x10, x2, x10` and
            // `0x2ff0 stp x0, x2, [x0, #0xb0]` inside setup, which derive every stack field of the
            // struct from x0 and x2 alone. §2c: "Neither is derived from the other, and neither is
            // derived from SP — libpthread never inspects SP. Retrace must supply both."
            ctx.regs.x[2] = stack_base;
            // x4 — the flags word. `0x2dc0 tbnz w4, #0x11` is the first instruction to read it, and
            // it selects fresh-vs-reused; the rest of the word asserts the TSD base and encodes the
            // QoS. See `WQ_ENTRY_FLAGS_FRESH` for the bit-by-bit decode, including the two bits that
            // are deliberately CLEAR, and `wq_qos_index` for the low byte.
            ctx.regs.x[4] = flags;
            // x3, x5, x6, x7 and LR are LEFT AT ZERO, which is what the kernel itself supplies —
            // not an omission. x3 (the kevent list) is read only on the bit-22/bit-19 branches the
            // flags above do not take, so it is dead here (§1c, 0 in 7/7). x5 (the event count) is
            // the one whose zero is LOAD-BEARING rather than merely faithful: `0x2e44 cmn w20, #0x1`
            // treats `-1` as a kill sentinel and branches straight to `__pthread_wqthread_exit`.
            // x6/x7 are read by nothing (6/6 zero), and LR is 0 (7/7) because `_start_wqthread`'s
            // `brk #0x1` is its only return target and is unreachable.
            //
            // SP — §2c: "SP == x0, 7/7 entries that read it. The stack top IS the struct base."
            // `_start_wqthread`'s first instruction is `stp xzr, xzr, [sp, #-0x10]!`, so
            // `[x0-0x10, x0)` must be writable stack, which is why `place_worker_stack` puts the
            // struct at the TOP of the region with the stack growing down into it.
            ctx.regs.sp_el0 = stack_top;
            // TPIDRRO_EL0 — EL0 cannot write it, so the KERNEL must, and flags bit 21 above is
            // retrace asserting that it did. M14's `pthread + 0xe0`, reconfirmed for a workqueue
            // thread two independent ways (host probe 3/3 across thread kinds; `_pthread_exit`'s
            // `mrs x8, TPIDRRO_EL0; sub x0, x8, #0xe0`). §2c is emphatic that getting this wrong is
            // NOT caught by the bit-21 `brk` — libpthread tests the flag, never the register — so a
            // mismatch here is silent corruption with nothing to hunt for.
            ctx.tpidrro_el0 = pthread + PTHREAD_TSD_OFF;

            // `(stack_base, stack_top - stack_base)` — a REAL length, unlike `bsdthread_create`'s
            // `(stack, 0)` placeholder. That syscall carries no size; this one is placed by the box,
            // so the box knows it. `Thread::stack` is for teardown and diagnostics only.
            let spawned = self.threads.spawn(ctx, (stack_base, stack_top - stack_base));
            assert_eq!(spawned, tid, "ThreadTable::spawn appends; the kport above assumed it");
        }
        0
    }

    /// Reserve the main thread's believed-but-unbacked stack (M8 spec risk R3).
    ///
    /// **A reservation, not an `mmap`** — the same choice, for the same measured reason, as
    /// `place_worker_stack`: `guest_vm_reserve` is bookkeeping only, and each page is demand-committed
    /// with a fresh zeroed anon page on first touch by `commit_reserved_page`. Eagerly backing the full
    /// 8 MiB was measured at ~1.7x on `hello_rust` and worse across the dyld suite, because the
    /// per-syscall diff scales with total MAPPED memory — and a reservation maps nothing. A guest that
    /// never recurses deeply commits zero pages and pays zero.
    ///
    /// **The window deliberately stops one granule ABOVE the guard page.** libstd mmaps its guard
    /// `MAP_FIXED PROT_NONE` at `GUARD_PAGE_IPA`; a backed PROT_NONE page faults at STAGE 1 (permission,
    /// via the EL1 trampoline) and arrives as `Stop::Fault` for M12's disposition check, which is what
    /// delivers SIGSEGV to libstd's handler. If the guard were inside the reservation it would instead
    /// be unbacked, fault at STAGE 2, and be silently committed here — turning a stack overflow into a
    /// corrupted guest that keeps running. That is M13's invariant (see `protect_none`), and this is the
    /// one place M21 could have broken it.
    ///
    /// Deterministic and trace-free: `load_dynamic` runs identically on record and replay, so the same
    /// reservation exists on both sides and nothing about it enters the trace (symmetry rule 2).
    fn reserve_believed_stack(&mut self) {
        // These three are pure functions of compile-time constants (GUARD_TOP, GUARD_PAGE_IPA,
        // DYN_STACK_BOTTOM, PT_L3_CEIL never vary at runtime), so clippy's assertions_on_constants
        // rightly refuses a plain assert! here — const { assert!(..) } is the existing convention
        // this file already uses for the same situation (see stack_geometry_tests's DYN_STACK_TOP
        // <= 1 << 36 check), and it is strictly stronger: a layout mistake fails the BUILD, not a
        // test run or a live load.
        const { assert!(GUARD_TOP > GUARD_PAGE_IPA,
            "the window must start ABOVE the guard page, or an overflow is silently committed \
             instead of faulting") };
        const { assert!(GUARD_TOP < DYN_STACK_BOTTOM,
            "believed-stack window inverted: DYN_STACK_TOP or DYN_STACK_SIZE moved without moving \
             this") };
        const { assert!(GUARD_TOP >= PT_L3_CEIL,
            "the window would overlap the L3 translation tables") };
        self.guest_vm_reserve(GUARD_TOP, DYN_STACK_BOTTOM - GUARD_TOP, false);
    }

    /// Place ONE workqueue worker's stack and pthread struct. Returns `(stack_base, stack_top,
    /// pthread)`, where `stack_top == pthread` — that equality is §2c's measurement (`SP == x0`,
    /// 7/7), not an implementation convenience, and both are named so the call site reads as the
    /// contract rather than as arithmetic.
    ///
    /// The layout is §2c's, and each part of it is required by something:
    /// - the struct sits at the **top**, the stack below it: `x0 > x2` is what makes
    ///   `[pthread+0xc8]`, the size libpthread computes as `round_up(x0,P) - (x2-P) + round_up(
    ///   0x18e0,P)`, come out sane;
    /// - the struct memory must read as **zero** — `__pthread_wqthread_setup` does read-modify-write
    ///   on `ldrb w11,[x0,#0x31]` and `ldrh w9,[x0,#0x4e]` before storing them back;
    /// - the guard page libpthread records at `pthread+0xc0` is `[x2 - vm_page_size, x2)`. It only
    ///   *records* that address — it neither maps nor checks it — so if this page is to guard
    ///   anything, **the guard is retrace's, not libpthread's.**
    ///
    /// **The one-granule bump is what makes the guard page real** (fix round 1, Important 1).
    /// `guest_vm_reserve` hands out `mmap_next` and advances it CONTIGUOUSLY, so without the bump
    /// `[x2 - GRANULE, x2)` is the last page of whatever reservation precedes it — for the second
    /// worker of a `numthreads=2` request, that is the FIRST worker's pthread struct page, and
    /// `commit_reserved_page` would back it on first touch because it lies inside a tracked
    /// reservation. A worker-2 stack overrun would then corrupt worker 1's struct in silence. With
    /// the bump the page falls in a hole no reservation covers, so `commit_reserved_page` returns
    /// `false` for it and the access stays a fatal stage-2 fault — which is the whole point: an
    /// overrun that faults is a bug you can find.
    ///
    /// That hole is a gap, not a claim on the address space: nothing marks it forbidden, so a later
    /// ANYWHERE-with-hint placement (`first_fit`) could still fit a small mapping into it, after
    /// which an overrun would land in that mapping instead. What the bump guarantees is the case
    /// that was actually silent — demand-commit — and `retrace-box/tests/threads.rs`'s
    /// `workq_reqthreads_leaves_an_uncommittable_guard_page_below_each_worker_stack` pins exactly
    /// that, refusal and non-vacuous positive control both.
    ///
    /// **A reservation, not an `mmap`.** `guest_vm_reserve` is bookkeeping only, and each page is
    /// demand-committed with a fresh zeroed anon page on first touch by `commit_reserved_page` —
    /// the path both dispatch loops already run for the guest's own reservations. That satisfies
    /// the zero requirement by construction, and it keeps the ~0.5 MiB a worker is *given* from
    /// being ~0.5 MiB every subsequent syscall has to diff (M8-stack's measured lesson: growing a
    /// guest region costs per-syscall diff time). A worker touches a handful of those pages.
    ///
    /// Deterministic: `guest_vm_reserve(.., anywhere)` hands out `mmap_next` and advances it, so
    /// an identical call sequence places an identical worker on record and on replay.
    fn place_worker_stack(&mut self, pthsize: u64) -> (u64, u64, u64) {
        // Skip one granule so the guard page below `x2` lands in a hole no reservation covers.
        // Deterministic like everything else here: a fixed bump of the same cursor.
        self.mmap_next += GRANULE as u64;
        let base = self.guest_vm_reserve(0, WQ_WORKER_STACK_SIZE + pthsize, true);
        let pthread = base + WQ_WORKER_STACK_SIZE;
        (base, pthread, pthread)
    }

    /// Recover the QoS index that belongs in the low byte of the `wqthread` entry flags from the
    /// priority word a `REQTHREADS` request carries in `args[3]`.
    ///
    /// This inverts arithmetic `__pthread_wqthread` performs in the other direction (§1d, verbatim
    /// from the disassembly): `priority = (1 << ((flags + 7) & 31)) | 0xff`, applied only when the
    /// index is in `1..=6` (`cmp w9,#5; csel ... hi` discards it otherwise, leaving priority 0). So
    /// the request's QoS bit is `1 << (qos_index + 7)`, which lives in bits `[15:8]` — the byte this
    /// reads — and `qos_index = trailing_zeros(byte) + 1`.
    ///
    /// Checked against all three measured `(request → fresh entry flags)` pairs (§1e): `0x10ff` →
    /// `0x244005`, `0x04ff` → `0x244003`, and the reused-path `0x02ff` → `0x064002`. The guest's own
    /// request is `0x040008ff` → byte `0x08` → index **4**, i.e. flags `0x244004`.
    ///
    /// **`0x244004` is an EXTRAPOLATION, not an observation, and it is the single load-bearing
    /// unverified claim in Task 1's document (§1e, §5 item 4).** No live `(request → flags)` pair
    /// was captured for `0x040008ff`; six queue configurations were tried and none reproduced that
    /// request, because a host process's main thread carries a real QoS. If a worker ever misbehaves
    /// in a way that smells like a QoS bucket, suspect this first.
    ///
    /// Every OTHER bit of the request is dropped, which is fidelity rather than laziness: §1e
    /// measured that no flags bit maps to priority bit 26, so a real kernel delivering `0x244004`
    /// hands the dispatch callback `0x8ff` and not the `0x040008ff` it was asked for. The round trip
    /// is lossy by design. (The one request bit that IS known to survive — bit 31, overcommit,
    /// which becomes flags bit 16 — was observed only on the reused path and is deliberately not
    /// modelled here; the guest's request has it clear.)
    fn wq_qos_index(priority: u64) -> u64 {
        let byte = ((priority >> 8) & 0xff) as u32;
        // `is_power_of_two()` is false for 0, so this rejects "no QoS bit" as well as "several".
        assert!(byte.is_power_of_two(),
            "M18 Stage 2b: REQTHREADS priority {priority:#x} has QoS byte {byte:#x}, which is not a \
             single set bit. Every measured request carries exactly one (§1e), and the flags word \
             libpthread demands encodes exactly one index — measure what issues this before \
             modelling it rather than picking a bit.");
        let idx = byte.trailing_zeros() as u64 + 1;
        assert!((1..=6).contains(&idx),
            "M18 Stage 2b: REQTHREADS priority {priority:#x} decodes to QoS index {idx}, outside \
             the 1..=6 libpthread accepts (`0x2e34 cmp w9, #0x5` after a `sub w9,w9,#1`). Outside \
             that range libpthread silently leaves the worker's priority 0 rather than failing, so \
             this refuses by value instead of shipping a worker at a priority nobody asked for.");
        idx
    }

    /// `bsdthread_create(func, arg, stack, pthread, flags)`, emulated.
    ///
    /// **Never forwarded.** The host would create a real thread inside retrace's own process
    /// starting at a guest address — the same class of hazard as M13's `mach_vm_protect`, but worse,
    /// because it would execute.
    ///
    /// The stack is the guest's own: libpthread `mach_vm_map`s it and `mach_vm_protect`s its guard
    /// page PROT_NONE (M13's first real caller) two traps before this one. M14 places no IPAs.
    pub fn guest_bsdthread_create(&mut self, args: [u64; 8]) -> u64 {
        let (func, arg, stack, pthread, flags) = (args[0], args[1], args[2], args[3], args[4]);
        let start = self.thread_start_pc.expect(
            "M14: bsdthread_create before bsdthread_register — refusing to invent a thread entry \
             point. Every dynamic guest registers one at startup (measured, M14 Task 2), so this \
             means the guest took a path no measurement covers.",
        );

        let mut ctx = thread::ThreadCtx::zeroed();
        // The entry point is written to BOTH `elr` and `regs.pc`, not just one. `switch_to_thread`'s
        // `load_ctx` -> `write_regs` sets `reg::PC` from `regs.pc` for a live vCPU resume, while the
        // syscall-return path (`set_x0_err_and_return`) resumes off ELR_EL1 and would clobber x0 —
        // the one register this child must receive intact. WHICH convention actually drives a
        // scheduled thread's first resume is Task 9's to settle, not this task's; writing both here
        // makes the context resumable either way rather than leaving that choice to be rediscovered
        // as a bug when Task 9 wires the scheduler in.
        ctx.elr = start;
        ctx.regs.pc = start;
        ctx.spsr = self.spsr();
        // M14 Task 9 SETTLES the convention Task 7 deferred by name ("WHICH convention actually
        // drives a scheduled thread's first resume is Task 9's to settle"). It is `regs.pc` /
        // `regs.cpsr`: `switch_to_thread` -> `load_ctx` -> `write_regs` writes `reg::PC` and
        // `reg::CPSR`, and those are what HVF resumes the vCPU from — the same two registers
        // `set_x0_err_and_return` writes to send a thread back to EL0 after a syscall. ELR_EL1 /
        // SPSR_EL1 are carried too, but only matter to a later ERET-shaped path.
        //
        // So `regs.cpsr` must be the creating thread's EL0 PSTATE, NOT `ThreadCtx::zeroed()`'s 0.
        // SPSR_EL1 at this trap IS that PSTATE (the guest trapped from EL0 through the trampoline).
        // Leaving it 0 is quietly wrong rather than loudly broken: 0 is EL0t with DAIF clear, which
        // looks like it works right up until the mask bits matter.
        ctx.regs.cpsr = ctx.spsr;
        ctx.regs.sp_el0 = stack;

        // THE THREAD-START CONTRACT, MEASURED (Task 2) AND INDEPENDENTLY RE-DISASSEMBLED IN REVIEW.
        // NOT the classic _pthread_start(self, kport, fun, funarg, stacksize, pflags) shape — this
        // plan originally guessed that and was WRONG. `__pthread_start` reads only:
        //   x0 — the pthread-struct pointer
        //   w5 — flags (tested via tbnz/tbz at its entry, 0x6be0/0x6be4)
        // and it loads func/arg FROM THE STRUCT, not from registers:
        //   `ldp x8, x0, [x19, #0x90]` at 0x6c50 → x8 = [pthread+0x90] (func),
        //                                          x0 = [pthread+0x98] (arg), then `blraaz x8`.
        // x1-x4 are never read on the entry-to-dispatch path. (One callee,
        // `__pthread_markcancel_if_canceled`, does `mov x0, x1` at 0x3788, but only on an
        // already-canceled branch this path does not take — internal bookkeeping, not dispatch.)
        //
        // So the box seeds x0 and w5 and nothing else. It does NOT populate the struct: the guest's
        // own `_pthread_create` stored func/arg at +0x90/+0x98 BEFORE issuing this trap, which is
        // why the fields are there to be read. Writing them here would be retrace inventing guest
        // state it does not own.
        ctx.regs.x[0] = pthread;
        // The measured contract reads `w5` (32-bit), not `x5` — model the zero-extension a real
        // 32-bit register read performs rather than carrying the full 64-bit syscall argument.
        //
        // M14 t11: OR in the kernel's own `PTHREAD_START_TSD_BASE_SET`. `__pthread_start`'s SECOND
        // instruction is `tbz w5, #0x1c, 0x6c8c`, and 0x6c8c loads "BUG IN LIBPTHREAD:
        // thread_set_tsd_base() wasn't called by the kernel" and falls into `brk #0xb001`. Passing
        // the guest's flags through verbatim therefore kills the child on its second instruction —
        // measured, not theorised: that brk is exactly where M14's headline guest died once the
        // kport write above let a child run at all. ORed, not substituted: bits 0-27 are the
        // guest's own (QOS class, CUSTOM, …) and `__pthread_start` reads several of them.
        ctx.regs.x[5] = (flags as u32 as u64) | PTHREAD_START_TSD_BASE_SET;
        // Each thread gets its own thread pointer. TPIDR_EL0 stays 0 for ALL threads (M2-cpuid).
        //
        // M14 t11: the TSD is at `pthread + 0xe0`, NOT the struct pointer — this used to seed the
        // struct pointer itself. libpthread reads the register back the other way
        // (`__pthread_join`: `mrs x23, TPIDRRO_EL0` / `sub x21, x23, #0xe0`), so the old value made
        // every thread compute a pthread struct 0xe0 bytes below its real one. A host probe pinned
        // the convention directly, 4/4 for main and child: `TPIDRRO_EL0 - pthread_self() == 0xe0`.
        // This is the substance behind the flag bit above — setting that bit while leaving the base
        // wrong would be the box asserting something it had not done.
        ctx.tpidrro_el0 = pthread + PTHREAD_TSD_OFF;

        // `func` and `arg` are deliberately unused: they reach the child through the struct, not
        // through us. Named in the destructuring above so the ABI stays documented at the call site.
        let _ = (func, arg);

        // `(stack, 0)`: the SECOND element is a placeholder length, not a measured size — the
        // syscall carries no stack-size argument, and `stack` (x2) is the guest-computed stack TOP
        // libpthread already mapped, not a base. `Thread::stack` is documented as `(base, len)` for
        // TEARDOWN (Task 8's named consumer); nothing reads it before then.
        let tid = self.threads.spawn(ctx, (stack, 0));

        // THE KERNEL'S OTHER HALF OF bsdthread_create: the child's mach port, written into the
        // guest's own pthread struct. Without it `pthread_join` does not merely mis-wait — it does
        // not wait AT ALL, and reports success. From libsystem_pthread on this host:
        //
        //   __pthread_join:
        //     0x90b8  ldr  w9, [x19, #0x34]   ; the join futex word
        //     0x90bc  cmn  w9, #0x1           ; already -1 => the thread exited, take that path
        //     0x90c0  b.eq 0x9208
        //     0x90c4  ldr  w9, [x19, #0xf8]   ; <-- otherwise the KPORT becomes the wait value
        //     0x90c8  str  w9, [sp, #0x10]
        //     0x90cc  str  w9, [x19, #0x34]
        //     ...
        //     0x9024  cbz  w8, 0x9198         ; kport == 0  =>  skip __ulock_wait entirely
        //     0x9198  bl   __pthread_deallocate
        //     0x91a4  mov  w21, #0x0          ;              =>  and return SUCCESS
        //
        // So a zero here makes the guest free a thread that never ran and call it a successful
        // join. That is not a hypothetical: it is exactly how M14's headline guest failed before
        // this write existed — libstd's `Arc::get_mut(..).expect("threads should not terminate
        // unexpectedly")` panicked because the child never dropped its packet clone.
        //
        // THE KERNEL IS THE WRITER — measured twice, not inferred. (1) The only two userspace
        // writers of `+0xf8` in libsystem_pthread are `__pthread_main_thread_init` and
        // `__pthread_wqthread_setup`; neither is on the `pthread_create` path, and the parent's
        // call site at 0x83a0 only tests the return for -1 (`cmn x0, #0x1`) and stores nothing.
        // (2) A host probe read the field with the child provably not yet run, 5/5 runs: `[+0xf8]`
        // already equalled `pthread_mach_thread_np(t)` while `[+0x34]` was still 0.
        //
        // Fail loud on an untranslatable struct: the guest just wrote func/arg into this same page
        // (`+0x90`/`+0x98`, the contract above), so an unmapped pthread here means the box's view of
        // guest memory disagrees with the guest's — worth a panic, not a silent skip that would
        // resurface as the very "join returns without waiting" bug this write exists to prevent.
        let port = GUEST_THREAD_PORT_BASE | tid as u32;
        let ipa = self.va_to_ipa(pthread + PTHREAD_KPORT_OFF).unwrap_or_else(|| {
            panic!("M14: bsdthread_create pthread struct {pthread:#x} has no mapping for +0xf8")
        });
        self.write_guest(ipa, &port.to_le_bytes());
        0
    }

    /// `bsdthread_terminate(stackaddr, freesize, port, sem)` — a guest thread's exit.
    ///
    /// **Does not return.** The calling thread is gone; the trap loop must switch rather than
    /// resume it — M14 Task 9's job (wiring the scheduler into `Box_::run()`), not this one's.
    /// (Fix round 1, M-4: retrace-core's two arms now panic instead of silently resuming this
    /// thread's vCPU state, which is the enforcement half of that "does not return" — this method
    /// only does the bookkeeping half.) `_args` is unused here for the same reason M14 Task 7's
    /// `bsdthread_create` leaves `func`/`arg` untouched: `stackaddr`/`freesize` name the guest's
    /// own stack for the KERNEL to reclaim, which this box does not yet do (no test in this task
    /// depends on the guest's stack backing actually being freed) — teardown of the memory itself
    /// is left for whichever task needs it, rather than guessed at here.
    pub fn guest_bsdthread_terminate(&mut self, _args: [u64; 8]) -> u64 {
        let me = self.threads.current();
        // Open question 1 in the spec: whether the return value rides here or in the pthread
        // struct. Task 1/2 measured it; if it is in the struct, libpthread reads it back itself
        // and the box records 0 here rather than pretending to know.
        self.threads.exit_current(0);
        // M17 tripwire. M17 materialises pended signals at exactly ONE site (`guest_ulock_wake`),
        // which is sound only because no production path constructs `BlockReason::Join` today
        // (measured: every construction site is in `retrace-box/tests/threads.rs`; the guest's own
        // `sigblocked.rs` records the same fact). But `unblock_joiners_of` is still a live wake
        // path, so if a producer ever appears it becomes a SECOND place a thread goes from Blocked
        // to Runnable — and a signal pended on such a thread would be swallowed in silence.
        //
        // The assert fires on the HAZARD, not on a proxy for it: a woken joiner that is carrying a
        // pending signal. Asserting `joiners.is_empty()` instead would be both too strong and too
        // brittle — `a_terminating_thread_exits_and_wakes_whoever_joined_it` drives a Join state
        // through this very path on purpose, to prove the exit/wake wiring, and it strands nothing.
        // `pending_of` rather than the mask-filtered set is deliberate: a masked pending signal is
        // not stranded (its unmask landmark still materialises it), but a tripwire should err
        // toward firing early.
        let joiners = self.threads.unblock_joiners_of(me);
        let carrying: Vec<usize> = joiners.iter().copied()
            .filter(|&t| self.threads.pending_of(t) != 0)
            .collect();
        assert!(carrying.is_empty(),
            "unblock_joiners_of woke {carrying:?} while they were carrying pending signals. \
             `BlockReason::Join` has gained a producer, so joiner-wake is now a second path from \
             Blocked to Runnable — and M17 materialises pended signals only at __ulock_wake. Teach \
             this site to materialise too, or the signals pended on those threads are lost \
             silently.");
        0
    }

    /// `__ulock_wait(operation, addr, value, timeout_us)` — the join primitive M14 Task 1
    /// measured (syscall 515, `SYS_ULOCK_WAIT`). **Never forwarded**: the host has no guest
    /// thread to actually block on this address, and forwarding would block retrace's OWN
    /// process on a guest address — the same class of hazard Task 1 measured as
    /// whole-process-fatal for `bsdthread_create`.
    ///
    /// Blocks the CURRENT thread on `addr` only if the guest's own wait condition still holds:
    /// real `__ulock_wait` blocks iff the live memory at `addr` still equals the `value` the
    /// caller compared it against (`args[2]`) — if some other thread already changed it, the
    /// wait is ALREADY SATISFIED and this thread must stay Runnable. Blocking unconditionally
    /// would deadlock a race the guest already won (this is Step 4's guard, written explicitly
    /// rather than assumed).
    ///
    /// x0: real `__ulock_wait` returns 0 on a normal wake, some non-zero value when the wait was
    /// skipped because already-satisfied (the exact value is unmeasured), and a negative errno on
    /// timeout/failure. This box always returns 0 on the non-EFAULT paths — including the
    /// already-satisfied one — which is a SIMPLIFICATION, not a measured fact (fix round 1, M-5
    /// corrects the earlier doc's overstated claim). **The block/no-block decision made here is
    /// still not independently visible in `rc`/`err` or in any guest memory write**: both
    /// branches return the same `rc`/`err` (only the EFAULT path differs), and neither writes
    /// guest memory, so nothing in those fields of the recorded `Event::Syscall` distinguishes
    /// "blocked" from "stayed runnable" (fix round 1, I-3 corrects the earlier doc's false claim
    /// that the mirror already checks this).
    ///
    /// That gap is CLOSED now, though — directly, not indirectly. M15 Task 3 added a `thread: u32`
    /// field to `Event::Syscall` recording which thread issued each syscall (`TRACE_MAGIC` bumped
    /// to `RT\x00\x07`), and M15 Task 4 made `ReplaySession::advance` compare it against
    /// `self.current_thread()` on every syscall, right after the `(num, args)` check. If replay's
    /// memory at `addr` ever differed from record's — an upstream bug, not a normal-operation risk
    /// — and this call left a different thread `Runnable` here than record did, that wrongly
    /// scheduled thread is the one that issues the NEXT recorded syscall event, and the thread
    /// check catches the mismatch there directly. An earlier version of this doc argued the
    /// existing `(num, args)` check would catch that "automatically" once a scheduler existed —
    /// that claim was already known false during M14's own review, before this doc was written:
    /// two threads running the SAME code issue byte-identical `(num, args)`, so a wrong-thread
    /// replay of identical code passed that check in silence. The `thread` field and the M15
    /// Task 4 comparison are what actually closes the gap; nothing here catches it "automatically".
    ///
    /// **Returns the raw `x0` word, not a `Result`, because under `ULF_NO_ERRNO` this syscall has
    /// no error flag to return** (fix round 1, I-2). Both whitelisted operation words set bit 24 =
    /// `ULF_NO_ERRNO` (`0x0100_0002`, `0x0102_0002`), under which the kernel reports failure as a
    /// *negative errno in x0 with `PSTATE.C` CLEAR* rather than `+errno` with carry set. Measured
    /// on this host with a raw `svc #0x80` — libsyscall's stub branches to `cerror` on carry and
    /// would have destroyed both facts — passing an unmapped address to syscall 515. Probe source,
    /// output and build line in
    /// `.superpowers/sdd/2026-08-12-retrace-m14-threads/task-9-measurements.md` §6:
    ///
    /// ```text
    /// op=0x01000002  x0=0xfffffffffffffff2  w0=-14  C=0     <- ULF_NO_ERRNO
    /// op=0x01020002  x0=0xfffffffffffffff2  w0=-14  C=0     <- the second whitelisted word
    /// op=0x00000002  x0=0x000000000000000e  w0=+14  C=1     <- the control, without the flag
    /// ```
    ///
    /// The guest reads this back exactly that way: `__pthread_join` @ `0x911c` tests its result
    /// with `cmn w0, #0x4` (i.e. `w0 == -4`) and `__pthread_mutex_ulock_unlock_slow` @ `0xa154`
    /// with a bare sign-bit test `tbz w0, #0x1f`. Returning `+14` with carry set — which the
    /// earlier `Result<u64, u64>` shape made the dispatch arms do — sent the guest into `cerror`,
    /// missing every one of those tests: `__pthread_join` would then re-read the word and re-wait,
    /// livelocking on recorded 515 events. Collapsing the `Result` is what makes that
    /// unrepresentable rather than merely fixed: with no `Err` variant there is no `err: true` for
    /// a dispatch arm to record.
    ///
    /// Note the full 64-bit sign extension above (`0xffff_ffff_ffff_fff2`, not `0xffff_fff2`) —
    /// that is what the measurement shows, so that is what this returns, even though every
    /// measured consumer only ever looks at `w0`.
    pub fn guest_ulock_wait(&mut self, args: [u64; 8]) -> u64 {
        // Fix round 1, M-2: freshly re-disassembled `__pthread_join`'s retry loop (see the report)
        // — the ONLY operation words it is measured to pass are these two 32-bit
        // compare-and-wait shapes (differing only in a flag bit gated on a register this box
        // doesn't otherwise need). Anything else — in particular a 64-bit compare-and-wait op —
        // would silently degrade to this arm's 32-bit read below, making equality MORE likely and
        // therefore blocking where the real kernel would not: a manufactured deadlock. Fail loud
        // on an unmeasured operation instead of guessing its width.
        assert!(matches!(args[0] as u32, 0x1000002 | 0x1020002),
            "guest_ulock_wait: unmeasured operation word {:#x} — only __pthread_join's plain \
             32-bit compare-and-wait is modelled; a 64-bit compare would silently degrade to a \
             32-bit read here and could deadlock", args[0]);
        let addr = args[1];
        let expect = args[2] as u32;
        // Fix round 1, M-3: `addr` is guest-controlled and a bad pointer is LEGAL guest behaviour
        // (real `__ulock_wait` answers EFAULT), not a retrace invariant violation — use the
        // checked read rather than `read_guest`, which would panic the box on it.
        //
        // Fix round 1, I-2: NEGATED here, in the one place that reads the operation word. The
        // `-errno`-with-carry-clear convention is a property of `ULF_NO_ERRNO` in `args[0]`, and
        // the retrace-core dispatch arms never decode `args[0]` — putting the negation there would
        // put operation-word semantics in a layer that cannot see the operation word, and would
        // silently answer wrongly on the day a non-`ULF_NO_ERRNO` word is measured and whitelisted.
        // The assert above is what makes this sound: it admits only the two words that carry the
        // flag, so every path out of this function is a `ULF_NO_ERRNO` path.
        const EFAULT: i64 = 14;
        let Some(bytes) = self.read_guest_checked(addr, 4) else { return (-EFAULT) as u64; };
        let current = u32::from_le_bytes(bytes.try_into().unwrap());
        if current == expect {
            self.threads.block(thread::BlockReason::Wait { addr });
        }
        0
    }

    /// `__ulock_wake(operation, addr, wake_value)` — the wake half of the pair `guest_ulock_wait`
    /// blocks on (syscall 516, `SYS_ULOCK_WAKE`). **Never forwarded**: forwarding would issue a real
    /// `__ulock_wake` from retrace's OWN process against a guest address, the identical hazard class
    /// `guest_ulock_wait`'s doc cites for 515. Task 8 fail-loud-blocked it; this is the real handler.
    ///
    /// **Wakes by address equality, which is measured rather than fabricated.** See
    /// `ThreadTable::unblock_waiters_on` for the disassembly: `__pthread_join` waits on
    /// `pthread + 0x34` and `__pthread_joiner_wake` wakes on `pthread + 0x34`, so the address the
    /// guest hands both calls IS the correlation. Full walk in
    /// `.superpowers/sdd/2026-08-12-retrace-m14-threads/task-9-measurements.md`.
    ///
    /// Returns 0 unconditionally. Read off `__pthread_joiner_wake`'s own branches rather than
    /// assumed: it accepts 0 and -2 (`ENOENT`, nobody was waiting) as success, retries on -4
    /// (`EINTR`), and aborts the guest ("BUG IN LIBPTHREAD") on anything else. 0 is therefore
    /// always accepted, whether or not a waiter was present — so a wake nobody is waiting on is a
    /// legal no-op here, not a fault.
    ///
    /// The sole measured wake word `0x0100_0002` also carries bit 24 = `ULF_NO_ERRNO`, so if this
    /// ever grows a failure path it must answer `-errno` with `PSTATE.C` CLEAR, exactly like
    /// `guest_ulock_wait` (fix round 1, I-2 — see that function's doc for the kernel measurement).
    /// Today it cannot fail, which is why its dispatch arms hardcode `err: false`.
    ///
    /// Reads and writes no guest memory, so the recorded event carries no writes and replay's
    /// mirror recomputes this identically from the same `(num, args)`.
    ///
    /// M17: returns `(rc, woken_tids)`. `rc` is unchanged (always 0 — see above). The tids are what
    /// the dispatch arms need in order to materialise a signal pended on a thread that was blocked:
    /// after the wake those threads are merely `Runnable`, indistinguishable from threads that were
    /// never blocked, so the wake must report them as it performs them. Both dispatch loops
    /// destructure the same pair from the same call with the same args, so symmetry rule 1 holds.
    pub fn guest_ulock_wake(&mut self, args: [u64; 8]) -> (u64, Vec<usize>) {
        // The measured wake operation is exactly one word (`mov w0,#2` + `movk w0,#0x100,lsl#16` at
        // `__pthread_joiner_wake` 0x66f4). Fail loud on anything else, the same posture
        // `guest_ulock_wait` takes — and for a sharper reason than width: `ULF_WAKE_THREAD`
        // (0x200) names a specific thread PORT in x2 instead of waking by address, so silently
        // treating it as an address-wake would wake the WRONG thread. The wait side's
        // `0x1020002` is deliberately NOT accepted here: `ULF_WAIT_CANCEL_POINT` (0x20000) is a
        // wait-only flag and has no meaning on a wake.
        assert!(args[0] as u32 == 0x1000002,
            "guest_ulock_wake: unmeasured operation word {:#x} — only __pthread_joiner_wake's plain \
             address wake is modelled; ULF_WAKE_THREAD names a thread port in x2 and would wake the \
             wrong thread if treated as an address wake", args[0]);
        let woken = self.threads.unblock_waiters_on(args[1]);
        (0, woken)
    }

    /// `semaphore_wait_trap(port)` — mach trap `-36`, the wait half of the pair
    /// `dispatch_semaphore_wait` lowers to. `args[0]` is the semaphore's port name.
    ///
    /// **Never forwarded.** Forwarding it is not whole-process-*fatal* the way forwarding
    /// `bsdthread_create` is — no host thread is created and nothing jumps to a guest address — but
    /// it is whole-process-*hanging*: it blocks retrace's own process on a semaphore that only the
    /// guest's own worker could ever signal, and nothing in retrace's process will. Both Stage 2a
    /// measurement runs ended on this very trap having produced zero bytes of guest stdout, and the
    /// one whose exit code was captured died to a 120-second alarm — a hang, not a crash
    /// (`docs/superpowers/specs/2026-08-21-retrace-m18-stage2b-measurements.md`, its run table and
    /// §2). A hang is the expensive failure to diagnose precisely because it produces no output to
    /// diagnose it with, which is why Stage 2b Task 2 put a fail-loud guard in front of the whole
    /// trap family.
    ///
    /// **Returns 0, and 0 is the only safe answer** — §3's table, measured, not chosen:
    /// `_dispatch_sema4_wait` returns cleanly ONLY on `0`; `0xe` sends libdispatch round a retry
    /// loop that re-issues `-36` forever (`0x18d3ec49c b.eq 0x18d3ec488`), `0xf` is fatal, and every
    /// other value reaches a `cold` routine.
    ///
    /// It returns 0 *unconditionally*, with no already-satisfied re-read — the one structural
    /// difference from `guest_ulock_wait`, whose guard re-reads the compared word out of guest
    /// memory. There is nothing to re-read: a semaphore's count lives inside libdispatch's own
    /// object, not at an address this trap names, and the trap is only reached on the SLOW path
    /// (§3's table names it as such), i.e. after the guest's own counter decrement has already
    /// decided it must block. The signal side of that same fast/slow split is measured outright in
    /// §3c — a bare `ldaddl` with no trap.
    ///
    /// **There is deliberately no operation-word assert**, unlike its two neighbours.
    /// `guest_ulock_wait` and `guest_ulock_wake` both refuse an unmeasured `args[0]` because syscall
    /// 515/516 multiplex many operations through ONE syscall number, and an unmeasured operation
    /// width would silently deadlock. The semaphore traps do not multiplex: the trap number *is* the
    /// operation — §3a of
    /// `docs/superpowers/specs/2026-08-23-retrace-m18-stage2b-wqthread-measurements.md` read all
    /// seven of the family's stubs off libsystem_kernel and they are seven distinct numbers — and
    /// `args[0]` is the port. There is no operation word here to guard.
    pub fn guest_sem_wait(&mut self, args: [u64; 8]) -> u64 {
        self.threads.block(thread::BlockReason::Sem { port: args[0] });
        0
    }

    /// `semaphore_signal_trap(port)` — mach trap `-33`. `args[0]` is the port name; returns the
    /// woken tids for the reason `guest_ulock_wake` does: M17 materialises a signal pended on a
    /// thread that could not run at the wake that makes it runnable, and that site needs the
    /// identity of who woke.
    ///
    /// **Never forwarded**, for `guest_sem_wait`'s reason exactly — and **no operation-word assert**,
    /// for `guest_sem_wait`'s reason exactly (the trap number is the operation).
    ///
    /// **Returns 0 (`KERN_SUCCESS`), and again 0 is the only safe answer** (§3's table):
    /// `_dispatch_sema4_signal` treats a non-zero return as fatal (`cbnz w0, …cold.7`), with
    /// `-301` separately fatal (`cmn w0, #0x12d`). Returned whether or not anyone was parked.
    ///
    /// An empty `woken` is a legal, ordinary outcome, not an error: §5 item 7 measured that the
    /// signal's **fast path issues no trap at all** — a bare `ldaddl` on the counter at `sem+0x30`,
    /// with only the negative (a waiter exists) case falling through to the trap. So a worker
    /// signalling a semaphore nobody is parked on produces no landmark whatsoever, and this seam
    /// must never read "woke nobody" as a lost wake.
    ///
    /// Reads and writes no guest memory, so the recorded event carries no writes and replay's mirror
    /// recomputes this identically from the same `(num, args)`.
    pub fn guest_sem_signal(&mut self, args: [u64; 8]) -> (u64, Vec<usize>) {
        let woken = self.threads.unblock_sem_waiters_on(args[0]);
        // **`-33` wakes exactly ONE waiter; the plural case is a DIFFERENT TRAP.** Task 1 §3a read
        // the whole family off libsystem_kernel's own stubs, and `-34` is `_semaphore_signal_all_trap`
        // — a separate selector, still refused by `is_mach_semaphore_trap`'s guard because no run has
        // ever issued it. `unblock_sem_waiters_on` wakes every waiter on the port, which is the right
        // shape for that future `-34` arm and the WRONG one here the moment two threads park on one
        // semaphore: the extra thread would return 0 from a wait it should still be blocked in, and
        // it would do so silently, with record and replay agreeing.
        //
        // Refused BY VALUE rather than documented, matching this file's neighbours — `guest_ulock_wake`'s
        // operation-word assert, `guest_workq_park`'s `(0,0,0)` assert, M17's `deliver_to.len() <= 1`.
        // A sentence does not stop a wrong answer. No fixture in this tree parks two threads on one
        // port, which is exactly why the bound has to be here rather than left to a future reader.
        //
        // Placed in the box, BELOW the trace, deliberately: both dispatch loops reach it through the
        // same call, so it is symmetric by construction and there is no second site to keep in step.
        //
        // What servicing the plural case owes first: an arm for `-34` in both loops, a decision on
        // whether `-33` should wake the LONGEST-parked waiter rather than the lowest-indexed one
        // (`unblock_sem_waiters_on` imposes table order, which is arbitrary once there are two), and
        // a guest that measures either.
        assert!(woken.len() <= 1,
            "semaphore_signal_trap (-33) on port {:#x} found {} waiters parked ({woken:?}), but \
             -33 wakes exactly ONE. Waking all of them is _semaphore_signal_all_trap (-34), a \
             separate trap this box does not service — it is still refused by name by \
             retrace_arch::is_mach_semaphore_trap. Servicing the plural case owes a -34 arm in \
             BOTH dispatch loops and a measured answer to WHICH waiter -33 wakes when several are \
             parked; table order is arbitrary. Measure a guest that parks two threads on one \
             semaphore before modelling this.",
            args[0], woken.len());
        (0, woken)
    }

    /// Pick and switch after the running thread blocked or exited.
    ///
    /// The pick is `ThreadTable::pick_next` — lowest-indexed runnable — which is a pure function of
    /// the guest's own syscall sequence. That is what lets record and replay schedule identically
    /// with NOTHING recorded and no trace-format change (symmetry rule 2).
    pub fn schedule_after_block(&mut self) {
        match self.threads.pick_next() {
            Some(tid) => self.switch_to_thread(tid),
            None => panic!(
                "M14: DEADLOCK — no runnable thread. {} live of {} total. States: {:?}",
                self.threads.live(),
                self.threads.len(),
                (0..self.threads.len()).map(|i| self.threads.state_of(i)).collect::<Vec<_>>()
            ),
        }
    }

    /// Switch the vCPU from the running thread to `tid`.
    pub fn switch_to_thread(&mut self, tid: usize) {
        let cur = self.threads.current();
        if cur == tid {
            return;
        }
        let saved = self.save_ctx();
        *self.threads.ctx_mut(cur) = saved;
        self.threads.switch_to(tid);
        let next = self.threads.ctx_of(tid).clone();
        self.load_ctx(&next);
    }

    /// Capture all backings + architectural registers as an Event::Snapshot.
    pub fn snapshot(&self) -> retrace_trace::Event {
        let mut mem = Vec::new();
        for bk in &self.backings {
            let bytes = unsafe { std::slice::from_raw_parts(bk.host, bk.len) }.to_vec();
            mem.push(Region { ipa: bk.ipa, bytes });
        }
        retrace_trace::Event::Snapshot { regs: self.regs_snapshot(), mem }
    }
    /// The post-`svc` return address (ELR_EL1) — the execution position at a syscall trap.
    pub fn position(&self) -> u64 { self.vcpu.get_sys(sysreg::ELR_EL1).unwrap() }
    /// The current PC (for non-syscall exits).
    pub fn pc(&self) -> u64 { self.vcpu.get_reg(reg::PC).unwrap() }
    /// macOS 26 reads the guest's current CPU number from TPIDR_EL0[11:0] and cluster number from
    /// TPIDR_EL0[>=12] (`_os_cpu_number` / `_os_cpu_cluster_number`); a single-vCPU guest must
    /// present 0 here (cpu 0 / cluster 0). Test-facing accessor (M2-cpuid).
    pub fn tpidr_el0(&self) -> u64 { self.vcpu.get_sys(sysreg::TPIDR_EL0).unwrap() }
    /// The guest's TSD base (thread pointer): errno slot, pthread-self, etc. are read through it.
    /// Test-facing accessor (M2-cpuid).
    pub fn tpidrro_el0(&self) -> u64 { self.vcpu.get_sys(sysreg::TPIDRRO_EL0).unwrap() }

    /// Shared x0..x30 block, four per line — the part of `dbg_regs`'s layout that a saved
    /// `ThreadCtx` can also supply, factored out so `dbg_regs_of` renders a NON-current thread in
    /// the exact same shape without duplicating the format string.
    fn format_gprs(x: &[u64; 31]) -> String {
        let mut s = String::new();
        for (i, xi) in x.iter().enumerate() {
            s += &format!("x{i:<2}={xi:#018x}  ");
            if i % 4 == 3 { s.push('\n'); }
        }
        s
    }

    /// Bring-up diagnostic: dump x0..x30, SP_EL0, PC, ELR/SPSR/FAR as a multi-line string.
    pub fn dbg_regs(&self) -> String {
        let mut x = [0u64; 31];
        for (i, xi) in x.iter_mut().enumerate() { *xi = self.vcpu.get_reg(reg::x(i as u32)).unwrap(); }
        let mut s = Self::format_gprs(&x);
        s += &format!("\nsp={:#x} pc={:#x} elr={:#x} far={:#x} spsr={:#x}",
            self.vcpu.get_sys(sysreg::SP_EL0).unwrap(), self.pc(),
            self.vcpu.get_sys(sysreg::ELR_EL1).unwrap(), self.last_far, self.spsr());
        s
    }

    /// M15: render a specific thread's registers. For the CURRENT thread this reads the live vCPU,
    /// because the table's slot is stale between switches (only `switch_to_thread` writes it back)
    /// — the same reason `checkpoint()` folds the vCPU in before cloning the table. For any other
    /// thread the table IS the authority: that thread is not on the vCPU.
    ///
    /// `far` has no per-thread meaning (`last_far` is the box's last fault address, not a saved
    /// register) — 0 for a non-current thread, the honest "not applicable" value, rather than
    /// reusing the box's possibly-unrelated last fault.
    pub fn dbg_regs_of(&self, tid: usize) -> Option<String> {
        if tid >= self.threads.len() { return None; }
        if tid == self.threads.current() {
            return Some(self.dbg_regs());
        }
        let ctx = self.threads.ctx_of(tid);
        let mut s = Self::format_gprs(&ctx.regs.x);
        s += &format!("\nsp={:#x} pc={:#x} elr={:#x} far={:#x} spsr={:#x}",
            ctx.regs.sp_el0, ctx.regs.pc, ctx.elr, 0, ctx.spsr);
        Some(s)
    }

    /// Bring-up diagnostic: walk the guest AArch64 frame-pointer chain from x29, returning up to
    /// `max` return addresses (PAC bits stripped by masking to the 36-bit IPA space). Return
    /// addresses are `pacibsp`-signed on the stack; masking recovers the raw VA (dyld ~0x1_4000_0000).
    pub fn dbg_backtrace(&self, max: usize) -> String {
        let mut s = String::new();
        let mut fp = self.vcpu.get_reg(reg::x(29)).unwrap();
        let pc = self.vcpu.get_reg(reg::PC).unwrap();
        s += &format!("  pc  {pc:#x}\n");
        for _ in 0..max {
            if fp == 0 || self.host_span(fp).is_none() || fp & 0x7 != 0 { break; }
            let saved_fp = self.read_u64(fp);
            let ret = self.read_u64(fp + 8) & 0x0000_000F_FFFF_FFFF; // strip PAC bits
            if ret == 0 { break; }
            s += &format!("  ret {ret:#x}\n");
            if saved_fp <= fp { break; } // stacks grow down; fp must increase up the chain
            fp = saved_fp;
        }
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

    /// Capture COMPLETE internal state at the current (arbitrary) position — the M4 checkpoint
    /// primitive. Unlike `snapshot()` (trace format, landmark-0-only-correct on restore), this
    /// additionally captures FP/SIMD state and the true values of every field `restore()` defaults.
    pub fn checkpoint(&self) -> BoxState {
        let mut mem = Vec::new();
        for bk in &self.backings {
            let bytes = unsafe { std::slice::from_raw_parts(bk.host, bk.len) }.to_vec();
            mem.push(Region { ipa: bk.ipa, bytes });
        }
        let mut fp = [0u128; 32];
        for (i, fi) in fp.iter_mut().enumerate() { *fi = self.vcpu.get_simd(simd::q(i as u32)).unwrap(); }
        BoxState {
            regs: self.regs_snapshot(), fp,
            fpcr: self.vcpu.get_reg(reg::FPCR).unwrap(),
            fpsr: self.vcpu.get_reg(reg::FPSR).unwrap(),
            tpidr_el0: self.vcpu.get_sys(sysreg::TPIDR_EL0).unwrap(),
            elr: self.vcpu.get_sys(sysreg::ELR_EL1).unwrap(),
            spsr: self.vcpu.get_sys(sysreg::SPSR_EL1).unwrap(),
            mem,
            reservations: self.reservations.clone(),
            noaccess: self.noaccess.clone(),
            // M14: the RUNNING thread's context lives on the vCPU, not in the table — the table's
            // copy is only refreshed by `switch_to_thread`, so it is stale for whoever is running.
            // Fold the live registers back in before carrying it, or a checkpoint taken between two
            // switches restores the current thread to wherever it was at the LAST switch.
            threads: {
                let mut t = self.threads.clone();
                let cur = t.current();
                *t.ctx_mut(cur) = self.save_ctx();
                t
            },
            // M14 t7 fix round 1: carried, not re-derived — see the field comment on `BoxState`.
            thread_start_pc: self.thread_start_pc,
            // M18: carried for the same reason — see the `BoxState` field comment.
            wq_thread_pc: self.wq_thread_pc,
            pthread_size: self.pthread_size,
            mmap_next: self.mmap_next,
            bootstrap_port: self.bootstrap_port,
            cache_installed: self.cache.is_some(),
            last_far: self.last_far,
            synthetic_tsc: self.synthetic_tsc,
            cache_refault_ipa: self.cache_refault_ipa,
            cache_refault_count: self.cache_refault_count,
            pac_enabled: self.pac_enabled,
            stack_top: self.stack_top,
            stack_size: self.stack_size,
            fd_slots: self.fds.slots(),
            sigtable: self.sigtable.clone(),
        }
    }

    /// Rebuild a fresh guest from a `BoxState` captured at an ARBITRARY position — the M4 twin of
    /// `restore()` (which is only correct at landmark 0). Mirrors `restore()`'s structure exactly
    /// (fresh VM/vcpu, remap every backing, fixed EL1 sysregs, PAC keys, `set_trap_debug_exceptions`)
    /// but restores the TRUE captured values instead of `restore()`'s landmark-0 defaults for
    /// `reservations`/`mmap_next`/`bootstrap_port`/`cache`/`tpidr_el0`, plus FP/SIMD state `restore()`
    /// never touched at all. `l2_host`/`next_l3` are recomputed from the freshly-allocated `backings`
    /// — never stored raw (they are host pointers, meaningless across VM instances).
    pub fn from_checkpoint(state: &BoxState) -> Box_ {
        let vm = Vm::create().expect("hv_vm_create");
        let vcpu = Vcpu::create(&vm).expect("hv_vcpu_create");
        let mut backings = Vec::new();
        for r in &state.mem {
            let (host, len) = alloc_pages(r.bytes.len().max(GRANULE));
            unsafe { std::ptr::copy_nonoverlapping(r.bytes.as_ptr(), host, r.bytes.len()); }
            vm.map(host, r.ipa, len, MemFlags::RWX).expect("hv_vm_map (from_checkpoint)");
            backings.push(Backing { host, ipa: r.ipa, len });
        }
        vcpu.set_sys(sysreg::MAIR_EL1,  MAIR_EL1_V).unwrap();
        vcpu.set_sys(sysreg::TCR_EL1,   TCR_EL1_V).unwrap();
        vcpu.set_sys(sysreg::TTBR0_EL1, PT_L1_IPA).unwrap();
        vcpu.set_sys(sysreg::CPACR_EL1, CPACR_FP_ON).unwrap();
        // M14 t6: the thread pointer is PER-THREAD now, so a constant here is wrong the moment a
        // checkpoint is taken while a child runs — it would hand that thread MAIN's TSD. The
        // captured table is the authority (`checkpoint` folds the live vCPU into `ctx_mut(current)`
        // before carrying it), and for a single-threaded guest that captured value IS `TSD_IPA`, so
        // every M0–M13 restore is identical to what this line did before. That makes it the fourth
        // field here to stop being re-derived for the reason the `BoxState` comments give (M9 t3,
        // M10, M11): a restore that contradicts the state it just restored breaks quietly.
        let cur_tid = state.threads.current();
        vcpu.set_sys(sysreg::TPIDRRO_EL0, state.threads.ctx_of(cur_tid).tpidrro_el0).unwrap();
        vcpu.set_sys(sysreg::TPIDR_EL0, state.tpidr_el0).unwrap(); // captured, NOT forced to 0
        Self::set_pac_keys(&vcpu);
        vcpu.set_sys(sysreg::SCTLR_EL1, sctlr_mmu_on(state.pac_enabled)).unwrap();
        vcpu.set_sys(sysreg::VBAR_EL1, TRAMPOLINE_IPA).unwrap();
        vcpu.set_trap_debug_exceptions(true).unwrap(); // must not be omitted or step() stops trapping
        for i in 0..31 { vcpu.set_reg(reg::x(i as u32), state.regs.x[i]).unwrap(); }
        vcpu.set_reg(reg::PC, state.regs.pc).unwrap();
        vcpu.set_reg(reg::CPSR, state.regs.cpsr).unwrap();
        vcpu.set_sys(sysreg::SP_EL0, state.regs.sp_el0).unwrap();
        vcpu.set_sys(sysreg::ELR_EL1, state.elr).unwrap();   // captured EL1 return address (dead at a
        vcpu.set_sys(sysreg::SPSR_EL1, state.spsr).unwrap(); // step boundary, live at a syscall trap)
        vcpu.set_reg(reg::FPCR, state.fpcr).unwrap();
        vcpu.set_reg(reg::FPSR, state.fpsr).unwrap();
        for (i, &v) in state.fp.iter().enumerate() { vcpu.set_simd(simd::q(i as u32), v).unwrap(); }
        let l2_host = backings.iter().find(|b| b.ipa == PT_L2_IPA).map(|b| b.host)
            .unwrap_or(std::ptr::null_mut());
        for b in backings.iter().filter(|b| b.ipa >= PT_L3_BASE && b.ipa < PT_L3_CEIL) {
            assert_eq!(b.len, GRANULE,
                "from_checkpoint: backing at L3-window ipa {:#x} has len {} != GRANULE", b.ipa, b.len);
        }
        let next_l3 = backings.iter()
            .filter(|b| b.ipa >= PT_L3_BASE && b.ipa < PT_L3_CEIL)
            .map(|b| b.ipa + GRANULE as u64).max().unwrap_or(PT_L3_BASE);
        // M9 fix: if a flush ever ran before this checkpoint was captured, the TLBI stub is one of
        // `backings` (checkpoint() captures every backing) and was just re-mapped above — so it must
        // NOT be re-mapped again by a later `ensure_tlbi_stub()`, which would double-map its IPA and
        // panic (`hv_vm_map` rejects an overlapping range). The page table entry (ATTR_TRAMP) is
        // already restored as part of `state.mem`, so deriving readiness from the restored backings
        // is enough; nothing else needs redoing.
        let tlbi_stub_ready = backings.iter().any(|b| b.ipa == TLBI_STUB_IPA);
        let mut b = Box_ {
            vm, vcpu, backings,
            reservations: state.reservations.clone(),
            noaccess: state.noaccess.clone(),
            mmap_next: state.mmap_next,
            bootstrap_port: state.bootstrap_port,
            l2_host, next_l3,
            last_far: state.last_far,
            synthetic_tsc: state.synthetic_tsc,
            cache_refault_ipa: state.cache_refault_ipa,
            cache_refault_count: state.cache_refault_count,
            cache: None,
            bps_armed: false,
            wps_armed: false,
            watch_ranges: Vec::new(),
            syscall_watch_hit: None,
            pac_enabled: state.pac_enabled,
            stack_top: state.stack_top,
            stack_size: state.stack_size,
            tlbi_stub_ready,
            // M10: DERIVED from the captured slots, never reset — see the BoxState field comment.
            // A fresh table here would tell a seeked session every fd is Free.
            fds: FdTable::from_slots(&state.fd_slots),
            // M11: RESTORED from the capture, never reset — see the BoxState field comment. A fresh
            // table here would tell a seeked session every signal is at its default disposition.
            sigtable: state.sigtable.clone(),
            // M14 t6: RESTORED from the capture, never reset — see the BoxState field comment. A
            // fresh table here would tell a seeked session the guest has exactly one thread, so the
            // scheduler would never pick the child and a join on it would deadlock. The RUNNING
            // thread's registers are additionally loaded onto the vCPU above, from the flat fields
            // that describe it; the table agrees with them, having been folded at capture.
            threads: state.threads.clone(),
            // M14 t7 fix round 1: RESTORED from the capture, never reset — see the `BoxState` field
            // comment. A `None` here would tell a restored session it never saw the guest's own
            // `bsdthread_register`, even when it did — the fail-loud `.expect()` this field exists
            // to prevent.
            thread_start_pc: state.thread_start_pc,
            // M18: RESTORED from the capture, never reset — see the `BoxState` field comment. A
            // `None` here would tell a restored session it never saw the guest's own
            // `bsdthread_register`, even when it did.
            wq_thread_pc: state.wq_thread_pc,
            pthread_size: state.pthread_size,
        };
        if state.cache_installed { b.install_cache_pager(); }
        b
    }

    /// Bring-up/test diagnostic: dump V0..V31, FPCR, FPSR — the checkpoint round-trip's FP half of
    /// `dbg_regs()`. Kept separate from `dbg_regs()` deliberately: `dbg_regs()` is the `debug` CLI's
    /// `regs` command output (a byte-compare contract for existing golden-transcript tests); this is
    /// test-only and never wired into the CLI.
    pub fn dbg_fp_regs(&self) -> String {
        let mut s = String::new();
        for i in 0..32u32 {
            s += &format!("q{i:<2}={:#034x}  ", self.vcpu.get_simd(simd::q(i)).unwrap());
            if i % 2 == 1 { s.push('\n'); }
        }
        s += &format!("fpcr={:#x} fpsr={:#x}", self.vcpu.get_reg(reg::FPCR).unwrap(), self.vcpu.get_reg(reg::FPSR).unwrap());
        s
    }

    /// Test-only diagnostic: the internal bookkeeping fields `checkpoint()`/`from_checkpoint()` must
    /// round-trip that have no other observable accessor. Never used by production code.
    #[doc(hidden)]
    pub fn dbg_internal_state(&self) -> String {
        format!("reservations={:?} mmap_next={:#x} bootstrap_port={:?} cache_installed={} last_far={:#x} synthetic_tsc={:#x} cache_refault_ipa={:#x} cache_refault_count={} pac_enabled={}",
            self.reservations, self.mmap_next, self.bootstrap_port, self.cache.is_some(),
            self.last_far, self.synthetic_tsc, self.cache_refault_ipa, self.cache_refault_count,
            self.pac_enabled)
    }

    /// Test-only: the guest's live PAC posture, read back from SCTLR_EL1 and cross-checked against
    /// the field the constructor derived. PANICS if they disagree — i.e. if some install site set
    /// SCTLR without going through `sctlr_mmu_on(pac_enabled)`. A posture mismatch between the four
    /// sites otherwise fails LATE (a replay divergence, or a silently mis-seeked session); this
    /// makes it fail in a unit test instead.
    #[doc(hidden)]
    pub fn dbg_pac_enabled(&self) -> bool {
        let live = self.vcpu.get_sys(sysreg::SCTLR_EL1).unwrap() & SCTLR_PAC_EN != 0;
        assert_eq!(live, self.pac_enabled,
            "SCTLR_EL1 PAC bits ({live}) disagree with Box_::pac_enabled ({}) — an install site \
             bypassed sctlr_mmu_on()", self.pac_enabled);
        live
    }

    /// Read back the hardware watchpoint slot-0 registers and MDSCR_EL1 straight off the vCPU.
    /// Test-only (M15 Task 6): debug registers are vCPU-global and deliberately absent from
    /// `ThreadCtx`, so a watchpoint armed before a context switch must still be armed after one.
    /// Nothing in production reads these back — `arm_hw_watchpoint`/`clear_hw_watchpoints` only
    /// write them — so without this there is no way to assert the hardware leaf rather than the
    /// `watch_ranges` software mirror, and a test of the mirror alone would pass even if
    /// `load_ctx` wiped MDSCR_EL1. Returns `(DBGWVR0_EL1, DBGWCR0_EL1, MDSCR_EL1)`.
    #[doc(hidden)]
    pub fn dbg_watch0_hw(&self) -> (u64, u64, u64) {
        (
            self.vcpu.get_sys(sysreg::DBGWVR0_EL1).unwrap(),
            self.vcpu.get_sys(sysreg::DBGWCR0_EL1).unwrap(),
            self.vcpu.get_sys(sysreg::MDSCR_EL1).unwrap(),
        )
    }
}

/// One entry of `thread_of_port`'s panic diagnostic: `(tid=…, kport=…[, exited])`.
///
/// Pure and VM-free — factored out of `thread_of_port` (fix round 1, review finding 1) so the
/// format can be pinned by a plain unit test instead of only ever being read inside a panic
/// message from a real VM run. `kport` stays hex-with-prefix (matches the `{port:#x}` the target
/// port itself is printed with); "unreadable" instead of a 0 sentinel distinguishes an unreadable
/// pthread from a thread whose kport genuinely reads 0. `tid` stays plain decimal — the earlier
/// `{seen:x?}` swept it into hex along with the kport, ambiguous the moment a guest has >= 10
/// threads (`a` misreadable as decimal 10).
fn format_seen_entry(tid: usize, kport: Option<u32>, exited: bool) -> String {
    let kport = match kport {
        Some(k) => format!("{k:#x}"),
        None => "unreadable".to_string(),
    };
    format!("(tid={tid}, kport={kport}{})", if exited { ", exited" } else { "" })
}

#[cfg(test)]
mod thread_of_port_diagnostic_tests {
    use super::*;

    // No VM: `format_seen_entry` is the pure formatting slice of `thread_of_port`'s panic message.
    // Asserts on substrings, not the whole string, so it pins the two things review finding 1 and
    // 2 were actually about (tid stays decimal, kport stays hex, exited is tagged) without pinning
    // wording that isn't the point.
    #[test]
    fn tid_stays_decimal_and_kport_stays_hex_with_prefix() {
        let entry = format_seen_entry(10, Some(0xff), false);
        assert!(entry.contains("tid=10"), "tid must stay decimal, not fold into hex: {entry}");
        assert!(!entry.contains("tid=a"), "tid must NOT get swept into hex formatting: {entry}");
        assert!(entry.contains("kport=0xff"), "kport must stay hex-with-prefix: {entry}");
        assert!(!entry.contains("exited"), "a live (non-exited) thread must not be tagged: {entry}");
    }

    #[test]
    fn exited_and_unreadable_are_distinguishable_in_the_dump() {
        let exited = format_seen_entry(3, None, true);
        assert!(exited.contains("exited"), "an already-exited thread must be tagged: {exited}");
        assert!(exited.contains("kport=unreadable"),
            "a None kport must read as unreadable, not a bare '0' that could be confused with a \
             thread whose kport genuinely reads 0: {exited}");
    }
}

#[cfg(test)]
mod pac_posture_tests {
    use super::*;

    // `pac_posture_from_memory` is PRIVATE (restore()'s only route to re-derive PAC posture from a
    // bare snapshot). It must FAIL LOUD — never default — when EXE_BASE doesn't hold MH_MAGIC_64,
    // because a silent PAC-off fallback is indistinguishable from correct for every guest this repo
    // can build today (all plain arm64); it would hide a broken derivation at exactly the moment an
    // arm64e guest arrives. No VM needed — this exercises the pure byte-parsing path directly.
    #[test]
    #[should_panic(expected = "no MH_MAGIC_64 at EXE_BASE")]
    fn pac_posture_from_memory_fails_loud_on_corrupted_magic() {
        // A region covering EXE_BASE..EXE_BASE+12, but with a corrupted (non-Mach-O) magic in the
        // first 4 bytes — never a silent default.
        let mut bytes = vec![0u8; 16];
        bytes[0..4].copy_from_slice(&0xdead_beef_u32.to_le_bytes()); // NOT 0xfeedfacf
        let regions = vec![Region { ipa: EXE_BASE, bytes }];

        // Confirm the panic is actually reachable, not just declared: this call is the only thing
        // this test does, so if `pac_posture_from_memory` ever stopped panicking here (e.g. someone
        // "fixed" it into a silent default), `#[should_panic]` would fail the test for real.
        let _ = pac_posture_from_memory(&regions);
    }
}

// M8-stack. `Box_` must know its OWN stack, per load path — the static path's stack is one granule
// below STACK_TOP_IPA, the dynamic path's is DYN_STACK_SIZE below DYN_STACK_TOP. `restore()` rebuilds
// both from a bare snapshot, so it re-derives the geometry rather than hardcoding either constant;
// these tests pin the derivation (pure, no VM) and the static loader's own bookkeeping.
#[cfg(test)]
mod stack_geometry_tests {
    use super::*;

    // A minimal snapshot region list: the stack backing under test, plus whatever else the caller
    // wants. `bytes` is sized, never filled — only ipa/len are consulted.
    fn region(ipa: u64, len: u64) -> Region { Region { ipa, bytes: vec![0u8; len as usize] } }

    #[test]
    fn static_load_records_its_own_stack_geometry() {
        let bytes = std::fs::read(retrace_guest::HELLO).unwrap();
        let loaded = retrace_guest::parse_macho(&bytes);
        let b = Box_::load(&loaded);
        assert_eq!(b.stack_top(), STACK_TOP_IPA, "static stack top");
        assert_eq!(b.stack_size(), GRANULE as u64, "static stack is exactly one granule");
        assert_eq!(b.stack_top() - b.stack_size(), STACK_TOP_IPA - GRANULE as u64,
                   "the computed stack bottom must equal the IPA load() actually maps");
    }

    #[test]
    fn derives_the_static_geometry_from_a_static_snapshot() {
        let regions = vec![region(STACK_TOP_IPA - GRANULE as u64, GRANULE as u64)];
        assert_eq!(stack_geometry_from_memory(&regions), (STACK_TOP_IPA, GRANULE as u64));
    }

    #[test]
    fn derives_the_dynamic_geometry_from_a_dynamic_snapshot() {
        let regions = vec![region(DYN_STACK_TOP - DYN_STACK_SIZE, DYN_STACK_SIZE)];
        assert_eq!(stack_geometry_from_memory(&regions), (DYN_STACK_TOP, DYN_STACK_SIZE));
    }

    // Fail loud, never default: a snapshot naming no stack we recognize must not silently answer
    // with one path's constants, because that is precisely the "the guest is told a lie about its
    // own address space" bug M8-stack exists to remove.
    #[test]
    #[should_panic(expected = "refusing to guess a stack geometry")]
    fn fails_loud_when_no_stack_is_identifiable() {
        let _ = stack_geometry_from_memory(&[region(EXE_BASE, GRANULE as u64)]);
    }

    // The dynamic stack must sit high enough that the main-thread GUARD PAGE libstd computes is a
    // real guest address. libstd's `install_main_guard` mmaps MAP_FIXED at
    // `pthread_get_stackaddr_np() - pthread_get_stacksize_np()`, and macOS 26's libpthread reports
    // `MAIN_STACK_SIZE` for the main thread as a CONSTANT — it calls `getrlimit(RLIMIT_STACK)` and
    // then ignores the reply (measured: answering 0x10000000 instead of 0x40000 left the computed
    // address bit-identical). So retrace cannot influence the subtrahend; it can only make sure the
    // minuend leaves room. With the stack top at 2 MiB that subtraction UNDERFLOWED to
    // 0xffffffffffa04000, which is what parked rung 1.
    //
    // This is a pure constant check on purpose: it is instant, it runs on every gate, and it fails
    // the moment someone edits the layout back into a collision — which is the failure mode that
    // cost this milestone two walls. The end-to-end proof is `hello_rust_e2e`.
    #[test]
    fn the_guard_page_libstd_computes_is_a_mappable_guest_address() {
        // macOS 26 libpthread's main-thread stack size: 8 MiB minus one 16 KiB page.
        let guard = DYN_STACK_TOP.checked_sub(LIBPTHREAD_MAIN_STACK_SIZE).unwrap_or_else(|| panic!(
            "DYN_STACK_TOP {DYN_STACK_TOP:#x} is below libpthread's constant main-thread stack size \
             {LIBPTHREAD_MAIN_STACK_SIZE:#x}: libstd's guard-page subtraction underflows to a wild \
             address and the mmap is refused"));

        assert_eq!(guard % GRANULE as u64, 0, "guard page {guard:#x} must be granule-aligned");
        assert!(guard >= PT_L3_CEIL,
            "guard page {guard:#x} lands in the L3 page-table window [{PT_L3_BASE:#x}, \
             {PT_L3_CEIL:#x}) — mapping it would overwrite live translation tables");
        assert!(guard + GRANULE as u64 <= DYN_STACK_TOP - DYN_STACK_SIZE,
            "guard page {guard:#x} overlaps the stack backing [{:#x}, {DYN_STACK_TOP:#x}) — it must \
             sit BELOW the stack it guards", DYN_STACK_TOP - DYN_STACK_SIZE);
        const { assert!(DYN_STACK_TOP <= 1 << 36, "the stack must fit in the 36-bit guest IPA space") };
    }
}

// M18. `bsdthread_register`'s feature word cannot be forwarded to the host (the host is retrace's
// own already-registered process, and returns EINVAL — see WORKQ_FEATURE_WORD's doc comment), so
// this pins the SYNTHESIZED value against the four gates it was measured against in the shipped
// binaries, not against the literal itself — a test that only restated 0x4000005E would pass even
// if the value were wrong for its purpose.
#[cfg(test)]
mod workq_feature_word_tests {
    use super::*;

    #[test]
    fn the_feature_word_satisfies_every_gate_it_was_measured_against() {
        let w = WORKQ_FEATURE_WORD;
        // libpthread __pthread_init (+0x1040): `cmp w0,#1 / b.lt skip` — a value < 1 means the
        // feature word is never stored at all, which is the M18 Stage-1 bug.
        assert!(w >= 1, "must be >= 1 or libpthread skips the store");
        // libpthread __pthread_init (+0x1048): `mov w8,#0x1e / movk w8,#0x4000,lsl#16 /
        // bics wzr,w8,w0 / b.ne crash` — every bit of 0x4000001E must be present.
        assert_eq!(w & 0x4000_001E, 0x4000_001E, "libpthread requires all of 0x4000001E");
        // libdispatch _dispatch_root_queues_init_once (0x180348F68): `tbz w0,#4 -> .cold.5`.
        assert_ne!(w & (1 << 4), 0, "bit 4 clear crashes libdispatch at .cold.5");
        // libdispatch (0x180348F90/F94): bit 7 set registers THREE worker callbacks including the
        // workloop worker; bit 7 clear with bit 6 set registers two. Neither set is .cold.4.
        // Bit 7 is deliberately clear — it is the scope lever, not an accident.
        assert_ne!(w & (1 << 6), 0, "bit 6 must be set when bit 7 is clear, else .cold.4");
        assert_eq!(w & (1 << 7), 0, "bit 7 must stay CLEAR to keep the workloop worker out of scope");
    }
}
