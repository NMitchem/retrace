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
// - `the_launcher_records_and_replays_its_own_posix_spawn_failure` points at the launcher shim
//   `python3` resolves to on PATH, and it RUNS from this commit onward — but it pins a KNOWN GAP,
//   not a capability. The launcher's job is to `posix_spawn` (syscall 244, `POSIX_SPAWN_SETEXEC`)
//   the real interpreter binary in its place so the process gets a proper bundle identity;
//   exec-in-place is unmodelled in retrace today, so the `posix_spawn` is refused (M38) with the
//   errno the forward used to return — before M38 it was forwarded and EFAULTed on its
//   untranslated guest `argv`/`envp`; now `record_box`'s exec arm returns that same errno without
//   forwarding (`retrace_arch::exec_refusal_errno`, spec R4 continuity) — instead of replacing
//   the image, and the guest takes `pythonw.c`'s `err(1, …)` path and prints
//   "posix_spawn: …: Undefined error: 0" before exiting 1. Record and replay
//   agreeing byte-for-byte on THAT outcome is retrace working correctly — the divergence oracle
//   has nothing to disagree about — not a bug to fix. A successor milestone that implements
//   exec-in-place will change what the launcher guest actually does (it will exec into the real
//   interpreter and exit 0 having printed "1"), and at that point this test must be REWRITTEN to
//   assert the new behavior, not preserved as a regression check: it exists to hold a limitation
//   visible, not to define a requirement that exec-in-place must never happen.
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
fn the_launcher_records_and_replays_its_own_posix_spawn_failure() {
    if !std::path::Path::new(LAUNCHER).exists() {
        util::announce(&format!(
            "SKIPPED the_launcher_records_and_replays_its_own_posix_spawn_failure: {LAUNCHER} \
             not found (expected a Homebrew `python@3.14` install). This gate did NOT run — it is \
             not evidence of anything."
        ));
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
         refused (M38) and returns an error instead of replacing the image. stderr:\n{}",
        rec.stderr
    );

    // The load-bearing assertion. Exit code 1 alone is a code a weaker failure would also
    // produce; only the guest's own error text proves it actually ran pythonw.c's err(1, ...)
    // path rather than failing for some unrelated reason that happens to also exit 1.
    //
    // M51 §3e: the text is on `rec.stderr`, where the program wrote it, beside the recorder's own
    // `retrace: refused:` line. Before M51 retrace merged the program's fd 2 into its stdout.
    assert!(
        rec.stderr.contains("posix_spawn"),
        "expected the guest's own posix_spawn error text on stderr, got: {}",
        rec.stderr
    );

    // Replay must reproduce the exit and the guest's fd-2 error text. Replay prints no refusal
    // line (it never meets the refusal), so its stderr is the program's text alone.
    let rep = util::replay(&trace);
    assert_eq!(rep.code, rec.code, "replay exit code diverged from the recording");
    assert_eq!(rep.stdout, rec.stdout, "replay stdout diverged from the recording");
    assert!(rep.stderr.contains("posix_spawn"), "replay must reproduce the guest's stderr text, got: {}", rep.stderr);
    // The program's whole stderr, not only the word, with retrace's own `retrace:` lines set aside (replay
    // prints no refusal line, and the closing line is retrace's).
    let program = |s: &str| s.lines().filter(|l| !l.starts_with("retrace:")).collect::<Vec<_>>().join("\n");
    assert_eq!(program(&rep.stderr), program(&rec.stderr), "replay must reproduce the program's stderr in full");

    // M38: the posix_spawn is REFUSED now, not forwarded — the refusal line is the only thing
    // that distinguishes the two, since the errno the guest reads was chosen to match (spec R4).
    assert!(rec.stderr.contains("retrace: refused: posix_spawn (syscall 244), returned EFAULT"),
        "the launcher's posix_spawn must be refused by the M38 arm, not forwarded. stderr:\n{}", rec.stderr);
}
