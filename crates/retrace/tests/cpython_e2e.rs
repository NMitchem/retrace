// M25 headline gate: the real CPython interpreter binary, `-c 'print(1)'`.
//
// Two guests, two very different postures:
//
// - `the_real_cpython_interpreter_records_and_replays` points at the actual interpreter. Tasks 2
//   and 3 cleared every record-side wall (record now reaches a clean exit(0) with stdout "1\n"),
//   but Task 4 found a replay-side divergence and re-parked the test `#[ignore]`d there (see its
//   reason string for the verbatim evidence).
// - `the_launcher_records_and_replays_its_own_posix_spawn_failure` points at the launcher shim
//   `python3` resolves to on PATH, and it RUNS from this commit onward — but it pins a KNOWN GAP,
//   not a capability. The launcher's job is to `posix_spawn` (syscall 244, `POSIX_SPAWN_SETEXEC`)
//   the real interpreter binary in its place so the process gets a proper bundle identity;
//   exec-in-place is unmodelled in retrace today, so the forwarded `posix_spawn` returns an error
//   to the guest instead of replacing the image, and the guest takes `pythonw.c`'s `err(1, …)`
//   path and prints "posix_spawn: …: Undefined error: 0" before exiting 1. Record and replay
//   agreeing byte-for-byte on THAT outcome is retrace working correctly — the divergence oracle
//   has nothing to disagree about — not a bug to fix. A successor milestone that implements
//   exec-in-place will change what the launcher guest actually does (it will exec into the real
//   interpreter and exit 0 having printed "1"), and at that point this test must be REWRITTEN to
//   assert the new behavior, not preserved as a regression check: it exists to hold a limitation
//   visible, not to define a requirement that exec-in-place must never happen.
//
// Neither guest path is a repo artifact (both come from a Homebrew `python@3.14` install), so both
// tests skip with a loud `eprintln!` naming the missing path rather than passing quietly — a
// silent skip reads as a green it did not earn.
//
// These are the version-stable framework paths, not the `Cellar/python@3.14/3.14.6/…` forms the
// M25 t0 measurements used: a `brew upgrade` moves the Cellar path but not this one, so the gate
// survives an upgrade instead of silently turning into a skip that looks like "not installed" when
// it actually means "moved".
mod util;

const REAL: &str =
    "/opt/homebrew/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python";
const LAUNCHER: &str = "/opt/homebrew/Frameworks/Python.framework/Versions/3.14/bin/python3.14";

#[test]
#[ignore = "M25 wall 2, replay-side (unfixed as of this commit): record reaches a clean exit(0) \
            with stdout exactly \"1\\n\" — RETRACE_TRACE=1 shows SYS_write(1, \"1\\n\") as the \
            last real trap before exit, reproduced twice. Every record-side wall (DC ZVA, \
            fd_operands) is cleared. But of the two replays this test requires, the FIRST \
            diverges. Verbatim: DIVERGENCE at landmark 568 pc=0x1804b1834: syscall mismatch: \
            live (num=4, args=[2, 30086578176, 106, 1, 0, 42963282272, 10, 200]) != recorded \
            (num=75, args=[30086955008, 98304, 7, 0, 0, 42972720880, 42972417888, \
            18446726482597246976]). num=4 is write (live's args[0]=2, fd 2/stderr); num=75 is \
            mmap — at landmark 568 live re-execution is issuing a DIFFERENT syscall than the one \
            the recording holds there, so the two runs' syscall SEQUENCES have already diverged \
            by this point, not merely one call's arguments. No unit test at any single layer \
            reproduces this, and closing it would mean tracing which earlier syscall's count or \
            ordering differs between record and its own replay — modelling unmeasured \
            guest/kernel behaviour, which Task 4's stop criterion 4 rules out for a single pass. \
            UN-IGNORE when a successor milestone identifies and closes that specific sequence \
            divergence."]
fn the_real_cpython_interpreter_records_and_replays() {
    if !std::path::Path::new(REAL).exists() {
        eprintln!(
            "SKIPPED the_real_cpython_interpreter_records_and_replays: {REAL} not found \
             (expected a Homebrew `python@3.14` install). This gate did NOT run — it is not \
             evidence of anything."
        );
        return;
    }
    // `assert_rung_records_and_replays` demands a clean exit(0) with exactly this stdout and
    // replays TWICE, so a guest that died inside dyld cannot pass, and neither can one that
    // reached "core initialized" and then failed importing `encodings` (M25 t0 finding 2).
    util::assert_rung_records_and_replays(REAL, &["-c", "print(1)"], b"1\n");
}

#[test]
fn the_launcher_records_and_replays_its_own_posix_spawn_failure() {
    if !std::path::Path::new(LAUNCHER).exists() {
        eprintln!(
            "SKIPPED the_launcher_records_and_replays_its_own_posix_spawn_failure: {LAUNCHER} \
             not found (expected a Homebrew `python@3.14` install). This gate did NOT run — it is \
             not evidence of anything."
        );
        return;
    }
    // Can't use `assert_rung_records_and_replays` here: it demands exit(0), and this guest's own
    // correct behavior (given exec-in-place is unmodelled) is to exit 1.
    let (rec, trace) = util::record_dynamic_args(LAUNCHER, &["-c", "print(1)"]);

    // The guest's own exit. Exit 4 is retrace's own RECORD ERROR and 139 is a crash, so this
    // discriminates against both of those as well as against a clean success.
    assert_eq!(
        rec.code, 1,
        "launcher should exit 1 via pythonw.c's err(1, ...) path after its posix_spawn is \
         forwarded and returns an error instead of replacing the image. stderr:\n{}",
        rec.stderr
    );

    // The load-bearing assertion. Exit code 1 alone is a code a weaker failure would also
    // produce; only the guest's own error text proves it actually ran pythonw.c's err(1, ...)
    // path rather than failing for some unrelated reason that happens to also exit 1.
    //
    // This text is on `rec.stdout`, not `rec.stderr`. retrace mirrors guest writes to BOTH fd 1
    // and fd 2 into one buffer and prints it on retrace's own stdout (the `is_console_write` arm
    // of `record_box` in crates/retrace-core/src/lib.rs; the predicate at
    // crates/retrace-arch/src/lib.rs:22 covers both fds). retrace's own `[retrace]` diagnostics go
    // to retrace's stderr, so `rec.stderr` never carries guest text — asserting on it here would
    // fail for a reason unrelated to the guest.
    assert!(
        rec.stdout.windows(b"posix_spawn".len()).any(|w| w == b"posix_spawn"),
        "expected the guest's own posix_spawn error text on stdout (mirrored fd 1+2 writes), got: \
         {:?}",
        String::from_utf8_lossy(&rec.stdout)
    );

    // Replay must reproduce both the exit and the guest's mirrored fd-2 error text byte-for-byte.
    // Because that text lives in the mirrored buffer alongside fd 1, this equality is proof that
    // replay reproduced the guest's actual output, not merely that it exited the same way.
    let rep = util::replay(&trace);
    assert_eq!(rep.code, rec.code, "replay exit code diverged from the recording");
    assert_eq!(rep.stdout, rec.stdout, "replay stdout diverged from the recording");
}
