// M56 Task 1: the one `retrace record [-o <trace>] <program> [<args>...]`, `--version` and `--help`.
mod util;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const BOUND: u64 = 120;

/// A fresh scratch directory, unique to this process and call.
fn scratch_dir(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!("retrace-cli-{}-{}-{name}", std::process::id(),
                                              NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn cli(args: &[&str]) -> util::Bounded { util::run_bounded(util::bin(), args, &[], BOUND) }

fn retrace_lines(stderr: &str) -> Vec<&str> { stderr.lines().filter(|l| l.starts_with("retrace:")).collect() }

const NEW_FORM: &str = "retrace record [-o <trace>] <program> [<args>...]";

#[test]
fn one_record_takes_a_static_and_a_dynamic_program() {
    for guest in [retrace_guest::HELLO, retrace_guest::HELLO_DYN] {
        let d = scratch_dir("both");
        let t = d.join("t.bin");
        let rec = cli(&["record", "-o", t.to_str().unwrap(), guest]);
        assert_eq!(rec.code, Some(0), "{guest}: record. stderr:\n{}", rec.stderr);
        assert!(!rec.stdout.is_empty(), "{guest}: the program printed something");
        let rep = cli(&["replay", t.to_str().unwrap()]);
        assert_eq!(rep.code, Some(0), "{guest}: replay. stderr:\n{}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "{guest}: replay matches the recording");
    }
}

#[test]
fn a_relative_path_records_replays_and_reaches_the_guest_as_typed() {
    let d = scratch_dir("rel");
    std::fs::copy(retrace_guest::APPLE_DYN, d.join("apple")).unwrap();
    let native = std::process::Command::new("./apple").current_dir(&d).env_clear().output().unwrap().stdout;
    assert_eq!(String::from_utf8_lossy(&native), "argv0=./apple\nexecutable_path=./apple\n");
    let rec = util::run_in(&d, &["record", "./apple"], &[], BOUND);
    assert_eq!(rec.code, Some(0), "record. stderr:\n{}", rec.stderr);
    assert_eq!(rec.stdout, native, "the guest saw the path as typed");
    assert!(d.join("apple.retrace").exists(), "the default trace name");
    assert!(rec.stderr.trim_end().ends_with("retrace: saved the recording to apple.retrace"),
        "stderr:\n{}", rec.stderr);
    let rep = util::run_in(&d, &["replay", "apple.retrace"], &[], BOUND);
    assert_eq!(rep.code, Some(0), "replay. stderr:\n{}", rep.stderr);
    assert_eq!(rep.stdout, native, "replay prints the same");
}

#[test]
fn options_after_the_program_are_the_programs() {
    let d = scratch_dir("opts");
    // ARGV_ECHO prints argv[1]. `-o x -- --help` are the program's, so it prints `-o`, and retrace
    // neither names a trace `x` nor prints its own help.
    let native = std::process::Command::new(retrace_guest::ARGV_ECHO)
        .args(["-o", "x", "--", "--help"]).env_clear().output().unwrap();
    let t = d.join("t.bin");
    let rec = cli(&["record", "-o", t.to_str().unwrap(), retrace_guest::ARGV_ECHO, "-o", "x", "--", "--help"]);
    assert_eq!(rec.code, Some(0), "stderr:\n{}", rec.stderr);
    assert_eq!(rec.stdout, native.stdout);
    assert_eq!(rec.stdout, b"-o\n");
    assert!(!PathBuf::from("x").exists() && !d.join("x").exists(), "-o after the program names nothing");
    let rec2 = cli(&["record", "-o", t.to_str().unwrap(), retrace_guest::ARGV_ECHO, "--help"]);
    assert_eq!(rec2.stdout, b"--help\n", "--help after the program is the program's");
    assert_eq!(rec2.code, Some(0));
}

#[test]
fn an_explicit_o_prints_no_line_of_retraces_own() {
    let d = scratch_dir("quiet");
    let t = d.join("t.bin");
    let rec = cli(&["record", "-o", t.to_str().unwrap(), retrace_guest::HELLO_DYN]);
    assert_eq!(rec.code, Some(0));
    assert!(retrace_lines(&rec.stderr).is_empty(), "stderr:\n{}", rec.stderr);
}

#[test]
fn record_dyn_is_refused_by_name() {
    let r = cli(&["record-dyn", retrace_guest::HELLO_DYN, "-o", "x.bin"]);
    assert_eq!(r.code, Some(2));
    assert_eq!(r.stderr.trim_end(), format!("retrace: record-dyn is now record: {NEW_FORM}"));
}

#[test]
fn a_static_program_given_arguments_is_bad_input() {
    let d = scratch_dir("static-args");
    let t = d.join("t.bin");
    let r = cli(&["record", "-o", t.to_str().unwrap(), retrace_guest::HELLO, "x"]);
    assert_eq!(r.code, Some(2), "stderr:\n{}", r.stderr);
    let lines = retrace_lines(&r.stderr);
    assert_eq!(lines.len(), 1, "stderr:\n{}", r.stderr);
    assert!(lines[0].contains("a static program takes no arguments"), "{}", lines[0]);
    assert!(lines[0].contains("usage: retrace record [-o <trace>] <program> [<args>...]"), "{}", lines[0]);
    assert!(!t.exists());
}

#[test]
fn a_missing_or_directory_program_is_bad_input_and_writes_nothing() {
    let d = scratch_dir("missing");
    for prog in [d.join("nope"), d.clone()] {
        let r = util::run_in(&d, &["record", prog.to_str().unwrap()], &[], BOUND);
        assert_eq!(r.code, Some(2), "{prog:?}: stderr:\n{}", r.stderr);
        assert_eq!(retrace_lines(&r.stderr).len(), 1, "stderr:\n{}", r.stderr);
        assert!(!r.stderr.contains("saved the recording"), "stderr:\n{}", r.stderr);
        let names: Vec<_> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert!(names.is_empty(), "no trace written: {names:?}");
    }
}

#[test]
fn an_unwritable_default_name_is_named_in_the_error() {
    use std::os::unix::fs::PermissionsExt;
    let d = scratch_dir("ro");
    std::fs::copy(retrace_guest::APPLE_DYN, d.join("apple")).unwrap();
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o555)).unwrap();
    let r = util::run_in(&d, &["record", "./apple"], &[], BOUND);
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(r.code, Some(2), "stderr:\n{}", r.stderr);
    let lines = retrace_lines(&r.stderr);
    assert_eq!(lines.len(), 1, "stderr:\n{}", r.stderr);
    assert!(lines[0].contains("apple.retrace"), "{}", lines[0]);
}

#[test]
fn version_and_help_exit_0_and_no_arguments_exits_2() {
    let v = cli(&["--version"]);
    assert_eq!(v.code, Some(0));
    assert_eq!(String::from_utf8_lossy(&v.stdout).trim_end(), "retrace 0.1.0");
    for h in [&["--help"][..], &["help"]] {
        let r = cli(h);
        assert_eq!(r.code, Some(0), "{h:?}");
        let out = String::from_utf8_lossy(&r.stdout);
        for word in ["record", "replay", "lldb", "debug", "gdbserver"] {
            assert!(out.contains(word), "{h:?} lists {word}:\n{out}");
        }
    }
    assert_eq!(cli(&[]).code, Some(2));
}
