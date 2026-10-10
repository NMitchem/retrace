//! M49 §3e (X1, X2): the store-exclusive rewrite.
//!
//! Libsyscall's hand-written `__getpid` caches the pid with `ldxr w10, [x9]; cbnz w10, …;
//! stxr wzr, w0, [x9]` and discards the store's status. Any exception return between the halves
//! clears the local monitor (`note_exit`, Sail `AArch64_ExceptionReturn`; a host IRQ is enough), so
//! the store can be lost, and nothing retries it. The next `getpid()` in that image then issues a
//! syscall the other run does not (static `excl-report.md` §1). A box-emulated trap between the
//! halves loses it every time (`mrspair.c`).
//!
//! The box therefore rewrites every discarded-status pair into the plain accesses an uninterrupted
//! run performs, as it stages code. X3 reaches `rewrite_staged_code` in `lib.rs` from four calls in
//! three functions: the cache TEXT pager (`page_in_cache`), `load_dynamic`'s executable and dyld
//! segments (two calls), and the executable `mmap` (`guest_mmap_file`). That is correct (X2)
//! because retrace runs one vCPU, threads switch only at syscall landmarks, and the box writes guest
//! memory only at exits: the uninterrupted outcome, the store succeeding, is always architecturally
//! allowed. A spurious failure is permitted but never required, and the guest discards the status
//! anyway.
//!
//! Everything here is a pure function of the words, so record and replay rewrite identically
//! (symmetry rule 2). It reuses M42's decoder and backward scan (`excl.rs`, `retrace_arch`).
use crate::excl::{base_aliases_dest, regs_written, scan_back};
use retrace_arch::{decode_excl, ExclInsn};

/// X1: the load-exclusive sits at most this many words before its store.
const WINDOW: usize = 16;
/// Words in one 16 KiB guest page. X1 never matches a pair across a page boundary.
const PAGE_WORDS: usize = 0x4000 / 4;
/// A single-register store-exclusive of any size, plain or release, with status `wzr`: `o2 L o1`
/// (bits 23:21) clear, `Rs` (bits 20:16) and the should-be-one `Rt2` (bits 14:10) all ones. The
/// fast reject: every other word is skipped with one compare.
const STX_WZR_MASK: u32 = 0x3FFF_7C00;
const STX_WZR_BITS: u32 = 0x081F_7C00;
/// A load-exclusive's should-be-one `Rs` (bits 20:16) and `Rt2` (bits 14:10).
const LDX_SBO: u32 = 0x001F_7C00;

/// X3's entry: `words` is code staged at a 16 KiB-aligned guest address, so each 16 KiB chunk is one
/// guest page, and a pair whose halves straddle a page is never matched. Returns the number of
/// pairs rewritten.
pub fn rewrite_code_pages(words: &mut [u32]) -> usize {
    words.chunks_mut(PAGE_WORDS).map(rewrite_discarded_pairs).sum()
}

/// X1 and X2 over one slice, which the caller bounds at a page: rewrite every discarded-status pair
/// in place and return how many there were. A pair is matched only when all of these hold (R5):
/// - a single-register store-exclusive (`stxr`/`stlxr`, byte, half, word or doubleword) has `wzr`
///   as its status register, and a data register other than `wzr`;
/// - a single-register load-exclusive of the same size and base register sits at most 16 words
///   earlier, inside the slice, with nothing between them that `excl::scan_back` stops at (another
///   exclusive, `clrex`, or an instruction that does not fall through);
/// - the load does not overwrite its own base, and its should-be-one fields are all ones;
/// - every instruction between the halves is one `neutral_between` allows.
///
/// The load becomes the plain load of the same size and ordering (`ldxr` → `ldr`, `ldaxr` →
/// `ldar`), and the store the plain store (`stxr wzr` → `str`, `stlxr wzr` → `stlr`). Pair
/// instructions (`ldxp`/`stxp`) are never matched: the census has none (H7).
pub fn rewrite_discarded_pairs(words: &mut [u32]) -> usize {
    let mut n = 0;
    for j in 0..words.len() {
        if words[j] & STX_WZR_MASK != STX_WZR_BITS { continue; }
        if let Some(i) = paired_load(words, j) {
            words[i] = plain_load(words[i]);
            words[j] = plain_store(words[j]);
            n += 1;
        }
    }
    n
}

/// The index of the load-exclusive that the discarded-status store-exclusive at `j` pairs with
/// under X1, or None.
fn paired_load(words: &[u32], j: usize) -> Option<usize> {
    let Some(ExclInsn::Store { size, pair: false, rs: 31, rt, rn, .. }) = decode_excl(words[j]) else {
        return None;
    };
    // `stxr wzr, wzr, [xN]` names one register as both status and data: CONSTRAINED UNPREDICTABLE,
    // and `clang` refuses to assemble it. Left alone.
    if rt == 31 { return None; }
    // Nearest first, never past the slice's start: the scan `infer` uses for M42's §3d.
    let back: Vec<u32> = (1..=WINDOW.min(j)).map(|d| words[j - d]).collect();
    let (d, ld) = scan_back(&back)?;
    let i = j - 1 - d;
    let ExclInsn::Load { size: ld_size, pair: false, rn: ld_rn, .. } = ld else { return None };
    // `decode_excl` does not read the load's should-be-one fields; an encoding with any of them
    // clear is CONSTRAINED UNPREDICTABLE. Left alone.
    if words[i] & LDX_SBO != LDX_SBO { return None; }
    if ld_size != size || ld_rn != rn || base_aliases_dest(ld) { return None; }
    (i + 1..j).all(|k| neutral_between(words[k], k, j, rn)).then_some(i)
}

/// X1's rule for the instruction at `k`, strictly between a load at some `i < k` and its store at
/// `j`, whose base register is `rn`. Allowed, and nothing else:
/// - a system-register read (`mrs`, the `mrspair` shape) whose destination is not the base;
/// - a conditional branch (`B.cond`/`BC.cond`, `CBZ`/`CBNZ`, `TBZ`/`TBNZ`) whose target is the word
///   right after the store. It skips the store and nothing else (the getpid shape), so the two
///   paths meet again at `j + 1`;
/// - a data-processing instruction whose destination is not the base (`excl::regs_written`'s known
///   set, which counts an `Rd` of 31 as a write of SP, so an SP base refuses it).
///
/// Everything else refuses: a load or a store (a memory access, or a base write-back), a barrier,
/// `clrex`, a hint (`wfe` waits on the monitor), an unconditional branch, an exception, and SIMD or
/// FP, whose register effects `regs_written` does not know.
fn neutral_between(w: u32, k: usize, j: usize, rn: u32) -> bool {
    if w & 0xFFF0_0000 == 0xD530_0000 { return w & 0x1F != rn; } // mrs: writes Xt
    if let Some(off) = cond_branch_offset(w) { return k as i64 + off == j as i64 + 1; }
    regs_written(&[w]).is_some_and(|written| written & (1 << rn) == 0)
}

/// A conditional branch's target, in words relative to the branch, or None if `w` is not one.
fn cond_branch_offset(w: u32) -> Option<i64> {
    let imm = |bits: u32| i64::from((((w >> 5) << (32 - bits)) as i32) >> (32 - bits));
    if w & 0xFF00_0000 == 0x5400_0000 || w & 0x7E00_0000 == 0x3400_0000 {
        Some(imm(19)) // B.cond, BC.cond, CBZ, CBNZ: imm19 at bits 23:5
    } else if w & 0x7E00_0000 == 0x3600_0000 {
        Some(imm(14)) // TBZ, TBNZ: imm14 at bits 18:5
    } else {
        None
    }
}

/// X2's load. `ldxr{b,h,}` becomes `ldr{b,h,}` (immediate, unsigned offset, `#0`) and `ldaxr{b,h,}`
/// becomes `ldar{b,h,}`, keeping `size` (bits 31:30), `Rn` and `Rt` (bits 9:0).
///
/// Why the unsigned-offset form: an exclusive addresses `[Xn|SP]`, and so does `LDR (immediate)`
/// with `imm12 = 0`. `Rn = 31` is SP in both and `Rt = 31` is the zero register in both, so the
/// address and the destination are the same. It is the encoding `clang` emits for `ldr wT, [xN]`.
/// `LDAR`/`STLR` have only the `[Xn|SP]` form, and they keep the acquire and release.
///
/// One difference remains, and no census site can show it: an exclusive to an unaligned address
/// always takes an alignment fault, and a plain `ldr`/`str` on normal memory does not.
fn plain_load(w: u32) -> u32 {
    let (size, rn_rt) = (w >> 30, w & 0x3FF);
    if w & (1 << 15) == 0 { 0x3940_0000 | size << 30 | rn_rt } else { 0x08DF_FC00 | size << 30 | rn_rt }
}

/// X2's store: `stxr{b,h,} wzr` becomes `str{b,h,}` (immediate, unsigned offset, `#0`) and
/// `stlxr{b,h,} wzr` becomes `stlr{b,h,}`. See `plain_load`.
fn plain_store(w: u32) -> u32 {
    let (size, rn_rt) = (w >> 30, w & 0x3FF);
    if w & (1 << 15) == 0 { 0x3900_0000 | size << 30 | rn_rt } else { 0x089F_FC00 | size << 30 | rn_rt }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every word below was assembled with `clang -arch arm64 -c` and read back with `otool -tvj`,
    // or read from the host's shared cache and `/usr/lib/dyld` (static `excl-ctx_all.txt` names
    // each site). None is hand-encoded except the two CONSTRAINED UNPREDICTABLE encodings, which no
    // assembler emits.

    /// `ldxr w10, [x9]; cbnz w10, +8; stxr wzr, w0, [x9]`: the census pair (static
    /// `excl-wzr_hits2.json`), and what it becomes.
    const GETPID: [u32; 3] = [0x885f_7d2a, 0x3500_004a, 0x881f_7d20];
    const GETPID_PLAIN: [u32; 3] = [0xb940_012a, 0x3500_004a, 0xb900_0120];
    const NOP: u32 = 0xd503_201f;
    const RET: u32 = 0xd65f_03c0;

    /// `words` rewritten, and the count.
    fn rw(words: &[u32]) -> (usize, Vec<u32>) {
        let mut w = words.to_vec();
        (rewrite_discarded_pairs(&mut w), w)
    }

    /// `ldxr w10, [x9]`, then `between`, then `stxr wzr, w0, [x9]`; the count.
    fn with_between(between: &[u32]) -> usize {
        let mut w = vec![GETPID[0]];
        w.extend_from_slice(between);
        w.push(GETPID[2]);
        rw(&w).0
    }

    #[test]
    fn the_census_getpid_pairs_become_a_plain_load_and_store() {
        // Libsyscall's __getpid from its svc's return address, as the host stages it: the cache's
        // dyld at 0x180118f48 (on-disk /usr/lib/dyld +0x4f48 is the same twelve words) and
        // libsystem_kernel at 0x1804ae198. They differ only in the `bl _cerror_nocancel`.
        // b.lo; pacibsp; stp; mov x29, sp; bl; mov sp, x29; ldp; retab; ldxr; cbnz; stxr; ret.
        for bl in [0x97ff_f794u32, 0x9400_0113] {
            let site = [0x5400_0103, 0xd503_237f, 0xa9bf_7bfd, 0x9100_03fd, bl, 0x9100_03bf,
                        0xa8c1_7bfd, 0xd65f_0fff, GETPID[0], GETPID[1], GETPID[2], RET];
            let (n, w) = rw(&site);
            assert_eq!(n, 1, "{bl:#x}");
            let mut want = site;
            want[8..11].copy_from_slice(&GETPID_PLAIN);
            assert_eq!(w, want, "only the two halves change ({bl:#x})");
        }
    }

    #[test]
    fn the_mrspair_shape_with_a_system_register_read_between_is_rewritten() {
        // mrspair_dyn and mrspair_lib: ldxr w10, [x9]; mrs x11, cntvct_el0; cbnz w10, +8;
        // stxr wzr, w0, [x9].
        let (n, w) = rw(&[0x885f_7d2a, 0xd53b_e04b, 0x3500_004a, 0x881f_7d20]);
        assert_eq!((n, w), (1, vec![0xb940_012a, 0xd53b_e04b, 0x3500_004a, 0xb900_0120]));
    }

    #[test]
    fn every_size_and_ordering_becomes_the_plain_access_clang_assembles() {
        // (load-exclusive, store-exclusive wzr) -> (plain load, plain store), all on [x0] with w1/x1.
        let table: [([u32; 2], [u32; 2]); 8] = [
            ([0x085f_7c01, 0x081f_7c01], [0x3940_0001, 0x3900_0001]), // ldxrb/stxrb -> ldrb/strb
            ([0x485f_7c01, 0x481f_7c01], [0x7940_0001, 0x7900_0001]), // ldxrh/stxrh -> ldrh/strh
            ([0x885f_7c01, 0x881f_7c01], [0xb940_0001, 0xb900_0001]), // ldxr/stxr w -> ldr/str w
            ([0xc85f_7c01, 0xc81f_7c01], [0xf940_0001, 0xf900_0001]), // ldxr/stxr x -> ldr/str x
            ([0x085f_fc01, 0x081f_fc01], [0x08df_fc01, 0x089f_fc01]), // ldaxrb/stlxrb -> ldarb/stlrb
            ([0x485f_fc01, 0x481f_fc01], [0x48df_fc01, 0x489f_fc01]), // ldaxrh/stlxrh -> ldarh/stlrh
            ([0x885f_fc01, 0x881f_fc01], [0x88df_fc01, 0x889f_fc01]), // ldaxr/stlxr w -> ldar/stlr w
            ([0xc85f_fc01, 0xc81f_fc01], [0xc8df_fc01, 0xc89f_fc01]), // ldaxr/stlxr x -> ldar/stlr x
        ];
        for (excl, plain) in table {
            let (n, w) = rw(&excl);
            assert_eq!((n, w.as_slice()), (1, &plain[..]), "{excl:#010x?}");
            assert!(w.iter().all(|&x| decode_excl(x).is_none()), "the plain forms touch no monitor: {w:#010x?}");
        }
        // Each half keeps its own ordering: ldaxr w10 + stxr wzr -> ldar + str; ldxr + stlxr wzr ->
        // ldr + stlr.
        assert_eq!(rw(&[0x885f_fd2a, 0x881f_7d20]), (1, vec![0x88df_fd2a, 0xb900_0120]));
        assert_eq!(rw(&[0x885f_7d2a, 0x881f_fd20]), (1, vec![0xb940_012a, 0x889f_fd20]));
    }

    #[test]
    fn an_sp_base_becomes_the_sp_form() {
        // ldxr x1, [sp]; stxr wzr, x1, [sp] -> ldr x1, [sp]; str x1, [sp].
        assert_eq!(rw(&[0xc85f_7fe1, 0xc81f_7fe1]), (1, vec![0xf940_03e1, 0xf900_03e1]));
        // ldaxrh w3, [sp]; stlxrh wzr, w4, [sp] -> ldarh w3, [sp]; stlrh w4, [sp].
        assert_eq!(rw(&[0x485f_ffe3, 0x481f_ffe4]), (1, vec![0x48df_ffe3, 0x489f_ffe4]));
    }

    #[test]
    fn a_conditional_branch_between_must_skip_exactly_the_store() {
        // Each form, one word before the store, targeting the word after it (+2): allowed.
        assert_eq!(with_between(&[0x3500_004a]), 1); // cbnz w10, +8 (getpid)
        assert_eq!(with_between(&[0x3400_004a]), 1); // cbz w10, +8
        assert_eq!(with_between(&[0x3600_004a]), 1); // tbz w10, #0, +8
        assert_eq!(with_between(&[0x7100_015f, 0x5400_0041]), 1); // cmp w10, #0; b.ne +8
        // Past the word after the store, or backward: refused.
        assert_eq!(with_between(&[0x3500_006a]), 0); // cbnz w10, +12
        assert_eq!(with_between(&[0x35ff_ffea]), 0); // cbnz w10, -4
        assert_eq!(with_between(&[0x3500_002a]), 0); // cbnz w10, +4: lands ON the store
    }

    #[test]
    fn the_window_is_sixteen_words() {
        let add = 0x1100_058c; // add w12, w12, #1: neutral, base x9 untouched
        assert_eq!(with_between(&[add; 15]), 1, "the load 16 words before the store");
        assert_eq!(with_between(&[add; 16]), 0, "the load 17 words before the store");
    }

    #[test]
    fn a_retry_loop_that_reads_its_status_is_left_alone() {
        // libsystem_kernel __vfork, both loops (static `excl-ctx_all.txt`, at 0x1804bbcf4 and
        // 0x1804bbd44): ldxr w10, [x9]; mov w11, #-1; subs; csel; stxr w13, w12, [x9]; cbnz w13, and
        // ldxr w10, [x9]; add w10, w10, #1; stxr w11, w10, [x9]; cbnz w11.
        let vfork1 = [0x885f_7d2a, 0x1280_000b, 0x7100_054a, 0x1a8a_516c, 0x880d_7d2c, 0x35ff_ff6d];
        let vfork2 = [0x885f_7d2a, 0x1100_054a, 0x880b_7d2a, 0x35ff_ffab];
        // mrspair_dyn's retry-loop control: ldxr w10, [x9]; mrs x11, cntvct_el0; add w10, w10, #1;
        // stxr w12, w10, [x9].
        let retry = [0x885f_7d2a, 0xd53b_e04b, 0x1100_054a, 0x880c_7d2a];
        for w in [&vfork1[..], &vfork2[..], &retry[..]] {
            assert_eq!(rw(w), (0, w.to_vec()), "{w:#010x?}");
        }
    }

    #[test]
    fn the_osatomic_misalign_loops_and_a_cas_are_left_alone() {
        // libsystem_platform _OSAtomicEnqueue_misalign at 0x1804febf8: ldxr x9, [x0]; cmp x8, x9;
        // mov x8, x9; b.ne; stlxr w10, x1, [x0]; mov x8, x9; cbnz w10; ret.
        let enqueue = [0xc85f_7c09, 0xeb09_011f, 0xaa09_03e8, 0x54ff_ff81, 0xc80a_fc01, 0xaa09_03e8, 0x35ff_ff2a, RET];
        // _OSAtomicDequeue_misalign at 0x1804fed18: mov x8, x0; ldaxr x0, [x8]; cbz x0; ldr x9,
        // [x0, x1]; stxr w10, x9, [x8]; cbnz w10.
        let dequeue = [0xaa00_03e8, 0xc85f_fd00, 0xb400_00a0, 0xf861_6809, 0xc80a_7d09, 0x35ff_ff8a];
        // llsc.s shape (c), a CAS: ldaxr x1, [x0]; cmp x1, x3; b.ne c_out; stlxr w2, x4, [x0].
        let cas = [0xc85f_fc01, 0xeb03_003f, 0x5400_0061, 0xc802_fc04];
        for w in [&enqueue[..], &dequeue[..], &cas[..]] {
            assert_eq!(rw(w), (0, w.to_vec()), "{w:#010x?}");
        }
    }

    #[test]
    fn a_stray_discarded_status_store_with_no_load_is_left_alone() {
        // Data in a text section that decodes as a discarded-status store (the cache holds 37 such
        // words, plan-time probe): `stlxrb wzr, w0, [x0]` between `udf #0`s.
        assert_eq!(rw(&[0, 0, 0x081f_fc00, 0, 0]).0, 0);
        assert_eq!(rw(&[NOP, NOP, NOP, GETPID[2], RET]).0, 0);
    }

    #[test]
    fn a_barrier_a_clrex_a_hint_a_branch_or_a_memory_access_between_the_halves_refuses() {
        for between in [0xd503_3bbf, // dmb ish
                        0xd503_3f5f, // clrex
                        0xd503_205f, // wfe
                        NOP,          // nop: every hint refuses, not only the ones that wait
                        0x1400_0001, // b +4: not on the fall-through path
                        0xb940_012b, // ldr w11, [x9]
                        0xb900_010b] // str w11, [x8]
        {
            assert_eq!(with_between(&[between]), 0, "{between:#010x}");
        }
    }

    #[test]
    fn a_write_to_the_base_between_the_halves_refuses() {
        assert_eq!(with_between(&[0x9100_2129]), 0); // add x9, x9, #8
        assert_eq!(with_between(&[0xd53b_e049]), 0); // mrs x9, cntvct_el0
        // An SP base: any Rd of 31 refuses (add sp, sp, #16), as excl::regs_written counts it.
        assert_eq!(rw(&[0xc85f_7fe1, 0x9100_43ff, 0xc81f_7fe1]).0, 0);
    }

    #[test]
    fn a_size_or_base_mismatch_or_a_base_the_load_overwrites_refuses() {
        assert_eq!(rw(&[0xc85f_7d2a, GETPID[2]]).0, 0, "ldxr x10, [x9] with a word store");
        assert_eq!(rw(&[0x885f_7d0a, GETPID[2]]).0, 0, "ldxr w10, [x8] with a store to [x9]");
        assert_eq!(rw(&[0x885f_7d29, GETPID[2]]).0, 0, "ldxr w9, [x9] overwrites its own base");
    }

    #[test]
    fn pair_instructions_and_unpredictable_encodings_are_left_alone() {
        assert_eq!(rw(&[0xc87f_0801, 0xc83f_0801]).0, 0, "ldxp x1, x2, [x0]; stxp wzr, x1, x2, [x0] (H7)");
        // Hand-encoded, because clang refuses both: stxr wzr, wzr, [x9] (status is the source), and
        // ldxr w10, [x9] with Rs = 30 (a should-be-one field clear).
        assert_eq!(rw(&[GETPID[0], 0x881f_7d3f]).0, 0);
        assert_eq!(rw(&[0x885e_7d2a, GETPID[2]]).0, 0);
    }

    #[test]
    fn a_pair_cut_by_the_slice_end_is_not_matched() {
        assert_eq!(rw(&[NOP, GETPID[0], GETPID[1]]).0, 0, "the store is past the end");
        assert_eq!(rw(&[GETPID[2], RET]).0, 0, "the load is before the start");
    }

    #[test]
    fn a_pair_across_a_page_boundary_is_not_matched() {
        let mut w = vec![NOP; 2 * PAGE_WORDS];
        // Straddling: the load is page 0's last word, the cbnz and the store open page 1.
        w[PAGE_WORDS - 1] = GETPID[0];
        w[PAGE_WORDS] = GETPID[1];
        w[PAGE_WORDS + 1] = GETPID[2];
        // A whole pair inside page 1, which proves page 1 was scanned.
        w[PAGE_WORDS + 100..PAGE_WORDS + 103].copy_from_slice(&GETPID);
        assert_eq!(rewrite_code_pages(&mut w), 1);
        assert_eq!(w[PAGE_WORDS - 1..PAGE_WORDS + 2], GETPID, "the straddling pair is untouched");
        assert_eq!(w[PAGE_WORDS + 100..PAGE_WORDS + 103], GETPID_PLAIN);
    }

    #[test]
    fn a_rewritten_pair_is_not_rewritten_again() {
        let mut w = GETPID.to_vec();
        assert_eq!(rewrite_discarded_pairs(&mut w), 1);
        assert_eq!(rewrite_discarded_pairs(&mut w), 0);
        assert_eq!(w, GETPID_PLAIN);
    }

    /// The pairs `rewrite_code_pages` changed in one page, as `(load VA, the words before)`.
    fn changed(va: u64, before: &[u32], after: &[u32]) -> Vec<(u64, Vec<u32>)> {
        let diff: Vec<usize> = (0..before.len()).filter(|&k| before[k] != after[k]).collect();
        diff.chunks(2).map(|p| (va + 4 * p[0] as u64, before[p[0]..=p[p.len() - 1]].to_vec())).collect()
    }

    #[test]
    fn every_exec_page_of_the_shared_cache_rewrites_exactly_the_two_getpid_pairs() {
        // M49 §3e X3 rewrites each cache TEXT page as it is paged in, and a TEXT mapping holds more
        // than instructions (`__const`, `__cstring`, padding). So run the real classifier over every
        // byte of every exec mapping, page by page as `page_in_cache` stages them, and require
        // exactly the census (t0 M3). The plan-time probe: 2 pairs over 225,659 pages.
        let cache = crate::cache::CacheMeta::load(crate::cache::DEFAULT_CACHE_PATH).expect("load the host's shared cache");
        let mut hits = Vec::new();
        for (base, size) in cache.exec_mappings() {
            for va in (base..base + size).step_by(PAGE_WORDS * 4) {
                let mut words = [0u32; PAGE_WORDS];
                {
                    // SAFETY: a `[u32; 4096]` is 16384 initialised bytes, at least as aligned as
                    // `[u8; 16384]`; the cast borrow ends with this block, before `words` is read.
                    let bytes = unsafe { &mut *words.as_mut_ptr().cast::<[u8; PAGE_WORDS * 4]>() };
                    cache.stage_page(va, bytes).expect("an exec mapping's page routes");
                }
                let before = words;
                if rewrite_code_pages(&mut words) > 0 { hits.extend(changed(va, &before, &words)); }
            }
        }
        assert_eq!(hits.len(), 2, "the census is the cache's dyld and libsystem_kernel: {hits:#x?}");
        for (va, was) in &hits {
            assert_eq!(*was, GETPID.to_vec(), "every rewritten site is a getpid pair: {va:#x}");
        }
    }

    #[test]
    fn the_on_disk_dylds_code_rewrites_exactly_its_getpid_pair() {
        // `load_dynamic` stages /usr/lib/dyld's exec segments the same way (X3). The plan-time probe:
        // one pair, at vmaddr 0x4f68 (DYLD_BASE + 0x4f68 in the guest).
        let bytes = std::fs::read(retrace_guest::DYLD_PATH).unwrap();
        let dyld = retrace_guest::parse_macho(retrace_guest::slice_arm64e(&bytes));
        let mut hits = Vec::new();
        for s in dyld.segments.iter().filter(|s| s.exec) {
            let before: Vec<u32> = s.data.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
            let mut words = before.clone();
            rewrite_code_pages(&mut words);
            hits.extend(changed(s.vaddr, &before, &words));
        }
        assert_eq!(hits, vec![(0x4f68, GETPID.to_vec())]);
    }
}
