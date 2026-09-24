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

/// Spec §3d condition 3: the load's destination registers still hold what is at its VA. A jump into
/// the middle of a pair, or a rewritten register, almost always breaks this, and then nothing is
/// inferred. A destination of 31 (XZR) loads nothing to compare.
pub fn dests_match(ld: ExclInsn, rt_val: u64, rt2_val: u64, bytes: &[u8]) -> bool {
    let ExclInsn::Load { size, pair, rt, rt2, .. } = ld else { return false };
    let s = size as usize;
    (rt == 31 || rt_val.to_le_bytes()[..s] == bytes[..s])
        && (!pair || rt2 == 31 || rt2_val.to_le_bytes()[..s] == bytes[s..2 * s])
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
}
