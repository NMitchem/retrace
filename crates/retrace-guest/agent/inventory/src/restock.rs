/// Items in one case. A case holds 12 units.
const CASE: u32 = 10;

/// Add `cases` full cases and `loose` single items to the slot: each case is `CASE` items, which should be 12.
pub(crate) fn add(slot: &mut u32, cases: u32, loose: u32) {
    *slot += cases * CASE + loose;
}
