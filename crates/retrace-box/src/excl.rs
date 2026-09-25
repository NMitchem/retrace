//! M42: the shadow of the PE's local exclusive monitor, and the pure checks around it (spec
//! `docs/superpowers/specs/2026-09-24-retrace-m42-llsc-design.md` §3). Everything here is pure:
//! `Box_` reads the vCPU and guest memory and passes the values in, so that every fail-loud branch
//! of the store-exclusive emulator has a unit test with no VM.
use retrace_arch::{decode_excl, is_fallthrough_barrier, ExclInsn};

/// TBI: `TCR_EL1` sets TBI0, so a data VA may carry a tag in [63:56], and `va_to_ipa` does not
/// strip it. Every address the shadow holds or compares is stripped with this mask first.
pub const TAG_MASK: u64 = 0x00FF_FFFF_FFFF_FFFF;

/// How a shadow came to be set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetBy {
    /// `step()` retired the load-exclusive (the step exit's ISS.EX, t0 M8).
    Stepped,
    /// A native breakpoint or watchpoint stop, by spec §3d's backward scan.
    Inferred,
}

/// The shadow: what the hardware monitor would hold, had retrace's own exits not cleared it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Excl {
    /// The marked VA, tag stripped.
    pub va: u64,
    /// Bytes per element: 1, 2, 4 or 8.
    pub size: u8,
    pub pair: bool,
    /// The bytes the load returned: `access_len(size, pair)` of them.
    pub loaded: Vec<u8>,
    pub by: SetBy,
}

/// Bytes one exclusive access moves.
pub fn access_len(size: u8, pair: bool) -> usize { size as usize * if pair { 2 } else { 1 } }

/// Stage-1 AP[2:1] (descriptor bits 7:6) == 0b01 is the only encoding that lets EL0 write
/// (`ATTR_DATA`). `ATTR_CODE` (0b11), `ATTR_TRAMP` (0b10) and `ATTR_NONE` (0b00) do not.
pub fn el0_writable(leaf: u64) -> bool { (leaf >> 6) & 3 == 1 }

/// What an emulated store-exclusive writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StxPlan {
    /// The tag-stripped VA.
    pub va: u64,
    pub bytes: Vec<u8>,
    /// The status register to zero, or None for WZR.
    pub status: Option<u32>,
}

/// Spec §3b step 1: validate a store-exclusive against the shadow.
///
/// The caller reads every input:
/// - `base` is Rn's value, from SP_EL0 when Rn is 31;
/// - `rt_val` and `rt2_val` are the data registers, 0 for XZR;
/// - `target` is the bytes now at the VA, None if unmapped;
/// - `writable` says whether the stage-1 leaf grants EL0 write.
///
/// Each refusal names its check, and the caller panics with it.
pub fn plan_stx(ex: &Excl, st: ExclInsn, base: u64, rt_val: u64, rt2_val: u64,
                target: Option<&[u8]>, writable: bool) -> Result<StxPlan, String> {
    let ExclInsn::Store { size, pair, rs, rt, rt2, rn } = st else {
        return Err(format!("not a store-exclusive: {st:?}"));
    };
    if rs == rt || (pair && rs == rt2) || (rs == rn && rn != 31) {
        return Err(format!("status register w{rs} aliases a data or base register (CONSTRAINED UNPREDICTABLE)"));
    }
    let va = base & TAG_MASK;
    if va != ex.va || size != ex.size || pair != ex.pair {
        return Err(format!("the store ({va:#x}, {size} B, pair={pair}) does not match the load ({:#x}, {} B, pair={})",
            ex.va, ex.size, ex.pair));
    }
    let len = access_len(size, pair);
    if !va.is_multiple_of(len as u64) { return Err(format!("{va:#x} is not {len}-byte aligned")); }
    let Some(target) = target else { return Err(format!("{va:#x} is unmapped")) };
    if target != ex.loaded.as_slice() {
        return Err(format!("the bytes at {va:#x} changed since the load ({:02x?} -> {target:02x?})", ex.loaded));
    }
    if !writable { return Err(format!("{va:#x} is not EL0-writable")); }
    let s = size as usize;
    let mut bytes = rt_val.to_le_bytes()[..s].to_vec();
    if pair { bytes.extend_from_slice(&rt2_val.to_le_bytes()[..s]); }
    Ok(StxPlan { va, bytes, status: (rs != 31).then_some(rs) })
}

/// Spec §3a: a base register that is also a destination was overwritten by the load, so the marked
/// address is gone. SP (31) never is: 31 in a destination is XZR.
pub fn base_aliases_dest(ld: ExclInsn) -> bool {
    let ExclInsn::Load { pair, rt, rt2, rn, .. } = ld else { return false };
    rn != 31 && (rn == rt || (pair && rn == rt2))
}

/// Spec §3d's backward scan.
///
/// `words[i]` is the instruction at `P - 4 * (i + 1)`, nearest first. The caller bounds the slice
/// at 16 words and never goes past the start of P's page.
///
/// Returns the nearest load-exclusive and its index. Returns None if a store-exclusive, a `clrex` or
/// a fall-through barrier comes first: past any of those, the monitor cannot still hold that load's
/// mark on the path that falls through to P.
pub fn scan_back(words: &[u32]) -> Option<(usize, ExclInsn)> {
    for (i, &w) in words.iter().enumerate() {
        match decode_excl(w) {
            Some(ld @ ExclInsn::Load { .. }) => return Some((i, ld)),
            Some(_) => return None,
            None if is_fallthrough_barrier(w) => return None,
            None => {}
        }
    }
    None
}

/// Spec §3d condition 5 (amended in execution, Ruling T5-a): the registers the instructions strictly
/// between the load and the stop write, as a bitmask (bit 31 = SP or XZR), or None if any of them
/// has register effects this does not know. Then nothing is inferred.
///
/// Known effects:
/// - A data-processing instruction, immediate (op0 `100x`) or register (op0 `x101`), writes at
///   most its Rd, bits 4:0. The field is counted as a write even where it is not a register
///   (`ccmp`'s nzcv, `rmif`, `setf`). For the base check (condition 5) that is conservative: a
///   phantom write can only refuse. For condition 3 it is not: `infer` skips a written
///   destination, so a phantom write drops that register's comparison.
/// - A conditional branch (`B.cond`/`BC.cond`, `CBZ`/`CBNZ`, `TBZ`/`TBNZ`) writes no register.
///
/// Everything else is unknown: a load or store (which may write back its base), a system
/// instruction such as `mrs`, SIMD and FP.
pub fn regs_written(words: &[u32]) -> Option<u32> {
    words.iter().try_fold(0u32, |acc, &w| {
        if (w >> 26) & 7 == 0b100 || (w >> 25) & 7 == 0b101 {
            Some(acc | (1 << (w & 0x1f)))
        } else if w & 0xFF00_0000 == 0x5400_0000
            || w & 0x7E00_0000 == 0x3400_0000
            || w & 0x7E00_0000 == 0x3600_0000 {
            Some(acc)
        } else {
            None
        }
    })
}

/// Spec §3d condition 3 (amended in execution, Ruling T5-a): the load's destination registers still
/// hold what is at its VA. The caller passes 31 for a destination the sequence itself rewrites
/// (condition 5's mask), so only the untouched ones are compared. A jump into the middle of a pair
/// almost always breaks this for those, and then nothing is inferred. A destination of 31 (XZR, or
/// rewritten) has nothing to compare.
pub fn dests_match(ld: ExclInsn, rt_val: u64, rt2_val: u64, bytes: &[u8]) -> bool {
    let ExclInsn::Load { size, pair, rt, rt2, .. } = ld else { return false };
    let s = size as usize;
    (rt == 31 || rt_val.to_le_bytes()[..s] == bytes[..s])
        && (!pair || rt2 == 31 || rt2_val.to_le_bytes()[..s] == bytes[s..2 * s])
}

/// Spec §3d, as amended (Rulings T5-a and T5-b): the shadow that a breakpoint or watchpoint stop
/// taken NATIVELY at `p` infers, or None. This is M42's one heuristic. Every condition that fails
/// infers nothing, which is the pre-M42 behaviour, never a wrong emulation.
///
/// - `words[j]` is the instruction at `p - 4 * (j + 1)`, as `scan_back` takes them. The caller
///   bounds the slice at 16 words and never goes past the start of `p`'s page.
/// - `entry_pc` is the pc of the last guest entry.
/// - `data(r)` reads Xr as a data register (31 = XZR), and `base(r)` as a base register (31 = SP).
/// - `read(va, len)` returns the bytes at a tag-stripped VA, or None if it is unmapped.
pub fn infer(words: &[u32], p: u64, entry_pc: u64, data: impl Fn(u32) -> u64,
             base: impl Fn(u32) -> u64, read: impl Fn(u64, usize) -> Option<Vec<u8>>) -> Option<Excl> {
    let (i, ld) = scan_back(words)?;
    let l = p - 4 * (i as u64 + 1);
    // 1. The last entry came after the load, and that entry's ERET cleared the monitor.
    if entry_pc > l && entry_pc <= p { return None; }
    // 2. A base overwritten by the load no longer names the marked address.
    if base_aliases_dest(ld) { return None; }
    let ExclInsn::Load { size, pair, rt, rt2, rn } = ld else { unreachable!("scan_back returns a load") };
    // 5 (amended, Ruling T5-a). Every instruction in (L, P) has known register effects, and none
    // writes the base: a rewritten base no longer names the marked address. Bit 31 is SP or XZR, so
    // for an SP base any Rd of 31 refuses.
    let written = regs_written(&words[..i])?;
    if written & (1 << rn) != 0 { return None; }
    let va = base(rn) & TAG_MASK;
    // 4. The target maps.
    let bytes = read(va, access_len(size, pair))?;
    // 3 (amended, Ruling T5-a). Each destination that nothing in (L, P) writes still holds what is
    // there. One the sequence rewrites (an in-place retry loop's `add x1, x1, #1`) cannot be checked
    // this way, so it goes to dests_match as 31, which compares nothing.
    let untouched = |r: u32| if written & (1 << r) != 0 { 31 } else { r };
    let checked = ExclInsn::Load { size, pair, rt: untouched(rt), rt2: untouched(rt2), rn };
    if !dests_match(checked, data(rt), data(rt2), &bytes) { return None; }
    Some(Excl { va, size, pair, loaded: bytes, by: SetBy::Inferred })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shadow(va: u64, size: u8, pair: bool, loaded: &[u8]) -> Excl {
        Excl { va, size, pair, loaded: loaded.to_vec(), by: SetBy::Stepped }
    }
    /// `stxr wzr, w0, [x9]`: (a)'s store, and dyld getpid's.
    const A_STX: ExclInsn = ExclInsn::Store { size: 4, pair: false, rs: 31, rt: 0, rt2: 31, rn: 9 };
    /// `stlxr w2, x1, [x0]`: (b)'s.
    const B_STX: ExclInsn = ExclInsn::Store { size: 8, pair: false, rs: 2, rt: 1, rt2: 31, rn: 0 };
    /// `stxp w3, x4, x5, [x0]`: (d)'s.
    const D_STX: ExclInsn = ExclInsn::Store { size: 8, pair: true, rs: 3, rt: 4, rt2: 5, rn: 0 };

    #[test]
    fn a_matching_store_plans_its_bytes_and_its_status() {
        let ex = shadow(0x1_0000_4000, 4, false, &[0; 4]);
        assert_eq!(plan_stx(&ex, A_STX, 0x1_0000_4000, 0x4242, 0, Some(&[0; 4]), true),
            Ok(StxPlan { va: 0x1_0000_4000, bytes: vec![0x42, 0x42, 0, 0], status: None }));
        let two = 2u64.to_le_bytes();
        let ex = shadow(0x1_0000_4040, 8, false, &two);
        assert_eq!(plan_stx(&ex, B_STX, 0x1_0000_4040, 3, 0, Some(&two), true),
            Ok(StxPlan { va: 0x1_0000_4040, bytes: 3u64.to_le_bytes().to_vec(), status: Some(2) }));
    }

    #[test]
    fn a_pair_writes_both_elements_in_order() {
        let loaded: Vec<u8> = [1u64.to_le_bytes(), 2u64.to_le_bytes()].concat();
        let ex = shadow(0x1_0000_40c0, 8, true, &loaded);
        let p = plan_stx(&ex, D_STX, 0x1_0000_40c0, 11, 22, Some(&loaded), true).unwrap();
        assert_eq!(p.bytes, [11u64.to_le_bytes(), 22u64.to_le_bytes()].concat());
        assert_eq!(p.status, Some(3));
    }

    #[test]
    fn a_tagged_base_is_stripped_before_it_is_compared() {
        let ex = shadow(0x1_0000_4000, 4, false, &[0; 4]);
        let p = plan_stx(&ex, A_STX, 0x5a00_0001_0000_4000, 7, 0, Some(&[0; 4]), true).unwrap();
        assert_eq!(p.va, 0x1_0000_4000);
    }

    #[test]
    fn a_store_that_does_not_match_the_load_is_refused() {
        let ex = shadow(0x1_0000_4000, 4, false, &[0; 4]);
        let other_va = plan_stx(&ex, A_STX, 0x1_0000_4004, 0, 0, Some(&[0; 4]), true).unwrap_err();
        assert!(other_va.contains("does not match the load"), "{other_va}");
        let wider = ExclInsn::Store { size: 8, pair: false, rs: 31, rt: 0, rt2: 31, rn: 9 };
        assert!(plan_stx(&ex, wider, 0x1_0000_4000, 0, 0, Some(&[0; 8]), true).unwrap_err().contains("does not match"));
        let paired = ExclInsn::Store { size: 4, pair: true, rs: 31, rt: 0, rt2: 1, rn: 9 };
        assert!(plan_stx(&ex, paired, 0x1_0000_4000, 0, 0, Some(&[0; 8]), true).unwrap_err().contains("does not match"));
    }

    #[test]
    fn a_misaligned_address_is_refused() {
        let ex = shadow(0x1_0000_4002, 4, false, &[0; 4]);
        assert!(plan_stx(&ex, A_STX, 0x1_0000_4002, 0, 0, Some(&[0; 4]), true).unwrap_err().contains("aligned"));
    }

    #[test]
    fn every_aliasing_status_register_is_refused_and_an_sp_base_is_not_wzr() {
        let ex = shadow(0x1_0000_4000, 8, false, &[0; 8]);
        let s_is_t = ExclInsn::Store { size: 8, pair: false, rs: 1, rt: 1, rt2: 31, rn: 0 };
        let s_is_n = ExclInsn::Store { size: 8, pair: false, rs: 0, rt: 1, rt2: 31, rn: 0 };
        let zr_zr = ExclInsn::Store { size: 8, pair: false, rs: 31, rt: 31, rt2: 31, rn: 0 };
        for st in [s_is_t, s_is_n, zr_zr] {
            let e = plan_stx(&ex, st, 0x1_0000_4000, 0, 0, Some(&[0; 8]), true).unwrap_err();
            assert!(e.contains("aliases"), "{st:?}: {e}");
        }
        let exp = shadow(0x1_0000_40c0, 8, true, &[0; 16]);
        let s_is_t2 = ExclInsn::Store { size: 8, pair: true, rs: 5, rt: 4, rt2: 5, rn: 0 };
        assert!(plan_stx(&exp, s_is_t2, 0x1_0000_40c0, 0, 0, Some(&[0; 16]), true).unwrap_err().contains("aliases"));
        // A WZR status with an SP base names two different registers: allowed.
        let sp = ExclInsn::Store { size: 8, pair: false, rs: 31, rt: 1, rt2: 31, rn: 31 };
        assert!(plan_stx(&ex, sp, 0x1_0000_4000, 9, 0, Some(&[0; 8]), true).is_ok());
    }

    #[test]
    fn drifted_bytes_an_unmapped_target_and_a_read_only_page_are_refused() {
        let ex = shadow(0x1_0000_4000, 4, false, &[0; 4]);
        assert!(plan_stx(&ex, A_STX, 0x1_0000_4000, 0, 0, Some(&[1, 0, 0, 0]), true).unwrap_err().contains("changed since the load"));
        assert!(plan_stx(&ex, A_STX, 0x1_0000_4000, 0, 0, None, true).unwrap_err().contains("unmapped"));
        assert!(plan_stx(&ex, A_STX, 0x1_0000_4000, 0, 0, Some(&[0; 4]), false).unwrap_err().contains("EL0-writable"));
    }

    #[test]
    fn only_the_data_attribute_is_el0_writable() {
        assert!(el0_writable(crate::ATTR_DATA));
        assert!(!el0_writable(crate::ATTR_CODE));
        assert!(!el0_writable(crate::ATTR_TRAMP));
        assert!(!el0_writable(crate::ATTR_NONE));
    }

    #[test]
    fn a_base_that_is_also_a_destination_is_flagged_and_sp_never_is() {
        assert!(base_aliases_dest(ExclInsn::Load { size: 8, pair: false, rt: 0, rt2: 31, rn: 0 }));
        assert!(base_aliases_dest(ExclInsn::Load { size: 8, pair: true, rt: 1, rt2: 0, rn: 0 }));
        assert!(!base_aliases_dest(ExclInsn::Load { size: 8, pair: false, rt: 31, rt2: 31, rn: 31 }));
        assert!(!base_aliases_dest(ExclInsn::Load { size: 4, pair: false, rt: 10, rt2: 31, rn: 9 }));
    }

    #[test]
    fn the_scan_finds_the_nearest_load_across_neutral_instructions() {
        // dyld's getpid, back from its stxr: cbnz, then ldxr.
        assert_eq!(scan_back(&[0x3500_004a, 0x885f_7d2a]).map(|(i, _)| i), Some(1));
        // (b), back from its stlxr: add x1, x1, #1, then ldaxr.
        assert_eq!(scan_back(&[0x9100_0421, 0xc85f_fc01]).map(|(i, _)| i), Some(1));
        // (f), back from its stxr: the mrs is not a barrier. The ENTRY check rejects this one, not
        // the scan.
        assert!(scan_back(&[0xd53b_e047, 0x885f_7c06]).is_some());
    }

    #[test]
    fn the_scan_stops_at_a_store_exclusive_a_clrex_or_a_barrier() {
        assert_eq!(scan_back(&[0x881f_7d20, 0x885f_7d2a]), None); // an stxr consumed the older load
        assert_eq!(scan_back(&[0xd503_3f5f, 0x885f_7d2a]), None); // clrex
        assert_eq!(scan_back(&[0xd400_1001, 0x885f_7d2a]), None); // svc
        assert_eq!(scan_back(&[0x1400_0002, 0x885f_7d2a]), None); // b
        assert_eq!(scan_back(&[0xd65f_03c0, 0x885f_7d2a]), None); // ret
        assert_eq!(scan_back(&[0xd503_201f; 16]), None);          // sixteen nops, no load
    }

    #[test]
    fn the_destination_check_compares_each_element_and_skips_xzr() {
        let ldxr = ExclInsn::Load { size: 4, pair: false, rt: 10, rt2: 31, rn: 9 };
        assert!(dests_match(ldxr, 0x4242, 0, &[0x42, 0x42, 0, 0]));
        assert!(!dests_match(ldxr, 0x4343, 0, &[0x42, 0x42, 0, 0]));
        let ldxp = ExclInsn::Load { size: 8, pair: true, rt: 1, rt2: 2, rn: 0 };
        let b: Vec<u8> = [1u64.to_le_bytes(), 2u64.to_le_bytes()].concat();
        assert!(dests_match(ldxp, 1, 2, &b));
        assert!(!dests_match(ldxp, 1, 3, &b));
        let xzr = ExclInsn::Load { size: 8, pair: false, rt: 31, rt2: 31, rn: 0 };
        assert!(dests_match(xzr, 99, 0, &[7; 8]), "ldxr xzr loads nothing to compare");
    }

    // Condition 5's words below were assembled with `clang -arch arm64 -c` and read back with
    // `otool -tvj`, never hand-encoded.

    #[test]
    fn a_data_processing_instruction_writes_its_rd() {
        assert_eq!(regs_written(&[0x9100_0421]), Some(1 << 1)); // (b) and (i): add x1, x1, #1
        // (d): add x4, x1, #10; add x5, x2, #20. The loaded x1 and x2 stay checkable.
        assert_eq!(regs_written(&[0x9100_5045, 0x9100_2824]), Some(1 << 4 | 1 << 5));
        // Census #3, libsystem_kernel __vfork, nearest first: csel w12, w11, w10, pl;
        // subs w10, w10, #1; mov w11, #-1.
        assert_eq!(regs_written(&[0x1a8a_516c, 0x7100_054a, 0x1280_000b]), Some(1 << 10 | 1 << 11 | 1 << 12));
        // cmp x1, x3 is subs xzr, x1, x3: an Rd of 31 is a write of SP or XZR.
        assert_eq!(regs_written(&[0xeb03_003f]), Some(1 << 31));
        assert_eq!(regs_written(&[]), Some(0), "an adjacent pair has nothing between its halves");
    }

    #[test]
    fn a_conditional_branch_writes_nothing() {
        assert_eq!(regs_written(&[0x3500_004a]), Some(0)); // dyld getpid's cbnz w10
        assert_eq!(regs_written(&[0x5400_0141]), Some(0)); // b.ne
        assert_eq!(regs_written(&[0x3618_0122]), Some(0)); // tbz w2, #3
    }

    #[test]
    fn an_instruction_with_unknown_register_effects_infers_nothing() {
        assert_eq!(regs_written(&[0xb940_012b]), None); // ldr w11, [x9]
        assert_eq!(regs_written(&[0xf800_8521]), None); // str x1, [x9], #8: writes back its base
        assert_eq!(regs_written(&[0xd53b_e047]), None); // mrs x7, cntvct_el0
        assert_eq!(regs_written(&[0x9e67_0020]), None); // fmov d0, x1
        // One unknown poisons a sequence whose other instructions are all known.
        assert_eq!(regs_written(&[0x9100_0421, 0xb940_012b, 0x3500_004a]), None);
    }

    #[test]
    fn an_sp_base_is_refused_by_any_rd_of_31() {
        // infer_excl refuses when `written & (1 << rn) != 0`; for an SP base, rn is 31.
        let written = regs_written(&[0x9100_43ff]).unwrap(); // add sp, sp, #16
        assert_ne!(written & (1 << 31), 0);
        // cmp's XZR destination refuses an SP base too: 31 counts as a write of both.
        assert_ne!(regs_written(&[0xeb03_003f]).unwrap() & (1 << 31), 0);
    }

    // ---- `infer`, spec §3d per condition (Ruling T5-b) -----------------------------------------
    //
    // The words are the llsc fixture's own, read with `otool -tvj` from the built binary, at their
    // real addresses (nearest first, as `scan_back` takes them). The synthetic words for conditions
    // 2 and 5 were assembled with `clang -arch arm64 -c` and read back the same way. Each condition
    // has a case that passes every OTHER condition, so deleting that one condition's line in `infer`
    // turns its None into a Some (the ledgered deletion table).

    /// A fake PE for `infer`: x0..x30, SP, and one mapped cell.
    struct Pe { x: [u64; 31], sp: u64, cell: u64, mem: Vec<u8> }

    impl Pe {
        fn new(cell: u64, mem: &[u8]) -> Pe { Pe { x: [0; 31], sp: 0, cell, mem: mem.to_vec() } }
        fn x(mut self, r: usize, v: u64) -> Pe { self.x[r] = v; self }
        fn infer(&self, words: &[u32], p: u64, entry_pc: u64) -> Option<Excl> {
            infer(words, p, entry_pc,
                  |r| if r == 31 { 0 } else { self.x[r as usize] },
                  |r| if r == 31 { self.sp } else { self.x[r as usize] },
                  |va, len| (va == self.cell && len <= self.mem.len()).then(|| self.mem[..len].to_vec()))
        }
    }

    fn inferred(va: u64, size: u8, pair: bool, loaded: &[u8]) -> Option<Excl> {
        Some(Excl { va, size, pair, loaded: loaded.to_vec(), by: SetBy::Inferred })
    }

    const CELLA: u64 = 0x1_0000_4000;
    const CTR: u64 = 0x1_0000_4040;
    const CAS: u64 = 0x1_0000_4080;
    const PAIR: u64 = 0x1_0000_40c0;
    const CELLF: u64 = 0x1_0000_4180;
    /// (a) at a_stx 0x1_0000_0394: cbnz w10; ldxr w10, [x9] (a_ldx); mov w0, #0x4242; add; adrp.
    const A_P: u64 = 0x1_0000_0394;
    const A_L: u64 = 0x1_0000_038c;
    const A_WORDS: [u32; 5] = [0x3500_004a, 0x885f_7d2a, 0x5288_4840, 0x9100_0129, 0x9000_0029];
    /// _start, where a native run of window 1 enters.
    const A_ENTRY: u64 = 0x1_0000_0380;
    /// (b) at b_stx 0x1_0000_03e8: add x1, x1, #1; ldaxr x1, [x0] (b_ldx); add x20, x20, #1; ...
    const B_P: u64 = 0x1_0000_03e8;
    const B_WORDS: [u32; 4] = [0x9100_0421, 0xc85f_fc01, 0x9100_0694, 0xd280_0014];
    const B_ENTRY: u64 = 0x1_0000_03cc; // b_start (the probe measured this entry)
    /// (c) at c_stx 0x1_0000_0440: b.ne c_out; cmp x1, x3; ldaxr x1, [x0] (c_ldx); ...
    const C_P: u64 = 0x1_0000_0440;
    const C_WORDS: [u32; 4] = [0x5400_0061, 0xeb03_003f, 0xc85f_fc01, 0x9100_0694];
    const C_ENTRY: u64 = 0x1_0000_041c;
    /// (d) at d_stx 0x1_0000_0488: add x5, x2, #20; add x4, x1, #10; ldxp x1, x2, [x0] (d_ldx); ...
    const D_P: u64 = 0x1_0000_0488;
    const D_WORDS: [u32; 4] = [0x9100_5045, 0x9100_2824, 0xc87f_0801, 0x9100_0694];
    const D_ENTRY: u64 = 0x1_0000_046c;
    /// (f) at f_stx 0x1_0000_0500: mrs x7, cntvct_el0 (f_mrs); ldxr w6, [x0] (f_ldx); ...
    const F_P: u64 = 0x1_0000_0500;
    const F_WORDS: [u32; 4] = [0xd53b_e047, 0x885f_7c06, 0x5280_00e1, 0x9106_0000];

    fn pe_a() -> Pe { Pe::new(CELLA, &[0; 4]).x(9, CELLA).x(0, 0x4242) }
    fn pe_b() -> Pe { Pe::new(CTR, &[0; 8]).x(0, CTR).x(1, 1) } // x1 = the loaded 0, plus 1

    #[test]
    fn infer_positive_the_fixtures_stores_infer_their_shadow() {
        assert_eq!(pe_a().infer(&A_WORDS, A_P, A_ENTRY), inferred(CELLA, 4, false, &[0; 4]));
        assert_eq!(pe_b().infer(&B_WORDS, B_P, B_ENTRY), inferred(CTR, 8, false, &[0; 8]));
        let five = 5u64.to_le_bytes();
        let c = Pe::new(CAS, &five).x(0, CAS).x(1, 5).x(3, 5).x(4, 9);
        assert_eq!(c.infer(&C_WORDS, C_P, C_ENTRY), inferred(CAS, 8, false, &five));
        let pair: Vec<u8> = [1u64.to_le_bytes(), 2u64.to_le_bytes()].concat();
        let d = Pe::new(PAIR, &pair).x(0, PAIR).x(1, 1).x(2, 2).x(4, 11).x(5, 22);
        assert_eq!(d.infer(&D_WORDS, D_P, D_ENTRY), inferred(PAIR, 8, true, &pair));
    }

    #[test]
    fn infer_positive_a_tagged_base_is_stripped() {
        let a = pe_a().x(9, 0x5a00_0000_0000_0000 | CELLA);
        assert_eq!(a.infer(&A_WORDS, A_P, A_ENTRY), inferred(CELLA, 4, false, &[0; 4]));
    }

    #[test]
    fn infer_condition_1_an_entry_inside_l_to_p_infers_nothing() {
        assert_eq!(pe_a().infer(&A_WORDS, A_P, A_P), None, "entry at P itself");
        assert_eq!(pe_a().infer(&A_WORDS, A_P, A_L + 4), None, "entry just after the load");
        // An entry at or before L precedes the load: the monitor was marked after it.
        assert!(pe_a().infer(&A_WORDS, A_P, A_L).is_some(), "entry AT the load");
        assert!(pe_a().infer(&A_WORDS, A_P, A_ENTRY).is_some(), "entry before the load");
    }

    #[test]
    fn infer_condition_1_and_5_each_refuse_the_fixtures_reentry_shape() {
        let f = Pe::new(CELLF, &[0; 4]).x(0, CELLF).x(1, 7);
        // The emulated timebase read re-entered at f_stx: condition 1.
        assert_eq!(f.infer(&F_WORDS, F_P, F_P), None);
        // Even with an entry before the load, the mrs has unknown register effects: condition 5.
        assert_eq!(f.infer(&F_WORDS, F_P, 0x1_0000_04ec), None);
    }

    #[test]
    fn infer_condition_2_a_base_that_the_load_overwrote_infers_nothing() {
        // ldxr x9, [x9] at L = P - 4. The cell points at itself, so every OTHER condition holds: x9
        // names a mapped VA whose bytes equal x9.
        let e = Pe::new(CELLA, &CELLA.to_le_bytes()).x(9, CELLA);
        assert_eq!(e.infer(&[0xc85f_7d29], A_P, A_ENTRY), None);
    }

    #[test]
    fn infer_condition_3_an_untouched_destination_that_differs_infers_nothing() {
        // (a): nothing between the halves writes w10, and it no longer holds the cell's bytes.
        assert_eq!(pe_a().x(10, 0x4343).infer(&A_WORDS, A_P, A_ENTRY), None);
        // (d): the second element x2 is untouched and checked too.
        let pair: Vec<u8> = [1u64.to_le_bytes(), 2u64.to_le_bytes()].concat();
        let d = Pe::new(PAIR, &pair).x(0, PAIR).x(1, 1).x(2, 3);
        assert_eq!(d.infer(&D_WORDS, D_P, D_ENTRY), None);
        // (b): a destination the sequence rewrites differs from the cell and still infers (the
        // amendment): x1 = 1 against a cell of 0.
        assert!(pe_b().infer(&B_WORDS, B_P, B_ENTRY).is_some());
    }

    #[test]
    fn infer_condition_4_an_unmapped_target_infers_nothing() {
        // (b)'s x1 is rewritten, so nothing else would refuse: only the mapping does.
        let unmapped = Pe::new(0xdead_0000, &[0; 8]).x(0, CTR).x(1, 1);
        assert_eq!(unmapped.infer(&B_WORDS, B_P, B_ENTRY), None);
    }

    #[test]
    fn infer_condition_5_an_unknown_instruction_between_the_halves_infers_nothing() {
        // ldr w11, [x9] between ldxr w10, [x9] and the stop.
        let a = pe_a();
        assert_eq!(a.infer(&[0xb940_012b, 0x885f_7d2a], A_P, A_ENTRY), None);
    }

    #[test]
    fn infer_condition_5_an_instruction_that_writes_the_base_infers_nothing() {
        // add x9, x9, #8 between ldxr w10, [x9] and the stop. x9 as it is NOW maps, and w10 equals
        // it, so only the base check refuses.
        assert_eq!(pe_a().infer(&[0x9100_2129, 0x885f_7d2a], A_P, A_ENTRY), None);
    }

    #[test]
    fn infer_condition_5_an_sp_base_is_refused_by_any_rd_of_31() {
        let sp_base = |mut pe: Pe| { pe.sp = CELLA; pe };
        let pe = || sp_base(Pe::new(CELLA, &7u64.to_le_bytes()).x(1, 7));
        // ldxr x1, [sp], then add sp, sp, #16.
        assert_eq!(pe().infer(&[0x9100_43ff, 0xc85f_7fe1], A_P, A_ENTRY), None);
        // ldxr x1, [sp], then cmp x1, x3: XZR is register 31 too.
        assert_eq!(pe().infer(&[0xeb03_003f, 0xc85f_7fe1], A_P, A_ENTRY), None);
        // ldxr x1, [sp], then cbnz x1: nothing writes 31, so an SP base infers.
        assert_eq!(pe().infer(&[0xb500_0061, 0xc85f_7fe1], A_P, A_ENTRY), inferred(CELLA, 8, false, &7u64.to_le_bytes()));
    }
}
