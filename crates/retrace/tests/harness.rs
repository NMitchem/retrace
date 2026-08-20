// A19b: the test harness's own codesign race. `util::bin()` used to codesign
// CARGO_BIN_EXE_retrace **in place** — `codesign -f` replaces the file, so while one test
// process re-signs it, a concurrent test process (cargo runs test *binaries* as separate
// processes; `--test-threads=1` only serialises threads within one) can observe it missing
// and fail with "No such file or directory". Measured during M16 by passing 13 `--test`
// targets in one invocation: `kport` failed, then passed 2/2 alone.
//
// The property that kills the race is "calling bin() does not modify the shared binary" —
// that is what this test pins. It does NOT prove the concurrent race is fixed (a passing
// race is not evidence of anything); it proves the shared file is never touched, which makes
// the race structurally impossible regardless of scheduling.
mod util;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

#[test]
fn bin_does_not_modify_the_shared_binary() {
    let shared = env!("CARGO_BIN_EXE_retrace");
    let before = std::fs::metadata(shared).expect("shared binary must exist before bin()");

    let signed = util::bin();

    let after = std::fs::metadata(shared).expect("shared binary must exist after bin()");
    assert_eq!(before.ino(), after.ino(),
        "bin() replaced the shared binary's inode — codesign -f is still writing {shared} in place");
    assert_eq!(before.mtime(), after.mtime(),
        "bin() modified the shared binary's mtime — codesign -f is still writing {shared} in place");

    assert_ne!(signed, shared,
        "bin() must return a path distinct from the shared binary, not the shared path itself");
    let signed_meta = std::fs::metadata(signed)
        .unwrap_or_else(|e| panic!("bin()'s returned path {signed} does not exist: {e}"));
    assert!(signed_meta.is_file(), "bin()'s returned path {signed} is not a regular file");
    assert!(signed_meta.permissions().mode() & 0o111 != 0,
        "bin()'s returned path {signed} is not executable (mode {:o})", signed_meta.permissions().mode());
}
