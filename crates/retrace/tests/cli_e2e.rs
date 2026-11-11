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
fn retrace_dir_places_the_trace() {
    let d = scratch_dir("dir");
    let dir = d.join("nested/dir");
    let base = std::path::Path::new(retrace_guest::HELLO_DYN).file_name().unwrap().to_str().unwrap();
    let want = dir.join(format!("{base}.retrace"));
    let rec = util::run_bounded(util::bin(), &["record", retrace_guest::HELLO_DYN],
        &[("RETRACE_DIR", dir.to_str().unwrap())], BOUND);
    assert_eq!(rec.code, Some(0), "stderr:\n{}", rec.stderr);
    assert!(want.exists(), "{} missing", want.display());
    assert!(rec.stderr.contains(&format!("saved the recording to {}", want.display())), "stderr:\n{}", rec.stderr);
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
    assert_eq!(String::from_utf8_lossy(&v.stdout).trim_end(), "retrace 0.1.0 (macOS 26 and 27)");
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

/// Review Focus 5 for all four arguments: ARGV_ECHO prints only argv[1], so an Apple binary that echoes
/// them all is the check. `-o x -- --help` after the program are the program's, and retrace names no trace
/// `x`, prints no help, and treats nothing as its own.
#[test]
fn every_option_spelled_argument_after_the_program_reaches_it() {
    let d = scratch_dir("echo-all");
    let native = std::process::Command::new("/bin/echo").args(["-o", "x", "--", "--help"]).env_clear().output().unwrap();
    assert_eq!(native.stdout, b"-o x -- --help\n");
    let rec = util::run_in(&d, &["record", "/bin/echo", "-o", "x", "--", "--help"], &[], BOUND);
    assert_eq!(rec.code, Some(0), "stderr:\n{}", rec.stderr);
    assert_eq!(rec.stdout, native.stdout);
    assert!(!d.join("x").exists(), "-o after the program names nothing");
    assert!(d.join("echo.retrace").exists());
}

/// `retrace record --help` and `-h`, in the program position, are a request for help. A program named
/// `--help` stays reachable after `--`.
#[test]
fn record_help_in_the_program_position_prints_help() {
    let d = scratch_dir("record-help");
    for h in ["--help", "-h"] {
        let r = util::run_in(&d, &["record", h], &[], BOUND);
        assert_eq!(r.code, Some(0), "{h}: {}", r.stderr);
        assert!(String::from_utf8_lossy(&r.stdout).starts_with("usage: retrace <command>"), "{h}");
        let r = util::run_in(&d, &["record", "-o", "t", h], &[], BOUND);
        assert_eq!(r.code, Some(0), "-o t {h}: {}", r.stderr);
        assert!(String::from_utf8_lossy(&r.stdout).starts_with("usage: retrace <command>"), "-o t {h}");
    }
    let r = util::run_in(&d, &["record", "--", "--help"], &[], BOUND);
    assert_eq!(r.code, Some(2), "{}", r.stderr);
    assert!(r.stderr.starts_with("retrace: cannot read --help: "), "{}", r.stderr);
}

#[test]
fn a_bare_name_that_cannot_be_read_says_to_give_a_path() {
    let d = scratch_dir("bare-name");
    let r = util::run_in(&d, &["record", "ls"], &[], BOUND);
    assert_eq!(r.code, Some(2), "{}", r.stderr);
    assert_eq!(r.stderr.trim_end(),
        "retrace: cannot read ls: No such file or directory (os error 2) - give a path, such as ./ls or /bin/ls");
    let r = util::run_in(&d, &["record", "./ls"], &[], BOUND);
    assert!(!r.stderr.contains("give a path"), "{}", r.stderr);
}

#[test]
fn no_command_prints_the_help_and_exits_2_and_help_names_the_walls_url() {
    let bare = cli(&[]);
    assert_eq!(bare.code, Some(2));
    let help = cli(&["--help"]);
    assert_eq!(help.code, Some(0));
    assert_eq!(bare.stderr.trim_end(), String::from_utf8_lossy(&help.stdout).trim_end(), "the same text");
    assert!(bare.stderr.contains(&format!("listed at {}", retrace_arch::WALLS_URL)), "{}", bare.stderr);
    assert!(!bare.stderr.contains("in docs/walls.md."), "{}", bare.stderr);
    assert!(bare.stderr.contains("basename plus .retrace in the working directory"), "{}", bare.stderr);
}

/// P-18: `replay` and `lldb` take a recording. The usual slip is naming the program, not its trace.
#[test]
fn a_file_that_is_not_a_recording_is_bad_input_for_replay_and_lldb() {
    use std::os::unix::fs::PermissionsExt;
    let d = scratch_dir("not-a-trace");
    std::fs::write(d.join("note.txt"), b"just some text\n").unwrap();
    std::fs::copy("/bin/echo", d.join("echo_copy")).unwrap();
    std::fs::set_permissions(d.join("echo_copy"), std::fs::Permissions::from_mode(0o755)).unwrap();
    for cmd in ["replay", "lldb"] {
        let r = util::run_in(&d, &[cmd, "note.txt"], &[], BOUND);
        assert_eq!(r.code, Some(2), "{cmd}: {}", r.stderr);
        assert_eq!(r.stderr.trim_end(), "retrace: note.txt is not a retrace recording", "{cmd}");
        let r = util::run_in(&d, &[cmd, "echo_copy"], &[], BOUND);
        assert_eq!(r.code, Some(2), "{cmd}: {}", r.stderr);
        assert_eq!(r.stderr.trim_end(),
            "retrace: echo_copy is not a retrace recording - record it first: retrace record echo_copy", "{cmd}");
    }
}

/// A trace of another format version keeps the message `open_checked` gives it: the check must not swallow it.
#[test]
fn a_trace_of_another_format_version_keeps_its_own_message() {
    let d = scratch_dir("skew");
    std::fs::write(d.join("old.retrace"), b"RT\x00\x0bxxxxxxxx").unwrap();
    let r = util::run_in(&d, &["replay", "old.retrace"], &[], BOUND);
    assert!(r.stderr.contains("recorded by a different retrace trace format"), "{}", r.stderr);
    assert!(!r.stderr.contains("is not a retrace recording"), "{}", r.stderr);
    let fake = d.join("fakelldb.sh");
    std::fs::write(&fake, "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo lldb-1; exit 0; fi\nexit 0\n").unwrap();
    std::process::Command::new("chmod").args(["+x", fake.to_str().unwrap()]).status().unwrap();
    let r = util::run_in(&d, &["lldb", "old.retrace"], &[("RETRACE_LLDB", fake.to_str().unwrap())], BOUND);
    assert!(r.stderr.contains("recorded by a different retrace trace format"), "{}", r.stderr);
    assert!(!r.stderr.contains("is not a retrace recording"), "{}", r.stderr);
}
