//! M51 §3f: the program gets the environment `retrace record` received, in order and byte for byte,
//! minus `RETRACE_*` (R2). Replay never reads the host's: it is in the recording. An environment too
//! large for the start stack is refused before the VM exists (R3).
mod util;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const BOUND: u64 = 120;
const P: &str = retrace_guest::FID_ENV;

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("retrace-envp-{}-{name}", std::process::id()))
}

fn record(t: &str, env: &[(&str, &str)]) -> util::Bounded {
    util::run_bounded(util::bin(), &["record", "-o", t, P], env, BOUND)
}

/// Spec §4 "The environment": what the program prints of its environment under retrace is what it
/// prints natively with the same one.
#[test]
fn the_program_sees_the_environment_retrace_was_given() {
    let env = [("ALPHA", "1"), ("BETA", "two words"), ("GAMMA", "")];
    let native = util::run_bounded(P, &[], &env, BOUND);
    assert_eq!(String::from_utf8_lossy(&native.stdout), "ALPHA=1\nBETA=two words\nGAMMA=\ncount=3\n", "native");
    let t = scratch("seen.bin");
    let rec = record(t.to_str().unwrap(), &env);
    assert_eq!((rec.code, rec.stdout.as_slice()), (Some(0), native.stdout.as_slice()), "record. stderr:\n{}", rec.stderr);
    let _ = std::fs::remove_file(&t);
}

/// R2: `RETRACE_*` configures retrace, not the program, so the program never sees it.
#[test]
fn retrace_variables_are_not_passed_to_the_program() {
    let t = scratch("stripped.bin");
    let rec = record(t.to_str().unwrap(), &[("KEEP", "yes"), ("RETRACE_VERBOSE", "1")]);
    assert_eq!((rec.code, String::from_utf8_lossy(&rec.stdout)), (Some(0), "KEEP=yes\ncount=1\n".into()), "{}", rec.stderr);
    let _ = std::fs::remove_file(&t);
}

/// Spec §4: replay never reads the host's environment. A recording replayed under a different one
/// prints what was recorded, twice.
#[test]
fn a_replay_under_another_environment_prints_what_was_recorded() {
    let t = scratch("other.bin");
    let tp = t.to_str().unwrap();
    let rec = record(tp, &[("ALPHA", "recorded")]);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), "ALPHA=recorded\ncount=1\n", "{}", rec.stderr);
    for env in [&[("ALPHA", "replayed"), ("OMEGA", "new")][..], &[][..]] {
        let rp = util::run_bounded(util::bin(), &["replay", tp], env, BOUND);
        assert_eq!((rp.code, rp.stdout.as_slice()), (Some(0), rec.stdout.as_slice()), "replay under {env:?}. stderr:\n{}", rp.stderr);
    }
    let _ = std::fs::remove_file(&t);
}

/// R3: arguments plus environment over the limit exit 2 with one line, before any VM, and write no
/// recording.
#[test]
fn an_environment_over_the_limit_exits_2_before_the_vm() {
    let t = scratch("big.bin");
    let big = "x".repeat(retrace_core::START_ARGS_LIMIT + 1);
    let rec = record(t.to_str().unwrap(), &[("BIG", &big)]);
    let lines: Vec<&str> = rec.stderr.lines().filter(|l| l.starts_with("retrace:")).collect();
    assert_eq!((rec.code, lines.len()), (Some(2), 1), "exit 2, one line. stderr:\n{}", rec.stderr);
    assert!(lines[0].starts_with("retrace: the arguments and environment take ")
        && lines[0].contains(&format!("more than retrace's limit of {} KiB", retrace_core::START_ARGS_LIMIT / 1024))
        && lines[0].ends_with("run it under `env -i` with only what the program needs"), "{}", lines[0]);
    assert!(!t.exists(), "no recording is written");
}

/// Review Focus 4: an environment with a non-UTF-8 value, an empty value and a value containing `=`
/// reaches the program byte for byte, as natively.
#[test]
fn awkward_environment_bytes_reach_the_program_unchanged() {
    let env: [(&[u8], &[u8]); 3] = [(b"BYTES", b"caf\xe9\xff"), (b"EMPTY", b""), (b"EQUALS", b"a=b=c")];
    let run = |prog: &str, args: &[&str]| {
        let mut c = Command::new(prog);
        c.args(args).env_clear().stdin(Stdio::null()).stderr(Stdio::null());
        for (k, v) in env { c.env(OsStr::from_bytes(k), OsStr::from_bytes(v)); }
        c.output().unwrap()
    };
    let native = run(P, &[]);
    assert_eq!(native.stdout, b"BYTES=caf\xe9\xff\nEMPTY=\nEQUALS=a=b=c\ncount=3\n", "native");
    let t = scratch("awkward.bin");
    let rec = run(util::bin(), &["record", "-o", t.to_str().unwrap(), P]);
    assert_eq!((rec.status.code(), rec.stdout), (Some(0), native.stdout), "record");
    let _ = std::fs::remove_file(&t);
}
