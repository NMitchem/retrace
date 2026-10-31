// M25 headline gate: the real CPython interpreter binary, `-c 'print(1)'`.
//
// Two guests, two very different postures:
//
// - `the_real_cpython_interpreter_records_and_replays` points at the actual interpreter and RUNS,
//   un-`#[ignore]`d since M26 — this is rung 7. M25 cleared the record side (DC ZVA, fd_operands)
//   and then parked here, at a replay that diverged 560-odd landmarks in. The cause was not in
//   CPython and not in the replay path at all: `forward_and_diff` snapshotted a flat 64 KiB
//   `PTR_WINDOW_CAP` window per pointer argument while the forwarded read count was clamped only
//   by the destination's backing, so CPython's 88 KB `.pyc` read had its last 22567 bytes written
//   into guest memory on record and captured in no `Event`. Replay restored stale bytes there and
//   the guest — correctly — reported `bad marshal data (unknown type code)`. `bigread_e2e` is the
//   repo-owned regression guard for that mechanism; this test is the capability gate above it.
// - `the_launcher_execs_the_interpreter_and_prints_1` points at the launcher shim `python3` resolves to on PATH. The
//   launcher `posix_spawn`s (syscall 244, `POSIX_SPAWN_SETEXEC`) the real interpreter in its place so the process gets a
//   proper bundle identity. Since M54 task 4 retrace models that exec in place, so the launcher's process becomes the
//   interpreter, prints 1 and exits 0, and two replays reproduce it. (Until then the spawn was refused and the launcher
//   exited 1 through `pythonw.c`'s `err(1, ...)`; that gap is closed.)
//
// Neither guest path is a repo artifact (both come from a Homebrew `python@3.14` install), so both
// tests skip with a loud `util::announce` line (writes past libtest's capture) naming the missing
// path rather than passing quietly — a silent skip reads as a green it did not earn.
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
fn the_real_cpython_interpreter_records_and_replays() {
    if !std::path::Path::new(REAL).exists() {
        util::announce(&format!(
            "SKIPPED the_real_cpython_interpreter_records_and_replays: {REAL} not found \
             (expected a Homebrew `python@3.14` install). This gate did NOT run — it is not \
             evidence of anything."
        ));
        return;
    }
    // `assert_rung_records_and_replays` demands a clean exit(0) with exactly this stdout and
    // replays TWICE, so a guest that died inside dyld cannot pass, and neither can one that
    // reached "core initialized" and then failed importing `encodings` (M25 t0 finding 2).
    util::assert_rung_records_and_replays(REAL, &["-c", "print(1)"], b"1\n");
}

#[test]
fn the_launcher_execs_the_interpreter_and_prints_1() {
    if !std::path::Path::new(LAUNCHER).exists() {
        util::announce(&format!(
            "SKIPPED the_launcher_execs_the_interpreter_and_prints_1: {LAUNCHER} \
             not found (expected a Homebrew `python@3.14` install). This gate did NOT run — it is \
             not evidence of anything."
        ));
        return;
    }
    // M54 t4: the launcher's `posix_spawn(POSIX_SPAWN_SETEXEC)` replaces the image with the real interpreter,
    // which prints 1 and exits 0. `assert_rung_records_and_replays_env` demands exactly that stdout and exit 0
    // and replays TWICE, each equal to the recording.
    let (_, stderr) = util::assert_rung_records_and_replays_env(LAUNCHER, &["-c", "print(1)"], b"1\n", &[]);
    // The difference this work makes: before M54 t4 the spawn was refused (M38's EFAULT) and the launcher
    // exited 1 with a `retrace: refused: posix_spawn` line; a recording that execs has neither.
    // (The interpreter itself still prints a `mach_msg2` refusal line of its own, as it does when run directly.)
    assert!(!stderr.contains("posix_spawn"), "neither a refusal line nor the launcher's own error text: the spawn execs. stderr:\n{stderr}");
}
