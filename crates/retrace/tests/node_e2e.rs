//! M47 (spec §1 part 5, §3g): node, parked at its measured wall. Before M47 dyld refused node's
//! `@rpath/libnode.*.dylib` because AMFI's dyld policy read as 0 under retrace (the forwarded
//! `__mac_syscall` EFAULTed); M47 answers it from the host (R4), and node now loads every dylib and
//! runs to the wall below. The Cellar path, not the `/opt/homebrew/bin` symlink, is what the probe
//! and the walk measured.
//!
//! NOT a repo artifact: without Homebrew's node the test announces a skip, even when run with
//! `--ignored`.
mod util;

const NODE: &str = "/opt/homebrew/bin/node";

#[test]
#[ignore = "M47 wall, class C (new subsystem: kevent on a guest kqueue), parked, not routed. /opt/homebrew/bin/node -e 'console.log(1)': its @rpath dylibs load since M47 answers AMFI's dyld policy from the host; the run then stops at kevent (363), which has no arg_kinds row, on the descriptor kqueue (362) returned — libuv's loop — after 1,101 traps, with no thread created and no MAP_JIT mapping (the 2026-09-30 probe, docs/sweep-evidence/2026-09-30-m47-probe/node-kevent.txt; re-measured by M47 Task 6). UN-IGNORE when kevent on a guest kqueue is modelled."]
fn node_prints_one_and_replays() {
    if !std::path::Path::new(NODE).exists() {
        util::announce(&format!("SKIPPED node_prints_one_and_replays: {NODE} not installed (`brew install node`). \
            This gate did NOT run — it is not evidence of anything."));
        return;
    }
    let exe = std::fs::canonicalize(NODE).unwrap();
    let out = util::assert_rung_records_and_replays(exe.to_str().unwrap(), &["-e", "console.log(1)"], b"1\n");
    assert_eq!(out.stdout, b"1\n");
}
