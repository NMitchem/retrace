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
/// **Known limit (design risk R1):** the count is per-run, not per-position. A mismatch says
/// determinism broke, not where. Recording positions would be a trace-format change, and this gate
/// is what would justify spending it; if it ever fires, that is the moment.
///
/// **Two fixtures, and the second one is the whole point.** `hello_dyn` takes zero fall-throughs, as
/// does every one of the 34 Apple system binaries that recorded before this milestone (t2 sweep:
/// 34/34 `rec=0 rep=0`). A `0 == 0` fixture catches a count that is OFFSET or MISSING but *cannot*
/// catch a replay side that stops counting and reports a constant 0 — verified by mutation, not
/// assumed. When t2 was written no nonzero fixture existed: the guests that fall through were
/// exactly the ones stuck at the XPC wall. **t5 cleared that wall**, so `/usr/bin/aa` now records
/// and replays with a stable count of 5 (four consecutive runs, record and replay alike), and the
/// constant-0 mutation is caught. It skips loudly rather than passing quietly if the binary is
/// absent, the rule `jq_e2e` established for a fixture that is not a repo artifact.
#[test]
fn fall_through_counts_match_between_record_and_replay() {
    // The always-available baseline: zero, but it proves the reporting path exists at all.
    check("hello_dyn", retrace_guest::HELLO_DYN, None);

    // The fixture that actually exercises the phenomenon.
    const AA: &str = "/usr/bin/aa";
    if !std::path::Path::new(AA).exists() {
        eprintln!("SKIPPING the nonzero half of this gate: {AA} is missing. The zero fixture cannot \
                   catch a replay side that stops counting, so this run checked strictly less.");
        return;
    }
    let n = check("aa", AA, None);
    assert!(n > 0,
        "/usr/bin/aa took {n} fall-throughs, but 5 were measured on this platform. A zero here means \
         the counter stopped counting, which is the exact silent failure this gate exists for — \
         investigate before relaxing this assertion.");
}

/// Record and replay `guest`, require BOTH sides to report a count, and return it. `expect` pins the
/// value when the caller knows it.
fn check(name: &str, guest: &str, expect: Option<u64>) -> u64 {
    let (rec, trace) = util::record_dynamic(guest);
    assert_eq!(rec.code, 0, "{name}: record failed: {}", rec.stderr);
    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "{name}: replay failed: {}", rep.stderr);
    let r = fall_throughs(&rec.stderr, "record");
    let p = fall_throughs(&rep.stderr, "replay");
    assert_eq!(r, p, "{name}: EL1 vector fall-through count diverged: record {r}, replay {p}");
    if let Some(e) = expect { assert_eq!(r, e, "{name}: expected {e} fall-throughs, got {r}"); }
    r
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
