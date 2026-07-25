// M6: syscall-write watch detection on a REAL MMU-on dynamic guest — the M5 deferral's proof.
// crashy's fstat(1, &g.st) is a recorded kernel write into a watchable global, so the armed VA
// must translate through the guest's own stage-1 tables (va_to_ipa) and intersect it.
mod util;

#[test]
fn syscall_write_watch_fires_on_a_dynamic_guest() {
    let (rec, trace) = util::record_dynamic(retrace_guest::CRASHY);
    assert_eq!(rec.code, 139, "stderr: {}", rec.stderr); // crashy always ends in the planted fault
    let (st, _ptr) = util::discover_crashy_addrs(&trace);
    assert_eq!(st % 8, 0, "g.st ({st:#x}) must be 8-aligned to be watchable");
    let out = std::process::Command::new(util::bin())
        .args(["debug", trace.to_str().unwrap(), "--script",
               &format!("watch 0x{st:x}; continue")])
        .output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains(&format!("hit watch 0x{st:x} (syscall write)")),
            "expected the fstat kernel write to fire the software watch:\n{stdout}");
}
