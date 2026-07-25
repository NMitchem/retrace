// M6 golden crash-debug transcripts. Every coordinate DISCOVERED from the freshly-recorded
// trace (house convention); ground truth for the corrupting store comes from an independent
// memory-scan oracle, exactly like watch_cli.rs's discover_store_ks.
mod util;
use std::path::Path;

const GARBAGE_VA: u64 = 0x4000_DEAD_0000; // mirrors c/crashy.c

fn debug_run(trace: &str, script: &str) -> (i32, String, String) {
    let out = std::process::Command::new(util::bin())
        .args(["debug", trace, "--script", script])
        .output().expect("spawn debug");
    (out.status.code().unwrap_or(-1),
     String::from_utf8(out.stdout).unwrap(),
     String::from_utf8(out.stderr).unwrap())
}

fn recorded_crash_pc(trace: &Path) -> u64 {
    retrace_trace::Reader::open(trace).unwrap().iter().find_map(|e| match e {
        retrace_trace::Event::Crash { pc, .. } => Some(*pc),
        _ => None,
    }).expect("trace has a Crash event")
}

#[test]
fn continue_parks_at_the_crash_and_where_names_it() {
    let (rec, trace) = util::record_dynamic(retrace_guest::CRASHY);
    assert_eq!(rec.code, 139, "stderr: {}", rec.stderr);
    let ts = trace.to_str().unwrap();
    let crash_pc = recorded_crash_pc(Path::new(&trace));
    let (code, out, err) = debug_run(ts, "continue; where");
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("guest crashed: pc={crash_pc:#x} far=0x4000dead0000")),
            "crash line:\n{out}");
    // Parked AT the fault: where's pc is the crash pc (the faulting instruction, un-retired).
    assert!(out.trim_end().ends_with(&format!("pc={crash_pc:#x}")), "where:\n{out}");
}

#[test]
fn reverse_continue_from_the_crash_finds_the_corrupting_store() {
    let (rec, trace) = util::record_dynamic(retrace_guest::CRASHY);
    assert_eq!(rec.code, 139, "stderr: {}", rec.stderr);
    let ts = trace.to_str().unwrap();
    let (_st, ptr) = util::discover_crashy_addrs(Path::new(&trace));
    // THE demo: run to the crash, watch the corrupted pointer, run BACKWARD to its last writer,
    // then prove it: g.ptr still holds the pre-store value (pre-retire), and one stepi later it
    // holds GARBAGE_VA.
    let script = format!(
        "continue; watch 0x{ptr:x}; reverse-continue; x 0x{ptr:x} 8; stepi; x 0x{ptr:x} 8");
    let (code, out, err) = debug_run(ts, &script);
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("hit watch 0x{ptr:x} (write at ")), "watch hit:\n{out}");
    let garbage_hex = GARBAGE_VA.to_le_bytes().iter()
        .map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
    let xs: Vec<&str> = out.lines().filter(|l| l.starts_with(&format!("0x{ptr:x}:"))).collect();
    assert_eq!(xs.len(), 2, "two x dumps:\n{out}");
    assert!(!xs[0].contains(&garbage_hex), "before the store g.ptr is NOT yet garbage:\n{out}");
    assert!(xs[1].contains(&garbage_hex), "after one stepi the corrupting store retired:\n{out}");
}
