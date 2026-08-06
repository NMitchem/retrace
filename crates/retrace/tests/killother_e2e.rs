// M11 SAFETY boundary. A guest kill() aimed at another process must abort the recorder loudly.
// This is the only defect in this milestone that escapes the sandbox: before M11 the operand was
// untranslated and unchecked, so `kill(1, SIGKILL)` from a guest would have been forwarded into
// retrace's own process and signalled launchd.
mod util;

#[test]
fn killing_pid_1_aborts_the_recorder_instead_of_signalling_launchd() {
    let (rec, _trace) = util::record(retrace_guest::KILLOTHER);
    assert_ne!(rec.code, 0, "the guest must NOT reach its exit(0); stderr: {}", rec.stderr);
    assert!(rec.stderr.contains("kill to a pid other than the guest's own"),
            "the abort must name the boundary it enforced; stderr: {}", rec.stderr);
    // And launchd is still there, which is the actual thing being protected.
    assert!(pid_exists(1), "pid 1 must still exist — signalling it is what this test prevents");
}

/// Does `pid` still exist? `kill(pid, 0)` is the existence probe: it sends no signal and only
/// performs the permission/existence check.
///
/// The three answers must be told apart, and conflating them would gut this test:
///
/// - `0` — exists and is signallable.
/// - `-1`/`EPERM` — EXISTS but we may not signal it. This is the normal answer for launchd from a
///   non-root process, so treating it as failure would fail every ordinary run.
/// - `-1`/`ESRCH` — GONE. This is precisely the catastrophe the milestone guards against, so it
///   must NOT be lumped in with EPERM just because both return -1.
fn pid_exists(pid: i32) -> bool {
    const EPERM: i32 = 1;
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    if unsafe { kill(pid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(EPERM)
}
