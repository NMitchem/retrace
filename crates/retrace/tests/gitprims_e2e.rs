//! M47 gate (spec §3f): the repo-owned fixtures for git's mechanisms, one test per fixture mode.
//! Each test records, replays twice byte-identically, and asserts the difference M47 makes — never
//! an exit code a weaker failure would also produce. Every fixture is C, built by
//! `crates/retrace-guest/build.rs`, and was run natively by t0 for its reference output
//! (`docs/sweep-evidence/2026-09-30-m47-t0/native-*.out`).
mod util;
use std::path::PathBuf;

/// A fresh, empty directory for a fixture that writes to disk.
fn scratch_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("retrace-m47-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Record `guest` with `args`, replay it twice, and hold both replays to the recording.
fn records_and_replays_twice(guest: &str, args: &[&str]) -> (util::RunOut, PathBuf) {
    let (rec, trace) = util::record_dynamic_args(guest, args);
    assert_eq!(rec.code, 0, "record {guest} {args:?}: {}", rec.stderr);
    for i in 0..2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 0, "replay {i} of {guest} {args:?}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "replay {i} of {guest} {args:?}: stdout differs from the recording");
    }
    (rec, trace)
}

/// RED at `427fa0a`: the recorder stops at the M33 panic for mkdir (136), the first of the new rows
/// the fixture reaches. The on-disk assertions are the difference: only a FORWARDED link and
/// utimes can leave a second link and that mtime, and replay forwards nothing.
#[test]
fn fsops_mkdir_chdir_link_and_utimes_are_forwarded_and_land_on_disk() {
    use std::os::unix::fs::MetadataExt;
    let dir = scratch_dir("fsops");
    let (rec, _) = records_and_replays_twice(retrace_guest::FSOPS_DYN, &[dir.to_str().unwrap()]);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), "mkdir chdir link rename utimes ok\nh mtime=1234567890 nlink=2\n");
    let st = std::fs::metadata(dir.join("d/h")).unwrap();
    assert_eq!((st.mtime(), st.nlink()), (1_234_567_890, 2), "utimes and link must have reached the host");
    assert_eq!(std::fs::read(dir.join("d/f")).unwrap(), b"fsops\n", "the relative open after chdir must land in <dir>/d");
}
