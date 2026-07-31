// RETRACE_TRACE=1 must show the terminal fault, not just syscalls. CLAUDE.md advertises the flag as
// logging every dispatched trap, and M7's bring-up diagnosis needs the fault's pc/esr/far.
mod util;

#[test]
fn trace_log_shows_the_terminal_fault() {
    let trace = std::env::temp_dir().join(format!("retrace-faultlog-{}.bin", std::process::id()));
    let out = std::process::Command::new(util::bin())
        .args(["record", retrace_guest::CRASH, "-o", trace.to_str().unwrap()])
        .env("RETRACE_TRACE", "1")
        .output().unwrap();
    assert_eq!(out.status.code(), Some(139), "crash guest must record a crash");
    let err = String::from_utf8_lossy(&out.stderr);
    // crash.s stores to the never-mapped GARBAGE_VA => a lower-EL data abort (EC 0x24).
    assert!(err.contains("[fault] "), "no [fault] line in the trace log:\n{err}");
    assert!(err.contains("far=0x4000dead0000"), "[fault] line lacks the fault address:\n{err}");
    assert!(err.contains("ec=0x24"), "[fault] line lacks the data-abort class:\n{err}");
}
