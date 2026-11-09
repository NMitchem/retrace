// M57 §3c: a guest that reserves 24 GiB FIXED above 64 GiB, commits inside it FIXED|OVERWRITE and uses it, as macOS 27's
// xzone malloc does. Native's stdout is the oracle; the fixture's addresses go to stderr only.
mod util;
use retrace_guest::IPA40_DYN;

#[test]
fn ipa40_records_and_replays_twice_as_natively() {
    let native = util::run_bounded(IPA40_DYN, &[], &[], 30);
    assert_eq!(native.code, Some(0), "native: {}", native.stderr);
    let (_, rec_err) = util::assert_rung_records_and_replays_env(IPA40_DYN, &[], &native.stdout, &[]);
    assert!(rec_err.contains("ipa40 reserved at"), "{rec_err}");
}

#[test]
fn a_seek_into_the_committed_range_reads_the_pattern_after_a_checkpointed_restore() {
    let (rec, trace) = util::record_dynamic_args(IPA40_DYN, &[]);
    assert_eq!(rec.code, 0, "{}", rec.stderr);
    let line = rec.stderr.lines().find(|l| l.starts_with("ipa40 reserved at")).expect("the fixture's address line");
    let seg = line.rsplit("committed at ").next().unwrap().trim().to_string();
    let script = format!("break _ipa40_done; continue; x {seg} 16; reverse-stepi; x {seg} 16");
    let (code, out, err) = util::debug_bounded(trace.to_str().unwrap(), &script, 120);
    assert_eq!(code, Some(0), "{err}\n{out}");
    let want = format!("{seg}: a0 a1 a2 a3 a4 a5 a6 a7 a8 a9 aa ab ac ad ae af");
    let n = out.lines().filter(|l| l.trim() == want).count();
    assert_eq!(n, 2, "want two lines {want:?}:\n{out}\nstderr:\n{err}");
    let _ = std::fs::remove_file(&trace);
}
