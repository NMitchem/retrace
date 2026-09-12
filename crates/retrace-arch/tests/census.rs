//! M33 Task 1 census — every syscall number a guest in this repo's corpora dispatched, measured
//! 2026-09-12 from `RETRACE_TRACE=1`'s `[trap] num=` lines (`crates/retrace-core/src/lib.rs`,
//! `record_box`), record side. That line prints for EVERY syscall stop, before routing, so this
//! OVER-approximates what reaches `forward_and_diff` — the safe direction: a row for an emulated
//! syscall is documentation, a missing row for a forwarded one is a gate guest that panics.
//!
//! Corpora: 59 repo-owned guests (static via `record`, dynamic via `record-dyn`, bare argv; 3 of
//! them — `crash`, `crashjmp`, `wildstore` — fault before issuing any syscall and so contribute no
//! numbers), `jq --version`, `jq . <file>`, the CPython interpreter and its launcher, `/bin/ps`,
//! and all 54 of `tools/apple-sweep-binaries.txt`. Raw per-guest outputs:
//! `.superpowers/sdd/2026-09-12-retrace-m33-readerenum/task-1-census/` and the M33 section of
//! `docs/status-log.md`.
//!
//! `i64` because mach traps are negative; convert with `as u64` to look one up.
//! `every_census_number_has_a_row` (Task 5) is what keeps M33's loud failure from firing on
//! anything the corpora dispatch.
//!
//! One entry, `2147483648` (`0x8000_0000`), is not a BSD/mach syscall number in the usual range:
//! it is a genuine `Stop::Syscall` (x16 read after `ec_of(esr1) == Ec::Svc` confirmed a real `svc`
//! trap — `crates/retrace-box/src/lib.rs`, `run()`), not a decode artifact of this script, and the
//! same value already appears independently in
//! `docs/superpowers/specs/2026-09-02-retrace-m25-cpython-measurements.md`'s CPython census. What
//! it names is unidentified; that identification is left to whichever task gives it a row.
pub const CENSUS: &[i64] = &[
    -89, -70, -50, -47, -36, -33, -29, -28, -27, -26, -24, -19, -18, -15, -14, -12, -10, 1, 3, 4,
    5, 6, 13, 20, 24, 25, 33, 36, 37, 38, 39, 41, 42, 43, 46, 47, 48, 49, 52, 53, 54, 58, 59, 60,
    73, 74, 75, 81, 90, 92, 97, 98, 116, 117, 133, 153, 169, 170, 184, 189, 191, 194, 195, 197,
    199, 202, 220, 228, 244, 266, 286, 294, 327, 328, 329, 331, 336, 338, 339, 340, 344, 346, 347,
    360, 361, 362, 366, 367, 368, 372, 381, 396, 397, 398, 399, 406, 412, 427, 463, 470, 478, 483,
    500, 515, 516, 539, 550, 2147483648,
];

#[test]
fn census_is_sorted_and_deduplicated() {
    assert!(CENSUS.windows(2).all(|w| w[0] < w[1]), "CENSUS must be strictly ascending");
    assert!(CENSUS.len() > 60, "a census this small means a corpus was skipped: {}", CENSUS.len());
}
