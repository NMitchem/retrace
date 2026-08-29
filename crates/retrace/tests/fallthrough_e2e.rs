// M23 t2: the EL1 vector fall-through count must be IDENTICAL on record and replay.
mod util;

/// t1 made trampoline padding trap (`hvc #1`) instead of decoding as `UDF #0`, which means a
/// fall-through past a vector slot head now **recovers**: the padding leaves `ESR_EL1`/`ELR_EL1`
/// intact, so the dispatch re-reads the still-valid exception and carries on. A silent recovery is
/// precisely the failure a determinism oracle structurally cannot see — record and replay would
/// agree with each other while the anomaly vanished. It is the M18 `semaphore_wait_trap` argument
/// applied to a new case: the thing that must not happen is not a wrong answer, it is an *agreed*
/// wrong answer.
///
/// The count is deliberately **not** in the trace. An `Event` variant would renumber every landmark,
/// which `Event::Sched`'s removal already settled. `Box_::run()` is shared by record and replay
/// (symmetry rule 2), so both sides compute the count independently and this gate compares the two
/// reported numbers — which is why the invariant fails loud *here* rather than at a runtime assert:
/// no single process holds both numbers.
///
/// **Known limit 1 (design risk R1):** the count is per-run, not per-position. A mismatch says
/// determinism broke, not where. Recording positions would be a trace-format change for a phenomenon
/// expected to be empty in every gate guest; if this ever fires, that is the moment to spend it.
///
/// **Known limit 2 — the fixture is 0 == 0, and that is measured, not assumed.** Every one of the 34
/// Apple system binaries that records end-to-end today takes ZERO fall-throughs (M23 t2 sweep: 34/34
/// `rec=0 rep=0`), and `hello_dyn` is one more. The guests that DO fall through (measured 4 or 8
/// each: `aa`, `flex`, `dyld_info`, `desdp`, ...) are exactly the ones that cannot yet be recorded —
/// they stop at the XPC wall. So no fixture with a nonzero count exists to build this gate on.
///
/// What that costs, stated plainly rather than left implicit: a 0 == 0 fixture catches a count that
/// is OFFSET (mutation: replay reports `n + 1` -> caught) and a count that is MISSING (mutation:
/// record stops reporting -> caught), but it cannot catch a replay side that stops counting
/// altogether and reports a constant 0 (mutation: verified **NOT** caught). The in-process test
/// `retrace-box/tests/trampoline.rs::a_fall_through_onto_vector_padding_is_counted` covers the
/// counter itself; what stays uncovered is only the plumbing from the box to the reported number, on
/// the replay side. Revisit when a fall-through-taking guest becomes recordable — that is a fixture
/// this gate should be moved onto, not a second test.
#[test]
fn fall_through_counts_match_between_record_and_replay() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "replay failed: {}", rep.stderr);
    let r = fall_throughs(&rec.stderr, "record");
    let p = fall_throughs(&rep.stderr, "replay");
    assert_eq!(r, p, "EL1 vector fall-through count diverged: record {r}, replay {p}");
}

/// Both sides must *report* the count, not merely compute it — an unreported count is exactly as
/// invisible as an uncounted one, so the absence of the line is itself a failure, never a zero.
fn fall_throughs(stderr: &str, side: &str) -> u64 {
    const TAG: &str = "[retrace] fall-throughs: ";
    let line = stderr.lines().find(|l| l.starts_with(TAG)).unwrap_or_else(|| panic!(
        "{side} reported no fall-through count (expected a line starting {TAG:?}); \
         an unreported count is as invisible as an uncounted one.\nstderr:\n{stderr}"));
    line[TAG.len()..].trim().parse().unwrap_or_else(|e| panic!("{side}: parse {line:?}: {e}"))
}
