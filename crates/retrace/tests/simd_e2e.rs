//! M48 Task 1: callee-saved SIMD registers survive a thread switch and a sigreturn, and a threaded
//! replay does not depend on the host environment. Before the fix, every SIMD restore installed the
//! host's v0 (walls.md §4 item 1). `simd_dyn` then printed MISMATCH (t0 M9), and node's walk 1
//! replayed clean under one login environment and diverged under another.
mod util;
use retrace_guest::SIMD_DYN;

fn native(mode: &str) -> Vec<u8> {
    let out = std::process::Command::new(SIMD_DYN).arg(mode).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "native simd_dyn {mode} must pass: {}", String::from_utf8_lossy(&out.stdout));
    out.stdout
}

#[test]
fn a_thread_switch_preserves_the_callee_saved_simd_registers() {
    let want = native("thread");
    assert_eq!(want, b"simd thread intact\n");
    util::assert_rung_records_and_replays(SIMD_DYN, &["thread"], &want);
}

#[test]
fn a_sigreturn_restores_the_interrupted_simd_registers() {
    let want = native("signal");
    assert_eq!(want, b"simd signal intact\n");
    util::assert_rung_records_and_replays(SIMD_DYN, &["signal"], &want);
}

// The property walls.md §4 lost: one recording replays identically whatever the replayer's
// environment. A larger environment shifts the host's stack and so what the host leaves in v0.
#[test]
fn a_threaded_replay_does_not_depend_on_the_host_environment() {
    let rec = util::assert_rung_records_and_replays(SIMD_DYN, &["thread"], b"simd thread intact\n");
    let pad = "x".repeat(4096);
    let envs: [Vec<(&str, &str)>; 3] =
        [vec![], vec![("M48_SIMD_PAD", pad.as_str())], vec![("M48_SIMD_PAD", "1"), ("PWD", "/")]];
    for env in &envs {
        let rep = util::replay_env(&rec.trace, env);
        assert_eq!(rep.code, 0, "replay under {env:?} must exit 0:\n{}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "replay under {env:?} diverged from the recording");
    }
}
