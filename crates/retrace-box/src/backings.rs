//! M44 B1: the guest's backings, with a sorted index beside them.
//!
//! `read_guest`, `read_guest_checked`, `host_span` and `backing_of` scanned `backings` linearly, and
//! M43's step pre-decode made that scan hot (+38 % CPU on stepping-heavy tests; status log, M43
//! "What stays owed"). The index answers the same questions by binary search. **It sits beside the
//! Vec and never reorders it** (M44 R2): the Vec's order is what `snapshot` and `checkpoint`
//! iterate, and changing it could change what a snapshot's bytes mean.
//!
//! Correctness rests on one invariant, asserted on every insert: no two backings' IPA spans
//! overlap. Stage-2 maps each backing at its IPA and HVF refuses a mapping over a mapped range, so
//! the invariant already held; the assert makes a violation loud instead of a silent mis-index.
use crate::Backing;

/// `(start, len, pos)` per backing, sorted by `start`; `pos` is the backing's index in the Vec.
#[derive(Default)]
pub(crate) struct SpanIndex { e: Vec<(u64, usize, usize)> }

impl SpanIndex {
    /// Index the span at Vec position `pos`. Panics if it overlaps a span already indexed.
    pub(crate) fn insert(&mut self, start: u64, len: usize, pos: usize) {
        let i = self.e.partition_point(|&(s, _, _)| s < start);
        if let Some(&(ps, pl, _)) = i.checked_sub(1).map(|j| &self.e[j]) {
            assert!(ps + pl as u64 <= start, "backing {start:#x}+{len:#x} overlaps {ps:#x}+{pl:#x}");
        }
        if let Some(&(ns, nl, _)) = self.e.get(i) {
            assert!(start + len as u64 <= ns, "backing {start:#x}+{len:#x} overlaps {ns:#x}+{nl:#x}");
        }
        self.e.insert(i, (start, len, pos));
    }
    /// Forget Vec position `pos`; every later position shifts down by one, as `Vec::remove` does.
    pub(crate) fn remove(&mut self, pos: usize) {
        let i = self.e.iter().position(|&(_, _, p)| p == pos).expect("an indexed position");
        self.e.remove(i);
        for x in &mut self.e { if x.2 > pos { x.2 -= 1; } }
    }
    /// The backing holding all of `[ipa, ipa + len)` — `read_guest`'s test, `ipa >= start &&
    /// ipa + len <= end`. With `len == 0` that admits `ipa == end`, as the scan did. `checked_add`
    /// answers `None` where the scan's `ipa + len` would have overflowed.
    pub(crate) fn holding(&self, ipa: u64, len: usize) -> Option<usize> {
        let j = self.e.partition_point(|&(s, _, _)| s <= ipa).checked_sub(1)?;
        let (s, l, p) = self.e[j];
        ipa.checked_add(len as u64).is_some_and(|end| end <= s + l as u64).then_some(p)
    }
    /// The backing whose span contains the address `ipa` — `host_span`'s strict test.
    pub(crate) fn containing(&self, ipa: u64) -> Option<usize> {
        let j = self.e.partition_point(|&(s, _, _)| s <= ipa).checked_sub(1)?;
        let (s, l, p) = self.e[j];
        (ipa < s + l as u64).then_some(p)
    }
}

/// The Vec of backings and its index. Derefs to `[Backing]` for every read-only use; every
/// mutation is a method here, so the index cannot fall behind the Vec.
pub(crate) struct Backings { v: Vec<Backing>, idx: SpanIndex }

impl Backings {
    pub(crate) fn new() -> Self { Backings { v: Vec::new(), idx: SpanIndex::default() } }
    pub(crate) fn push(&mut self, b: Backing) {
        self.idx.insert(b.ipa, b.len, self.v.len());
        self.v.push(b);
    }
    pub(crate) fn extend(&mut self, it: impl IntoIterator<Item = Backing>) { for b in it { self.push(b); } }
    pub(crate) fn remove(&mut self, pos: usize) -> Backing {
        self.idx.remove(pos);
        self.v.remove(pos)
    }
    pub(crate) fn holding(&self, ipa: u64, len: usize) -> Option<&Backing> { self.idx.holding(ipa, len).map(|p| &self.v[p]) }
    pub(crate) fn containing(&self, ipa: u64) -> Option<&Backing> { self.idx.containing(ipa).map(|p| &self.v[p]) }
}

impl std::ops::Deref for Backings {
    type Target = [Backing];
    fn deref(&self) -> &[Backing] { &self.v }
}

#[cfg(test)]
mod tests {
    use super::SpanIndex;
    use retrace_sim::Rng;

    /// The scans the index replaces, over a model Vec kept in insertion order.
    fn lin_holding(m: &[(u64, usize)], ipa: u64, len: usize) -> Option<usize> {
        m.iter().position(|&(s, l)| ipa >= s && ipa + len as u64 <= s + l as u64)
    }
    fn lin_containing(m: &[(u64, usize)], ipa: u64) -> Option<usize> {
        m.iter().position(|&(s, l)| ipa >= s && ipa < s + l as u64)
    }

    #[test]
    fn the_index_answers_every_probe_as_the_linear_scan_did() {
        const G: u64 = 0x4000; // one 16 KiB granule
        for seed in 0..64 {
            let mut r = Rng::seed(seed);
            let (mut idx, mut m) = (SpanIndex::default(), Vec::<(u64, usize)>::new());
            for _ in 0..200 {
                if !m.is_empty() && r.below(4) == 0 {
                    let pos = r.below(m.len() as u64) as usize;
                    idx.remove(pos);
                    m.remove(pos);
                } else {
                    let start = r.below(4096) * G;
                    let len = ((1 + r.below(8)) * G) as usize;
                    if m.iter().all(|&(s, l)| start + len as u64 <= s || s + l as u64 <= start) {
                        idx.insert(start, len, m.len());
                        m.push((start, len));
                    }
                }
                for _ in 0..32 {
                    // Probes biased onto the edges, where an off-by-one lives.
                    let ipa = match m.get(r.below(m.len().max(1) as u64) as usize) {
                        Some(&(s, l)) => [s, s + l as u64, s + l as u64 - 1, s.saturating_sub(1)][r.below(4) as usize],
                        None => r.below(4096 * G),
                    };
                    let len = [0usize, 1, 8, G as usize, 3 * G as usize][r.below(5) as usize];
                    let (got, want) = (idx.holding(ipa, len), lin_holding(&m, ipa, len));
                    // With len 0 two backings can both hold a span where one ends and the next
                    // begins; the scan took the first in Vec order, the index the one starting
                    // there. Either way `read_guest` returns an empty Vec, so only presence
                    // matters at len 0.
                    if len == 0 { assert_eq!(got.is_some(), want.is_some(), "seed {seed}: holding({ipa:#x}, 0)"); }
                    else { assert_eq!(got, want, "seed {seed}: holding({ipa:#x}, {len})"); }
                    assert_eq!(idx.containing(ipa), lin_containing(&m, ipa), "seed {seed}: containing({ipa:#x})");
                }
            }
        }
    }

    #[test]
    fn a_zero_length_read_at_a_backings_end_is_held_as_the_scan_held_it() {
        let mut idx = SpanIndex::default();
        idx.insert(0x4000, 0x4000, 0);
        assert_eq!(idx.holding(0x8000, 0), Some(0), "the scan accepted ipa == end for len 0");
        assert_eq!(idx.holding(0x8000, 1), None);
        assert_eq!(idx.containing(0x8000), None, "host_span's test is strict");
    }

    #[test]
    #[should_panic(expected = "overlaps")]
    fn an_overlapping_insert_fails_loud() {
        let mut idx = SpanIndex::default();
        idx.insert(0x8000, 0x8000, 0);
        idx.insert(0xc000, 0x4000, 1);
    }
}
