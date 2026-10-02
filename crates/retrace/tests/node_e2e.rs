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
#[ignore = "M47 wall, class C (new subsystem: `kevent` on a guest `kqueue()`), parked, routed to node's next milestone (M47 §7). /opt/homebrew/bin/node (Cellar node 25.6.1, `v25.6.1`) -e 'console.log(1)': its @rpath dylibs load since M47 answers AMFI's dyld policy from the host (`__mac_syscall` \"AMFI\" 0x5a at landmark 59, rc 0); the run then records 1,100 landmarks in 47 s (a 355 MB trace), with no thread created and no `MAP_JIT` mapping among its 121 `mmap`s, and stops at landmark 1,101, `kevent`(363) at pc 0x1804b3fc4, rc 101 (no replay ran): `thread 'main' (17896196) panicked at crates/retrace-arch/src/lib.rs:1036:38: M33: syscall 363 (363) has no arg_kinds row in crates/retrace-arch/src/lib.rs — it cannot be forwarded unclassified (an untranslated guest fd would act on retrace's own descriptor of that number). …`. Its arguments, read by the debugger at the svc (pc 0x1804b3fc0, position (1101, 27), thread 0), are `kevent(7, 0x27ff368, 2, 0x27ff368, 1, 0x27ff358)`: two changes, and a one-entry event list at the same address — ident `0x1e7e7711`, filter `0xfff6` (-10, `EVFILT_USER`), flags `0x21` = `EV_ADD|EV_CLEAR`; then the same ident and filter with fflags `0x01000000` = `NOTE_TRIGGER` — and a zero timeout. fd 7 is what the second `kqueue()`(362) returned, at landmark 1,100; the first, at landmark 1,094, returned 4. The caller is libuv 1.52.1's `uv__kqueue_runtime_detection`: `x30` = `0xa0bd066c0` is libuv's offset `0x66c0`, the return address of that function's `bl kevent`, whose constants match the entry byte for byte. So node stops at libuv's check that `EVFILT_USER` works, on a throwaway kqueue, not on its loop's. Evidence docs/sweep-evidence/2026-09-30-m47/node.rec.err (the last 400 lines of the traced stderr), node.entry.txt, node.frame.txt and node.landmarks.txt; the 2026-09-30 probe stopped at the same call after the same 1,101 traps (docs/sweep-evidence/2026-09-30-m47-probe/node-kevent.txt). UN-IGNORE when `kevent` on a guest `kqueue()` is modelled, and whatever the run reaches next is cleared."]
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
