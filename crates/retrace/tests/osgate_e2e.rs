// M57 §3e: an unmeasured macOS major is one clean refusal line and exit 4, for every command.
mod util;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const BOUND: u64 = 120;

fn scratch(tag: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!("retrace-osgate-{}-{}-{tag}.bin", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
}

const W: &str = "https://github.com/NMitchem/retrace/blob/main/docs/walls.md";

#[test]
fn an_unsupported_major_is_one_line_and_exit_4_for_every_command() {
    let t = scratch("base");
    let r = util::run_bounded(util::bin(), &["record", "-o", t.to_str().unwrap(), retrace_guest::HELLO], &[], BOUND);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    for (product, build) in [("25.6", "24G100"), ("28.0", "27A100")] {
        let ov = format!("{product},{build}");
        let env = [("RETRACE_DEBUG_MACOS", ov.as_str())];
        let want = format!("retrace: unsupported: macOS {product} ({build}) is not supported: retrace runs on macOS 26 and 27 - see {W}#macos-version");
        let out = scratch("out");
        let tp = t.to_str().unwrap();
        let rows: Vec<Vec<&str>> = vec![
            vec!["record", "-o", out.to_str().unwrap(), retrace_guest::HELLO],
            vec!["replay", tp],
            vec!["debug", tp, "--script", "where"],
            vec!["gdbserver", tp, "--port", "0"],
            vec!["lldb", tp],
        ];
        for args in rows {
            let r = util::run_bounded(util::bin(), &args, &env, BOUND);
            assert_eq!(r.code, Some(4), "{args:?} on {ov}: {}", r.stderr);
            assert_eq!(r.stderr.trim_end(), want, "{args:?} on {ov}");
        }
        assert!(!out.exists(), "record on {ov} wrote a trace");
    }
    let _ = std::fs::remove_file(&t);
}

#[test]
fn the_supported_majors_and_their_point_releases_pass() {
    for ov in ["26.0,25A100", "26.7.2,25G332", "27.2,26B123"] {
        let t = scratch("ok");
        let r = util::run_bounded(util::bin(), &["record", "-o", t.to_str().unwrap(), retrace_guest::HELLO],
            &[("RETRACE_DEBUG_MACOS", ov)], BOUND);
        assert_eq!(r.code, Some(0), "{ov}: {}", r.stderr);
        let _ = std::fs::remove_file(&t);
    }
}

#[test]
fn version_names_the_supported_majors() {
    let r = util::run_bounded(util::bin(), &["--version"], &[], BOUND);
    assert_eq!(r.code, Some(0));
    assert_eq!(String::from_utf8_lossy(&r.stdout).trim_end(), "retrace 0.1.0 (macOS 26 and 27)");
}
