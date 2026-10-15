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
    util::run_bounded(util::bin(), &["record-dyn", retrace_guest::FID_OUTPUT, "-o", t, "--", mode], env, BOUND)
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
    assert_eq!(run(util::bin(), &["record-dyn", retrace_guest::FID_OUTPUT, "-o", tp, "--", "split"]), native, "record");
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
        .args(["record-dyn", retrace_guest::FID_OUTPUT, "-o", t.to_str().unwrap(), "--", "stall"])
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
        .args(["record-dyn", retrace_guest::FID_OUTPUT, "-o", tp, "--", "many"])
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
