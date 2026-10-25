// M52 §3c: every guest thread has its own synthetic, deterministic id. `thread_selfid` (372) is
// answered by the box and never forwarded, and main's pthread carries its id, so libpthread's
// recursive and error-checking mutexes see distinct owners. Before M52 main read 0 and every child
// read the recorder's one host id. `tid_dyn` is the fixture; rows with a native answer run natively
// first, so the expected output is native's.
mod util;
use retrace_trace::{Event, Reader, Writer};

const WALLS: &str = "https://github.com/NMitchem/retrace/blob/main/docs/walls.md";

/// Native run of `guest` with `args`: exit 0, then record and replay it twice and require the same
/// stdout. Returns that stdout.
fn as_native(guest: &str, args: &[&str]) -> String {
    let native = util::run_bounded(guest, args, &[], 30);
    assert_eq!(native.code, Some(0), "native {guest} {args:?}: {}", native.stderr);
    util::assert_rung_records_and_replays(guest, args, &native.stdout);
    String::from_utf8(native.stdout).unwrap()
}

#[test]
fn every_guest_thread_reads_its_own_nonzero_id_by_every_route() {
    let out = as_native(retrace_guest::TID_DYN, &["routes"]);
    for l in ["routes agree=yes", "distinct=yes", "nonzero=yes", "main_is_not_a_child=yes", "reused=no"] {
        assert!(out.contains(l), "{l}:\n{out}");
    }
}

/// The ids are synthetic, so only the recording and its replays are compared (native's differ).
#[test]
fn the_ids_are_synthetic_and_the_same_on_replay() {
    let (rec, trace) = util::record_dynamic_args(retrace_guest::TID_DYN, &["raw"]);
    assert_eq!(rec.code, 0, "{}", rec.stderr);
    let out = String::from_utf8(rec.stdout.clone()).unwrap();
    let want = format!("main {}\nc1 {}\nc2 {}\nc3 {}\nc4 {}\n", 0x10_0000, 0x10_0001, 0x10_0002, 0x10_0003, 0x10_0004);
    assert_eq!(out, want, "main is MAIN_THREAD_ID and children follow in creation order");
    util::assert_rung_records_and_replays(retrace_guest::TID_DYN, &["raw"], &rec.stdout);
    let rp = util::replay(&trace);
    assert_eq!((rp.code, rp.stdout), (0, rec.stdout), "{}", rp.stderr);
}

#[test]
fn a_recursive_mutex_main_holds_is_busy_to_a_child_and_an_errorcheck_relock_is_edeadlk() {
    let out = as_native(retrace_guest::TID_DYN, &["mutex"]);
    assert_eq!(out, "trylock=16\nrelock=11\n");
}

#[test]
fn a_rewritten_thread_id_is_a_divergence_naming_thread_selfid() {
    let (rec, trace) = util::record_dynamic_args(retrace_guest::TID_DYN, &["raw"]);
    assert_eq!(rec.code, 0, "{}", rec.stderr);
    let mut events = Reader::open(&trace).unwrap();
    let ev = events.iter_mut().find(|e| matches!(e, Event::Syscall { num, .. } if *num == retrace_arch::SYS_THREAD_SELFID))
        .expect("the recording has a thread_selfid landmark");
    if let Event::Syscall { ret, .. } = ev { *ret += 1; }
    let tampered = trace.with_extension("tid.tampered.bin");
    let mut w = Writer::create(&tampered).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);
    let rp = util::replay(&tampered);
    assert_eq!(rp.code, 3, "CLI exit 3 is the Divergence convention; stderr:\n{}", rp.stderr);
    assert!(rp.stderr.contains("thread_selfid"), "the divergence names the call; stderr:\n{}", rp.stderr);
}

/// Natively `proc_pidinfo` answers a live tid. With synthetic ids the tid a guest passes is one the
/// host never issued, so the call is refused before the forward (ruling T2-pidinfo).
#[test]
fn a_thread_lookup_by_id_through_proc_pidinfo_is_refused_by_value() {
    let native = util::run_bounded(retrace_guest::TID_DYN, &["pidinfo"], &[], 30);
    assert_eq!(native.code, Some(0), "{}", native.stderr);
    assert!(String::from_utf8_lossy(&native.stdout).starts_with("pidinfo="), "native answers");
    let t = std::env::temp_dir().join(format!("retrace-m52-pidinfo-{}.bin", std::process::id()));
    let tp = t.to_str().unwrap();
    let rec = util::run_bounded(util::bin(), &["record-dyn", retrace_guest::TID_DYN, "-o", tp, "--", "pidinfo"], &[], 60);
    let lines: Vec<&str> = rec.stderr.lines().filter(|l| l.starts_with("retrace:")).collect();
    assert_eq!(lines.len(), 1, "exactly one retrace: line. stderr:\n{}", rec.stderr);
    assert!(lines[0].starts_with("retrace: unsupported: proc_info") && lines[0].contains("PROC_PIDTHREADID64INFO"), "{}", lines[0]);
    assert!(lines[0].ends_with(&format!(" - see {WALLS}#threads")), "{}", lines[0]);
    assert_eq!(rec.code, Some(4), "a wall exits 4. stderr:\n{}", rec.stderr);
    let rp = util::run_bounded(util::bin(), &["replay", tp], &[], 60);
    assert!(rp.stderr.contains("retrace: incomplete recording: it ends at "), "{}", rp.stderr);
    assert!(!rp.stderr.contains("DIVERGENCE"), "{}", rp.stderr);
    assert_eq!(rp.code, Some(4), "{}", rp.stderr);
    let _ = std::fs::remove_file(&t);
}
