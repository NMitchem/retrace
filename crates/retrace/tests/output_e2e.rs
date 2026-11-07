//! M51 §3e: the program's output under retrace. stdout and stderr stay apart, are printed as the
//! program writes them, and nothing of retrace's own is printed beside them unless asked for
//! (`RETRACE_VERBOSE`). Every run here goes through the CLI, because the split and the live sink
//! are the CLI's.
mod util;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const BOUND: u64 = 120;

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("retrace-output-{}-{name}", std::process::id()))
}

fn record(mode: &str, t: &str, env: &[(&str, &str)]) -> util::Bounded {
    util::run_bounded(util::bin(), &["record", "-o", t, retrace_guest::FID_OUTPUT, mode], env, BOUND)
}

fn replay(t: &str, env: &[(&str, &str)]) -> util::Bounded {
    util::run_bounded(util::bin(), &["replay", t], env, BOUND)
}

/// Poll `child` every 50 ms for at most `secs` seconds, then kill it. `None` when the bound fired.
fn wait_bounded(child: &mut std::process::Child, secs: u64) -> Option<i32> {
    for _ in 0..secs * 20 {
        if let Some(st) = child.try_wait().expect("try_wait") { return st.code(); }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

/// Spec §4 "The split": a program's stderr bytes are retrace's stderr, never its stdout, on record
/// and on replay, as natively. Before M51 both streams went to retrace's stdout.
#[test]
fn stderr_bytes_never_reach_stdout() {
    let native = util::run_bounded(retrace_guest::FID_OUTPUT, &["split"], &[], BOUND);
    assert_eq!((native.code, native.stdout.as_slice(), native.stderr.as_str()),
               (Some(0), &b"out1\nout2\n"[..], "err1\nerr2\n"), "native");
    let t = scratch("split.bin");
    let tp = t.to_str().unwrap();
    let rec = record("split", tp, &[]);
    assert_eq!((rec.code, rec.stdout.as_slice(), rec.stderr.as_str()),
               (Some(0), &b"out1\nout2\n"[..], "err1\nerr2\n"), "the recording, as native");
    for i in 1..=2 {
        let rp = replay(tp, &[]);
        assert_eq!((rp.code, rp.stdout.as_slice(), rp.stderr.as_str()),
                   (Some(0), &b"out1\nout2\n"[..], "err1\nerr2\n"), "replay {i}, as native");
    }
    let _ = std::fs::remove_file(&t);
}

/// Review Focus 2: `2>&1`, or a terminal, puts both streams in one place. They interleave in the
/// program's write order, as natively.
#[test]
fn stdout_and_stderr_into_one_file_interleave_as_natively() {
    let both = scratch("both.out");
    let run = |prog: &str, args: &[&str]| {
        let f = std::fs::File::create(&both).unwrap();
        let st = Command::new(prog).args(args).env_clear().stdin(Stdio::null())
            .stdout(f.try_clone().unwrap()).stderr(f).status().unwrap();
        (st.code(), std::fs::read_to_string(&both).unwrap())
    };
    let native = run(retrace_guest::FID_OUTPUT, &["split"]);
    assert_eq!(native, (Some(0), "out1\nerr1\nout2\nerr2\n".to_owned()), "native");
    let t = scratch("both.bin");
    let tp = t.to_str().unwrap();
    assert_eq!(run(util::bin(), &["record", "-o", tp, retrace_guest::FID_OUTPUT, "split"]), native, "record");
    assert_eq!(run(util::bin(), &["replay", tp]), native, "replay");
    let _ = std::fs::remove_file(&t);
    let _ = std::fs::remove_file(&both);
}

/// Spec §4 "Live output": the recorder is killed after the program's first line, and the line is
/// already on retrace's stdout. Buffered to the end, nothing is.
#[test]
fn output_is_printed_as_the_program_writes_it() {
    let t = scratch("stall.bin");
    let out = scratch("stall.out");
    let mut child = Command::new(util::bin())
        .args(["record", "-o", t.to_str().unwrap(), retrace_guest::FID_OUTPUT, "stall"])
        .env_clear().stdin(Stdio::null())
        .stdout(std::fs::File::create(&out).unwrap()).stderr(Stdio::null())
        .spawn().unwrap();
    let mut seen = Vec::new();
    for _ in 0..1200 {
        seen = std::fs::read(&out).unwrap_or_default();
        if !seen.is_empty() { break; }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(String::from_utf8_lossy(&seen), "first\n", "the first line, printed while the program still ran");
    let _ = std::fs::remove_file(&t);
    let _ = std::fs::remove_file(&out);
}

/// Review Focus 1: `retrace record-dyn prog | head -1`. The reader goes away after one line; retrace
/// stops writing to it (R5), records the program to its end, and exits with the program's own code,
/// with no bug banner. The recording is whole: its replay prints every line.
#[test]
fn a_reader_that_goes_away_does_not_stop_the_recording() {
    let t = scratch("many.bin");
    let tp = t.to_str().unwrap();
    let err = scratch("many.err");
    let mut child = Command::new(util::bin())
        .args(["record", "-o", tp, retrace_guest::FID_OUTPUT, "many"])
        .env_clear().stdin(Stdio::null()).stdout(Stdio::piped())
        .stderr(std::fs::File::create(&err).unwrap()).spawn().unwrap();
    let mut first = String::new();
    let mut r = std::io::BufReader::new(child.stdout.take().unwrap());
    r.read_line(&mut first).unwrap();
    drop(r);
    assert_eq!(first, "line 0\n");
    let code = wait_bounded(&mut child, BOUND);
    let stderr = std::fs::read_to_string(&err).unwrap();
    assert_eq!(code, Some(7), "the program's own exit code. stderr:\n{stderr}");
    assert!(!stderr.contains("internal error"), "a reader going away is not a bug:\n{stderr}");
    let rp = replay(tp, &[]);
    let all: usize = (0..20000).map(|i| format!("line {i}\n").len()).sum();
    assert_eq!((rp.code, rp.stdout.len()), (Some(7), all), "the replay prints every line. stderr:\n{}", rp.stderr);
    let _ = std::fs::remove_file(&t);
    let _ = std::fs::remove_file(&err);
}

/// FW-4: retrace's own `refused:` line goes through a writer that drops its error, as the program's
/// output does. A program that survives a refusal (the fork fixture, whose `fork` is refused with EAGAIN
/// and which then prints and exits 0) ends with its own code when stderr's reader has already gone, not at
/// the bug banner (`eprintln!` panics on EPIPE). M54 moved it off `exec_dyn`, whose `execve` now succeeds
/// and so prints no refusal (Ruling T2-a); the control below shows this fixture's line is printed.
#[test]
fn a_refusal_line_to_a_closed_stderr_does_not_become_a_bug() {
    // Control: with stderr open, the refusal's line is there, so the closed-stderr run below is a run
    // that had a line to lose.
    let t = scratch("refused-open.bin");
    let open = util::run_bounded(util::bin(), &["record", "-o", t.to_str().unwrap(), retrace_guest::FORKFAIL_DYN], &[], BOUND);
    assert_eq!(open.code, Some(0), "{}", open.stderr);
    assert!(open.stderr.lines().any(|l| l.starts_with("retrace: refused: fork") && l.ends_with("returned EAGAIN to the program")),
        "control: the refusal line is printed when stderr is open:\n{}", open.stderr);
    let t = scratch("refused.bin");
    let mut child = Command::new(util::bin())
        .args(["record", "-o", t.to_str().unwrap(), retrace_guest::FORKFAIL_DYN])
        .env_clear().stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
    // The reader goes away before dyld has finished starting the program, so the refusal's line has nowhere to go.
    drop(child.stderr.take());
    let code = wait_bounded(&mut child, BOUND);
    assert_eq!(code, Some(0), "the program's own exit code, not exit 70");
    let _ = std::fs::remove_file(&t);
}

/// FW-R: the `DIVERGENCE` and `repro:` lines go through the same writer, so a diverging replay whose stderr
/// reader has gone away still exits 3 and not at an abort.
#[test]
fn a_divergence_line_to_a_closed_stderr_does_not_become_a_bug() {
    use retrace_trace::{Event, Reader, Writer};
    let t = scratch("diverge.bin");
    let tp = t.to_str().unwrap();
    let rec = util::run_bounded(util::bin(), &["record", "-o", tp, retrace_guest::FID_BADPTR, "readunmapped"], &[], BOUND);
    assert_eq!(rec.code, Some(0), "{}", rec.stderr);
    let mut events = Reader::open(&t).unwrap();
    let i = events.iter().rposition(|e| matches!(e, Event::Syscall { num: 3, err: true, .. })).expect("the refused read");
    if let Event::Syscall { ret, err, .. } = &mut events[i] { *ret = 16; *err = false; }
    let mut w = Writer::create(&t).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);
    let mut child = Command::new(util::bin()).args(["replay", tp])
        .env_clear().stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
    drop(child.stderr.take());
    assert_eq!(wait_bounded(&mut child, BOUND), Some(3), "exit 3 for a divergence, not an abort");
    let _ = std::fs::remove_file(&t);
}

/// Spec §4 "Quiet": `warn` printed `[retrace warn] anon PROT_EXEC mmap …` before M51, and every run
/// printed `[retrace] fall-throughs: N`. A plain run prints neither, on record or replay.
#[test]
fn a_plain_run_prints_no_line_of_retraces_own() {
    let t = scratch("warn.bin");
    let tp = t.to_str().unwrap();
    let rec = record("warn", tp, &[]);
    assert_eq!((rec.code, rec.stdout.as_slice(), rec.stderr.as_str()), (Some(0), &b"mapped\n"[..], ""), "record");
    let rp = replay(tp, &[]);
    assert_eq!((rp.code, rp.stdout.as_slice(), rp.stderr.as_str()), (Some(0), &b"mapped\n"[..], ""), "replay");
    let _ = std::fs::remove_file(&t);
}

/// R4: `RETRACE_VERBOSE=1` brings retrace's own diagnostic lines back, on record and on replay.
#[test]
fn retrace_verbose_prints_retraces_diagnostic_lines_on_record_and_replay() {
    let t = scratch("verbose.bin");
    let tp = t.to_str().unwrap();
    let env = [("RETRACE_VERBOSE", "1")];
    let rec = record("warn", tp, &env);
    assert_eq!(rec.code, Some(0), "{}", rec.stderr);
    assert!(rec.stderr.lines().any(|l| l.starts_with("[retrace warn] anon PROT_EXEC mmap")), "record:\n{}", rec.stderr);
    assert!(rec.stderr.lines().any(|l| l.starts_with("[retrace] fall-throughs: ")), "record:\n{}", rec.stderr);
    let rp = replay(tp, &env);
    assert_eq!(rp.code, Some(0), "{}", rp.stderr);
    assert!(rp.stderr.lines().any(|l| l.starts_with("[retrace] fall-throughs: ")), "replay:\n{}", rp.stderr);
    let _ = std::fs::remove_file(&t);
}

/// The program's crash from the recording's terminal event: its `sig`.
fn crash_sig(trace: &str) -> u64 {
    let (events, torn) = retrace_trace::Reader::open_checked(std::path::Path::new(trace)).unwrap();
    assert!(!torn, "a complete recording");
    match &events[events.len() - 2] {
        retrace_trace::Event::Crash { sig, .. } => *sig,
        other => panic!("the second-to-last event must be the Crash, got {other:?}"),
    }
}

/// The `retrace:` lines a run printed.
fn retrace_lines(stderr: &str) -> Vec<&str> { stderr.lines().filter(|l| l.starts_with("retrace:")).collect() }

/// Spec §3c: a crash ends with one closing line, after the program's output, naming the signal, the
/// fault and the function, and the exit code is native's. On record and on both replays.
#[test]
fn a_crash_ends_with_one_closing_line_naming_the_signal_and_the_function() {
    let native = util::run_bounded(retrace_guest::FID_OUTPUT, &["crash"], &[], BOUND);
    assert_eq!((native.code, native.stdout.as_slice()), (Some(139), &b"before\n"[..]), "native");
    let t = scratch("crash.bin");
    let tp = t.to_str().unwrap();
    let rec = record("crash", tp, &[]);
    assert_eq!(crash_sig(tp), 11, "the recording's Crash carries the signal it died of");
    for (what, run) in [("record", rec), ("replay 1", replay(tp, &[])), ("replay 2", replay(tp, &[]))] {
        assert_eq!((run.code, run.stdout.as_slice()), (Some(139), &b"before\n"[..]), "{what}: {}", run.stderr);
        let lines = retrace_lines(&run.stderr);
        assert_eq!(lines.len(), 1, "{what}: one closing line. stderr:\n{}", run.stderr);
        assert_eq!(run.stderr.lines().last(), Some(lines[0]), "{what}: the closing line is the last line");
        assert!(lines[0].starts_with("retrace: the program was killed by SIGSEGV (fault at 0x0, pc 0x")
            && lines[0].contains(" in _fid_store_null") && lines[0].ends_with(')') && lines[0].is_ascii(),
            "{what}: {}", lines[0]);
    }
    let _ = std::fs::remove_file(&t);
}

/// Spec §3c: a fatal signal the program raised ends with the same line, without a place.
#[test]
fn a_fatal_abort_ends_with_one_closing_line_naming_sigabrt() {
    let t = scratch("abort.bin");
    let tp = t.to_str().unwrap();
    let rec = record("abort", tp, &[]);
    for (what, run) in [("record", rec), ("replay 1", replay(tp, &[])), ("replay 2", replay(tp, &[]))] {
        assert_eq!((run.code, run.stdout.as_slice()), (Some(134), &b"before\n"[..]), "{what}: {}", run.stderr);
        assert_eq!(retrace_lines(&run.stderr), ["retrace: the program was killed by SIGABRT"], "{what}");
    }
    let _ = std::fs::remove_file(&t);
}

/// Review Focus 3: `strlen(NULL)` faults inside the shared cache, whose symbols the recording does
/// not hold (C2). The line says "a system library" rather than guess a name, and the exit code is
/// native's.
#[test]
fn a_crash_inside_a_system_library_says_so() {
    let native = util::run_bounded(retrace_guest::FID_OUTPUT, &["crashlib"], &[], BOUND);
    assert_eq!(native.code, Some(139), "native: {}", native.stderr);
    let t = scratch("crashlib.bin");
    let tp = t.to_str().unwrap();
    let rec = record("crashlib", tp, &[]);
    for (what, run) in [("record", rec), ("replay", replay(tp, &[]))] {
        assert_eq!(run.code, native.code, "{what}: {}", run.stderr);
        let lines = retrace_lines(&run.stderr);
        assert_eq!(lines.len(), 1, "{what}: {}", run.stderr);
        assert!(lines[0].starts_with("retrace: the program was killed by SIGSEGV (fault at 0x0, pc 0x")
            && lines[0].ends_with(" in a system library)"), "{what}: {}", lines[0]);
    }
    let _ = std::fs::remove_file(&t);
}

/// Final review minor 1: `--help` and `--version` into a reader that has gone away end quietly with exit 0,
/// not at the bug banner (`println!` panics on EPIPE).
#[test]
fn help_and_version_to_a_closed_stdout_do_not_become_a_bug() {
    for arg in ["--help", "--version"] {
        for _ in 0..5 {
            let mut child = Command::new(util::bin()).arg(arg)
                .env_clear().stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
            drop(child.stdout.take());
            let mut err = String::new();
            std::io::Read::read_to_string(child.stderr.as_mut().unwrap(), &mut err).unwrap();
            assert_eq!(wait_bounded(&mut child, BOUND), Some(0), "{arg}: stderr:\n{err}");
            assert!(!err.contains("internal error"), "{arg}: {err}");
        }
    }
}
