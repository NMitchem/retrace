//! M48 §3f (J1): the guest's `MAP_JIT` ranges and the stage-1 view stamped over them.
//!
//! Pure data. `Box_` owns one `JitSet`, stamps what [`JitSet::stamped_extents`] returns with the
//! attribute its [`View`] names, and carries it through every rebuild path (`BoxState`). It is rebuilt
//! from the guest's own `mmap`/`mprotect`/`munmap` landmarks and its own `msr`s on both sides, so
//! nothing here is recorded (R3).
//!
//! The view is per process, and it always equals the running thread's mode (§3f "Flips"). Native
//! gives each thread its own `S3_6_C15_C1_5` (§2c); under one vCPU and a cooperative scheduler only
//! the running thread's is ever consulted, so one view that follows it is native's per-thread view.

use crate::subtract_range;

/// `PROT_READ | PROT_WRITE | PROT_EXEC` (SDK `sys/mman.h`).
const PROT_RWX: u64 = 7;
// SDK `sys/mman.h`.
const MAP_SHARED: u64 = 0x1;
const MAP_FIXED: u64 = 0x10;
const MAP_ANON: u64 = 0x1000;

/// The stage-1 view over a `MAP_JIT` range's accessible pages. `Box_::restamp_jit` is the one place
/// a view becomes an attribute: `Rx` is `ATTR_CODE` and `Rw` is `ATTR_DATA`, so no page is ever
/// writable and executable at once, and the W^X invariant holds by construction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    /// Protected: read and execute. Every native thread starts here (§2c), and so does every guest
    /// thread (R1).
    #[default]
    Rx,
    /// Write-enabled: read and write, never execute.
    Rw,
}

impl View {
    /// The mode of a thread whose `S3_6_C15_C1_5` holds `sprr` (Ruling T6-e). `write_enable` is the
    /// commpage's `+0x110` word, or None for a guest with no SPRR commpage. Only that exact word
    /// write-enables: R1's initial 0 and the commpage's `+0x118` both protect.
    pub fn of_sprr(sprr: u64, write_enable: Option<u64>) -> View {
        if sprr != 0 && Some(sprr) == write_enable { View::Rw } else { View::Rx }
    }
}

/// Every `MAP_JIT` range the guest holds, and the view currently stamped over them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JitSet {
    /// Page-aligned `(start, len)`, sorted by start and disjoint: what the guest mapped `MAP_JIT`
    /// and has not unmapped. A partial `munmap` trims or splits a range (Ruling T6-d).
    ranges: Vec<(u64, u64)>,
    /// What stage 1 holds over every range minus its no-access extents.
    view: View,
}

impl JitSet {
    /// Is an `mmap` with these `prot` and `flags` a `MAP_JIT` one (`Ok(true)`), an ordinary one
    /// (`Ok(false)`), or a `MAP_JIT` shape this model refuses (`Err`, naming the value)? Rulings
    /// T6-a and T6-b. An ordinary RWX anonymous map keeps today's path untouched (§11a item 2).
    pub fn admit_mmap(prot: u64, flags: u64) -> Result<bool, String> {
        if flags & retrace_arch::MAP_JIT == 0 { return Ok(false); }
        if flags & (MAP_FIXED | MAP_SHARED) != 0 || flags & MAP_ANON == 0 {
            return Err(format!(
                "M48: MAP_JIT mmap flags {flags:#x}: MAP_JIT with MAP_FIXED, with MAP_SHARED or \
                 without MAP_ANON is unmodelled. xnu's mmap answers EINVAL (kern_mman.c:mmap), and \
                 no measured guest issues one (Ruling T6-b)"));
        }
        if prot != 0 && prot != PROT_RWX {
            return Err(format!(
                "M48: MAP_JIT mmap prot {prot:#x}: only PROT_NONE (V8's code-range reservation, \
                 t0 M5) and RWX (sprr.c's page) are measured on a MAP_JIT mapping (Ruling T6-a)"));
        }
        Ok(true)
    }

    /// Does an `mprotect(ipa, len, prot)` touch a `MAP_JIT` range (`Ok(true)`, and the caller
    /// restamps the view afterwards), miss every range (`Ok(false)`), or ask one for a protection
    /// this model refuses (`Err`)? Partial overlap is admitted: V8 commits a sub-range of its
    /// reservation (§11a item 5, walls.md §3).
    pub fn admit_mprotect(&self, ipa: u64, len: u64, prot: u64) -> Result<bool, String> {
        let Some(&(s, l)) = self.ranges.iter().find(|&&(s, l)| overlap(ipa, len, s, l)) else {
            return Ok(false);
        };
        if prot != 0 && prot != PROT_RWX {
            return Err(format!(
                "M48: MAP_JIT mprotect [{ipa:#x}, +{len:#x}) prot {prot:#x} over the range \
                 [{s:#x}, {:#x}): only PROT_NONE and RWX are measured on a MAP_JIT page \
                 (Ruling T6-a)", s + l));
        }
        Ok(true)
    }

    /// `Err` naming the range when `[addr, addr+len)` overlaps one (Ruling T6-c). `what` names the
    /// request ("a FIXED mapping", "a mach_vm_remap source", "a mach_vm_remap target").
    pub fn refuse_overlap(&self, addr: u64, len: u64, what: &str) -> Result<(), String> {
        match self.ranges.iter().find(|&&(s, l)| overlap(addr, len, s, l)) {
            None => Ok(()),
            Some(&(s, l)) => Err(format!(
                "M48: MAP_JIT range [{s:#x}, {:#x}) overlapped by {what} at [{addr:#x}, +{len:#x}): \
                 unmodelled, since the view would stay stamped over pages the new mapping owns \
                 (Ruling T6-c)", s + l)),
        }
    }

    /// Add a newly mapped range. `Box_::map_mmap_region` placed it, so it is page-aligned and
    /// overlaps nothing already mapped, ranges included.
    pub fn add(&mut self, start: u64, len: u64) {
        assert!(!self.ranges.iter().any(|&(s, l)| overlap(start, len, s, l)),
            "M48: a new MAP_JIT range [{start:#x}, +{len:#x}) overlaps a live one: {:x?}", self.ranges);
        let at = self.ranges.partition_point(|&(s, _)| s < start);
        self.ranges.insert(at, (start, len));
    }

    /// Remove page-aligned `[start, end)`: a range wholly inside goes, one it cuts is trimmed or
    /// split (Ruling T6-d). Returns the removed pieces, which the caller stamps back to `ATTR_DATA`.
    pub fn remove(&mut self, start: u64, end: u64) -> Vec<(u64, u64)> {
        let gone = self.ranges.iter()
            .filter(|&&(s, l)| overlap(start, end - start, s, l))
            .map(|&(s, l)| { let (a, b) = (s.max(start), (s + l).min(end)); (a, b - a) })
            .collect();
        subtract_range(&mut self.ranges, start, end - start);
        gone
    }

    /// The extents the view is stamped over: every range minus the no-access extents inside it
    /// (§11b item 4). V8 maps its whole code range `PROT_NONE` and commits a sub-range, so the
    /// uncommitted remainder, a head and a tail, keeps M13's `ATTR_NONE`; stamping it would make it
    /// accessible. `subtract_range` keeps the order, so the result is sorted like the ranges.
    pub fn stamped_extents(&self, noaccess: &[(u64, u64)]) -> Vec<(u64, u64)> {
        let mut out = self.ranges.clone();
        for &(s, l) in noaccess { subtract_range(&mut out, s, l); }
        out
    }

    pub fn ranges(&self) -> &[(u64, u64)] { &self.ranges }
    pub fn view(&self) -> View { self.view }
    pub fn set_view(&mut self, view: View) { self.view = view; }
}

/// `[a, a+alen)` and `[b, b+blen)` share a byte.
fn overlap(a: u64, alen: u64, b: u64, blen: u64) -> bool {
    a < b.saturating_add(blen) && b < a.saturating_add(alen)
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: u64 = 0x4_0000_0000;      // a range base, page-aligned
    const WE: u64 = 0x2010_0020_3030_0000; // the probe host's commpage +0x110 (sprr.out)
    const PR: u64 = 0x2010_0020_3010_0000; // its +0x118
    const V8_FLAGS: u64 = 0x41842;         // MAP_JIT|MAP_ANON|MAP_NORESERVE|MAP_PRIVATE|MAP_UNIX03 (P5)

    #[test]
    fn admit_mmap_takes_prot_none_and_rwx_map_jit_and_passes_the_rest_through() {
        assert_eq!(JitSet::admit_mmap(0, V8_FLAGS), Ok(true), "V8's reservation (P5)");
        assert_eq!(JitSet::admit_mmap(7, 0x1802), Ok(true), "sprr.c's MAP_PRIVATE|MAP_ANON|MAP_JIT RWX page");
        assert_eq!(JitSet::admit_mmap(7, 0x1002), Ok(false), "an RWX map without MAP_JIT keeps today's path");
        assert_eq!(JitSet::admit_mmap(3, 0x1002), Ok(false), "an ordinary data map");
    }

    #[test]
    fn a_map_jit_mmap_with_another_prot_is_refused_by_value() {
        for prot in [1u64, 3, 5] {
            let e = JitSet::admit_mmap(prot, 0x1802).unwrap_err();
            assert!(e.starts_with(&format!("M48: MAP_JIT mmap prot {prot:#x}:")), "{e}");
        }
    }

    #[test]
    fn a_fixed_shared_or_file_map_jit_mmap_is_refused_by_value() {
        for flags in [0x1812u64 /* FIXED */, 0x1801 /* SHARED */, 0x802 /* no MAP_ANON */] {
            let e = JitSet::admit_mmap(7, flags).unwrap_err();
            assert!(e.starts_with(&format!("M48: MAP_JIT mmap flags {flags:#x}:")), "{e}");
        }
    }

    /// Review Focus item 4 (pure half): a committed sub-range inside a larger PROT_NONE
    /// reservation, as V8 maps it, scaled down. V8's own offset is the first 256 KiB boundary past
    /// the base, so a head of `0x4000`-`0x34000` and a PROT_NONE tail follow in the six measured
    /// walks (t0 M5, Ruling T6-LAYOUT); this test's `+0x40000` head is a fixture shape. Only the
    /// committed part carries the view. A no-access extent elsewhere changes nothing, a head and a
    /// tail leave the middle, and an interior one splits.
    #[test]
    fn the_stamped_extents_are_the_ranges_minus_their_noaccess_extents() {
        let mut j = JitSet::default();
        j.add(B, 0x100_0000);
        assert!(j.stamped_extents(&[(B, 0x100_0000)]).is_empty(), "all PROT_NONE: nothing stamped");
        let noaccess = [(0x1_0000_0000, 0x4000), (B, 0x4_0000)];
        assert_eq!(j.stamped_extents(&noaccess), vec![(B + 0x4_0000, 0xfc_0000)],
            "the committed sub-range only, past a PROT_NONE head");
        let head_and_tail = [(B, 0x4_0000), (B + 0xfc_0000, 0x4_0000)];
        assert_eq!(j.stamped_extents(&head_and_tail), vec![(B + 0x4_0000, 0xf8_0000)],
            "the measured V8 shape: PROT_NONE head and tail, only the committed middle carries the view");
        let punched = [(B, 0x4_0000), (B + 0x10_0000, 0x8000)];
        assert_eq!(j.stamped_extents(&punched),
            vec![(B + 0x4_0000, 0xc_0000), (B + 0x10_8000, 0xef_8000)], "an interior guard splits it");
        assert_eq!(j.stamped_extents(&[]), vec![(B, 0x100_0000)], "no protection: the whole range");
    }

    #[test]
    fn admit_mprotect_admits_none_and_rwx_on_a_range_and_refuses_the_rest() {
        let mut j = JitSet::default();
        j.add(B, 0x10_0000);
        assert_eq!(j.admit_mprotect(B + 0x4_0000, 0xc_0000, 7), Ok(true), "a sub-range RWX commit, V8's shape");
        assert_eq!(j.admit_mprotect(B, 0x4000, 0), Ok(true), "a guard page inside the range");
        assert_eq!(j.admit_mprotect(B - 0x4000, 0x8000, 7), Ok(true), "a straddle still touches the range");
        assert_eq!(j.admit_mprotect(B - 0x4000, 0x4000, 1), Ok(false), "outside every range: not ours to judge");
        let e = j.admit_mprotect(B + 0x8000, 0x4000, 5).unwrap_err();
        assert!(e.starts_with(&format!("M48: MAP_JIT mprotect [{:#x}, +0x4000) prot 0x5 ", B + 0x8000)), "{e}");
    }

    #[test]
    fn a_partial_munmap_trims_and_splits_and_returns_what_it_removed() {
        let mut j = JitSet::default();
        j.add(B, 0x4_0000);
        assert_eq!(j.remove(B, B + 0x4000), vec![(B, 0x4000)], "a head trim");
        assert_eq!(j.ranges(), &[(B + 0x4000, 0x3_c000)]);
        assert_eq!(j.remove(B + 0x1_0000, B + 0x1_8000), vec![(B + 0x1_0000, 0x8000)], "an interior punch");
        assert_eq!(j.ranges(), &[(B + 0x4000, 0xc000), (B + 0x1_8000, 0x2_8000)], "split in two, still sorted");
        assert!(j.remove(0x1_0000_0000, 0x1_0000_4000).is_empty(), "a disjoint munmap removes nothing");
        assert_eq!(j.remove(B, B + 0x4_0000), vec![(B + 0x4000, 0xc000), (B + 0x1_8000, 0x2_8000)],
            "V8's whole-range munmap at exit takes both pieces");
        assert!(j.ranges().is_empty());
    }

    #[test]
    fn a_fixed_or_remap_overlap_is_refused_by_value_and_a_disjoint_one_is_not() {
        let mut j = JitSet::default();
        j.add(B, 0x10_0000);
        assert_eq!(j.refuse_overlap(B + 0x10_0000, 0x4000, "a FIXED mapping"), Ok(()), "adjacent above");
        assert_eq!(j.refuse_overlap(B - 0x4000, 0x4000, "a FIXED mapping"), Ok(()), "adjacent below");
        let e = j.refuse_overlap(B + 0xf_c000, 0x8000, "a FIXED mapping").unwrap_err();
        assert!(e.starts_with(&format!("M48: MAP_JIT range [{B:#x}, {:#x}) overlapped by a FIXED mapping at ",
            B + 0x10_0000)), "{e}");
    }

    #[test]
    fn the_mode_is_write_enabled_only_at_the_commpages_write_enable_word() {
        assert_eq!(JitSet::default().view(), View::Rx, "the set starts protected (R1)");
        assert_eq!(View::of_sprr(0, Some(WE)), View::Rx, "R1's initial 0 protects");
        assert_eq!(View::of_sprr(PR, Some(WE)), View::Rx, "+0x118 protects");
        assert_eq!(View::of_sprr(WE, Some(WE)), View::Rw, "+0x110 write-enables");
        assert_eq!(View::of_sprr(WE, None), View::Rx, "no SPRR commpage: nothing write-enables (T6-f)");
        assert_eq!(View::of_sprr(0, Some(0)), View::Rx, "a zero commpage word never write-enables 0");
    }
}
