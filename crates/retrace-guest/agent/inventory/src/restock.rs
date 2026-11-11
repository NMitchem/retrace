/// Items in one case.
const CASE: u32 = 10;

pub(crate) fn add(slot: &mut u32, cases: u32, loose: u32) {
    *slot += cases * CASE + loose;
}
