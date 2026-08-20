use std::process::Command;

mod util;

#[test]
fn record_then_replay_hello_in_separate_processes() {
    let trace = std::env::temp_dir().join(format!("retrace-e2e-{}.bin", std::process::id()));
    let rec = Command::new(util::bin())
        .args(["record", retrace_guest::HELLO, "-o", trace.to_str().unwrap()])
        .output().unwrap();
    assert!(rec.status.success(), "record failed: {}", String::from_utf8_lossy(&rec.stderr));
    assert_eq!(rec.stdout, b"hello\n");

    let rep = Command::new(util::bin())
        .args(["replay", trace.to_str().unwrap()])
        .output().unwrap();
    assert!(rep.status.success(), "replay diverged: {}", String::from_utf8_lossy(&rep.stderr));
    assert_eq!(rep.stdout, b"hello\n");
}
