// M36 parked a gate per measured wall in the Apple sweep; M37 moved each to the wall it measured;
// M38 un-parked `launchctl` and moved the other five past the RCV-only message-queue call. M44
// added `arg_kinds` rows the other five's new walls needed (`openat_nocancel`/464, `statfs64`/345,
// `getattrlistbulk`/461, `unlink`/10, `rename`/128): `ls` and `ed` now gate un-parked, and the
// remaining five are re-parked at the wall each now measurably reaches (none is still the M38 RCV
// receive — that call is refused and the run continues past it on every one of them). M45
// emulated `kevent_qos`(374)'s workqueue-kqueue init, which moved `automationmodetool` past it to
// the next call, a second `kevent_qos` shape (libdispatch's by its values — inferred, not
// symbolicated), where it is re-parked with that shape measured and not modelled (M45 §7 Halt 3).
// M46 modelled libdispatch's event manager and UPTIME timers, which moved `automationmodetool` past
// that second `kevent_qos` (the memory-pressure registration, now recorded) to the next wall, a
// workloop thread request through `kevent_id`(375), where it is re-parked (M46 §7 H5).
// M47 refused `fork` (`EAGAIN`) and answered `mach_ports_register` (3403), which moved `csh` and
// `tcsh` past the 3403 to the next call their caller makes after the refused fork, `wait4`(7), a
// syscall with no `arg_kinds` row, where both are re-parked.
// Every `#[ignore]` here is ON PURPOSE, and each reason is the measurement that parks it —
// the label the sweep printed, the exit codes, the recorder-pid regimes, the recorder's own
// RECORD ERROR line with the symbol, the landmark, the committed evidence file, the charter class,
// and what un-parks it. The sweep's old category string ("replay diverged") is not evidence and
// appears in no reason.
//
// Each body is the statement that becomes true when its wall falls: the binary records to a
// clean exit and replays bit-for-bit. The assertion messages print the recorder's stderr tail so
// a run with `--ignored` shows the wall by name (that run is this file's positive control).
mod util;

fn records_and_replays_clean(path: &str) {
    if !std::path::Path::new(path).exists() {
        util::announce(&format!("SKIPPED: {path} is not present on this machine"));
        return;
    }
    let (rec, trace) = util::record_dynamic(path);
    let tail = rec.stderr.lines().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    assert_eq!(rec.code, 0, "{path}: record exited {} — the wall this gate is parked at, in the recorder's own words:\n{tail}", rec.code);
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "{path}: replay exited {}: {}", rp.code, rp.stderr.lines().last().unwrap_or(""));
    assert_eq!(rp.stdout, rec.stdout, "{path}: replay stdout differs from the recording");
}

#[test]
#[ignore = "M47 wall, class C (new subsystem: process creation — the `wait4` a refused fork's caller issues), parked, not routed. /bin/csh: the M37–M46 wall, `mach_ports_register` (msgh_id 3403, from libxpc's `fork` prepare handler), is answered `KERN_SUCCESS` since M47 (landmark 336, one write), and `fork`(2) itself is refused: `[retrace] refusing fork (syscall 2): process creation is unmodelled; returning errno 35 without forwarding` (landmark 337, `ret=35 err=true`). The caller is csh's own `remotehost` (lldb against /bin/csh, unslid: the fork returns through libsystem_c `fork+56` to `remotehost+108`, and `remotehost` is called from `main+3120`). It does not test fork's `-1` and takes the parent's path: `close(7)`, its pipe's write end; `read(6, …, 0x1000)` → 0, EOF; `close(6)`; then `wait4(-1, 0x27fe2cc, 0, NULL)` at pc 0x1804b5478 (returning to `remotehost+236`), landmark 341, which has no `arg_kinds` row. The sweep labels it `FAIL /bin/csh (recorder panicked: thread 'main' (17955312) panicked at crates/retrace-arch/src/lib.rs:1036:38: M33: syscall 7 (7) has no arg_kinds row in crates/retrace-arch/src/lib.rs — it cannot be forwarded unclassified …)`, rc/rp 101/n/a (no replay ran; recpid 549). The walk's traced run is the same stop after 341 traps, and its replay's `DIVERGENCE at landmark 341 pc=0x1804b5478: expected recorded syscall, got None` is the trace with no terminal event the panic left, not a divergence of its own. The landmark is 322 plus the guest's own `gettimeofday`(116) count, which was 19 in the walk; the 3403 sits at 317 plus that count, M45's and M46's constant. Both control rounds on the base binary (427fa0a's code) still stop at the 3403, and both on the swept binary at `wait4`. Natively, with the empty environment retrace gives every guest and every fork failing `EAGAIN` (`ulimit -u 1`), csh exits 0 silently; whether it forks at all natively was not traced. M37 (`dup2`) and M38 (`pipe`'s pair) cleared the calls before the 3403 (docs/sweep-evidence/2026-09-13-m37/, 2026-09-16-m38/). Evidence docs/sweep-evidence/2026-09-30-m47/csh.rec.err, csh.landmarks.txt (the trace's own landmarks 330–340, from a later run whose `gettimeofday` count, 19, matches the walk's), csh.frames.txt, shells-walk.txt, shells-forkfail.txt, controls.txt and sweep/csh.rec.err. UN-IGNORE when the box models process creation (here, the wait for a child the refused fork never made), and whatever the run reaches next is cleared."]
fn csh_records_and_replays() { records_and_replays_clean("/bin/csh"); }

#[test]
#[ignore = "M47 wall, class C (new subsystem: process creation — the `wait4` a refused fork's caller issues), parked, not routed. /bin/tcsh is the same file as /bin/csh (one hard-linked inode, `cmp` 0), and its run takes the same path: the M37–M46 wall, `mach_ports_register` (msgh_id 3403, from libxpc's `fork` prepare handler), is answered `KERN_SUCCESS` since M47 (landmark 340, one write), and `fork`(2) itself is refused: `[retrace] refusing fork (syscall 2): process creation is unmodelled; returning errno 35 without forwarding` (landmark 341, `ret=35 err=true`). The caller is the shell's own `remotehost` (symbolicated on csh's recording, the same binary: the fork returns through libsystem_c `fork+56` to `remotehost+108`, and `remotehost` is called from `main+3120`). It does not test fork's `-1` and takes the parent's path: `close(7)`, its pipe's write end; `read(6, …, 0x1000)` → 0, EOF; `close(6)`; then `wait4(-1, 0x27fe2bc, 0, NULL)` at pc 0x1804b5478, landmark 345, which has no `arg_kinds` row. The sweep labels it `FAIL /bin/tcsh (recorder panicked: thread 'main' (17974050) panicked at crates/retrace-arch/src/lib.rs:1036:38: M33: syscall 7 (7) has no arg_kinds row in crates/retrace-arch/src/lib.rs — it cannot be forwarded unclassified …)`, rc/rp 101/n/a (no replay ran; recpid 4186). The walk's traced run is the same stop after 345 traps, and its replay's `DIVERGENCE at landmark 345 pc=0x1804b5478: expected recorded syscall, got None` is the trace with no terminal event the panic left, not a divergence of its own. The landmark is 322 plus the guest's own `gettimeofday`(116) count, which was 23 in the walk; the 3403 sits at 317 plus that count, M45's and M46's constant. Both control rounds on the base binary (427fa0a's code) still stop at the 3403, and both on the swept binary at `wait4`. Natively, with the empty environment retrace gives every guest and every fork failing `EAGAIN` (`ulimit -u 1`), tcsh exits 0 silently; whether it forks at all natively was not traced. M37 (`dup2`) and M38 (`pipe`'s pair) cleared the calls before the 3403 (docs/sweep-evidence/2026-09-13-m37/, 2026-09-16-m38/). Evidence docs/sweep-evidence/2026-09-30-m47/tcsh.rec.err, tcsh.landmarks.txt (the trace's own landmarks, from a later run with 19 `gettimeofday` calls, so 4 lower than the numbers above: its header gives the offset), csh.frames.txt, shells-walk.txt, shells-forkfail.txt, controls.txt and sweep/tcsh.rec.err. UN-IGNORE when the box models process creation (here, the wait for a child the refused fork never made), and whatever the run reaches next is cleared."]
fn tcsh_records_and_replays() { records_and_replays_clean("/bin/tcsh"); }

/// M38: un-parked — the RCV-only message-queue call is refused (`MACH_RCV_REFUSAL`), and the
/// binary records to a clean exit and replays bit-for-bit (evidence
/// docs/sweep-evidence/2026-09-16-m38/launchctl.MACH_RCV_INVALID_NAME.{rec,rp}.err, and the same
/// under the two losing candidate codes).
///
/// Its clean exit is `1`, not `0`: with no arguments `/bin/launchctl` prints its usage to stdout
/// and exits 1 on the host too (measured 2026-09-16: 4484 bytes, byte-identical to the guest's
/// recording), so `records_and_replays_clean`'s `rc == 0` cannot express this binary and the body
/// asserts on what the binary does instead. Each assertion is on the difference M38 makes, per the
/// honest-gate rule: `1` is a code no retrace failure produces (the CLI's own are 2/3/4/5 and
/// 128+signo), so it can only be the guest's own `exit(1)`; the usage text on stdout is the guest
/// reaching `main` (M37 parked it inside libxpc's initializer, at the receive); and exactly one
/// refusal line on the recorder's stderr is the positive control that the new arm, and not some
/// other path, is what carried it there. Replay is then held to the recording as the shared
/// helper holds it.
#[test]
fn launchctl_records_and_replays() {
    let path = "/bin/launchctl";
    if !std::path::Path::new(path).exists() {
        util::announce(&format!("SKIPPED: {path} is not present on this machine"));
        return;
    }
    let (rec, trace) = util::record_dynamic(path);
    let tail = rec.stderr.lines().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    assert_eq!(rec.code, 1, "{path}: record exited {} — its own no-argument usage exit is 1; the recorder's last lines:\n{tail}", rec.code);
    assert!(rec.stdout.starts_with(b"Usage: launchctl "),
        "{path}: stdout does not start with launchctl's usage text — the guest did not reach main:\n{tail}");
    assert_eq!(rec.stderr.matches("[retrace] refusing mach_msg2 message-queue receive").count(), 1,
        "{path}: expected exactly one RCV-only message-queue refusal on the recorder's stderr:\n{tail}");
    let rp = util::replay(&trace);
    assert_eq!(rp.code, rec.code, "{path}: replay exited {}: {}", rp.code, rp.stderr.lines().last().unwrap_or(""));
    assert_eq!(rp.stdout, rec.stdout, "{path}: replay stdout differs from the recording");
}

#[test]
#[ignore = "M46 wall, class C (new subsystem: libdispatch workloops, whose thread request goes through `kevent_id`(375), a syscall with no `arg_kinds` row), parked, routed to its own milestone (M46 §7 H5, which names workloops and `kevent_id` outside M46). /usr/bin/automationmodetool: libdispatch's event manager and UPTIME timers are modelled since M46 (its memory-pressure registration records at landmark 363, rc 0; the `task_get_special_port(TASK_DEBUG_CONTROL_PORT)` after it is answered `MACH_PORT_NULL` at landmark 364, t0 Ruling T0-a). The run now continues to `kevent_id`(375) at pc 0x1804afa74, landmark 365, rc/rp 101/n/a (no replay ran): `thread 'main' (10935275) panicked at crates/retrace-arch/src/lib.rs:1002:38: M33: syscall 375 (375) has no arg_kinds row in crates/retrace-arch/src/lib.rs — it cannot be forwarded unclassified …`. Its arguments are `x0` (id) = `0x6bc90`, one change and a one-entry event list at the same address (`x1` = `x3` = `0x27ff458`), and `x7` flags = `0x403` = `KEVENT_FLAG_WORKLOOP|KEVENT_FLAG_ERROR_EVENTS|KEVENT_FLAG_IMMEDIATE`. Its change entry, read by the debugger at the svc (pc 0x1804afa70, position (365, 83), thread 0), is ident and udata `0x6bc90`, filter `0xffef` (-17, `EVFILT_WORKLOOP`), flags `0x0005` = `EV_ADD|EV_ENABLE`, qos `0x8ff`, fflags `0x111` = `NOTE_WL_THREAD_REQUEST|NOTE_WL_UPDATE_QOS|NOTE_WL_IGNORE_ESTALE`, `ext[1]` = `0x6bcc8` (the id plus 0x38), `ext[2]` = `0x3700000001` and `ext[3]` = `0x1ffea400000001` (the 8 bytes at `0x6bcc8`). The id is a dispatch queue labelled `com.apple.NSXPCConnection.m-user.com.apple.dt.automationmode.reader`, and the frame chain, symbolicated by lldb against the host's shared cache, is libdispatch `_dispatch_kq_poll+220` (the return address of its `bl kevent_id` at `+216`) ← `_dispatch_event_loop_poke` ← libxpc `_xpc_connection_init_failed` ← `_xpc_connection_init` ← `_xpc_connection_activate_if_needed` ← `xpc_connection_resume` ← AutomationMode `+[XAMObserver sharedInstance]`'s block ← … ← `XAMIsAutomationModeEnabled`: libxpc pokes the connection's workloop from its init-failed path (why the init failed was not traced). Landmark numbers move with the host's `gettimeofday`(116) count (M45 t0 M1). Evidence docs/sweep-evidence/2026-09-29-m46/automationmodetool.rec.err, automationmodetool.{entry,frames}.txt and landmarks.txt. UN-IGNORE when the box models libdispatch's workloop thread requests through `kevent_id`, and whatever the run reaches next is cleared (the XPC connection to `com.apple.dt.automationmode.reader`, a service retrace does not host, is already on its init-failed path here)."]
fn automationmodetool_records_and_replays() { records_and_replays_clean("/usr/bin/automationmodetool"); }

#[test]
#[ignore = "M44 wall, class C (process creation: exec-in-place), parked, not routed. /usr/bin/desdp: `openat_nocancel`(464) records (landmark 395, opening its own `/var/tmp/xcrun_db-XXXXXXXX`) and `rename`(128) records 3 landmarks later (landmark 398, installing `/var/tmp/xcrun_db`); the row now reaches, 7 landmarks later, `[retrace] refusing posix_spawn (syscall 244): exec-in-place is unmodelled; returning errno 14 without forwarding` — `posix_spawn`(244) at landmark 405 — and the guest runs itself to a clean, byte-identical exit(71) on both sides (rc/rp 71/71), printing `desdp: error: couldn't spawn '/Applications/Xcode.app/Contents/Developer/usr/bin/xcodebuild' (errno=No such file or directory)`; that 71 is NOT the native outcome (native `desdp </dev/null` exits 2, usage — t0 M3), so asserting it would pin retrace's refusal, not the program. Their 464/128 reach depends on xcrun's host cache `/var/tmp/xcrun_db` (t0 Ruling T0-e): this re-measure swept each trio member alone after `rm -f /var/tmp/xcrun_db` (Ruling T4-a, a cold cache per member), which is the path that reaches both rows; from a stale cache (already written by another member's run) the first two rows are skipped and `posix_spawn` is reached directly instead — the WALL itself (`posix_spawn`, 244) is reached in both cache states. NOTE: desdp/dyld_info/flex are xcselect shims that dispatch by argv[0] (dyld_info and flex one hard-linked file, desdp another — measured by the M44 final review, docs/sweep-evidence/2026-09-27-m44/native-trio.txt; they were believed to be one), so the tempfile name is the guest's own nondeterminism, never retrace's. Evidence docs/sweep-evidence/2026-09-27-m44-t0/t4/desdp.{rec,rp}.{err,out}. UN-IGNORE when exec-in-place is modelled."]
fn desdp_records_and_replays() { records_and_replays_clean("/usr/bin/desdp"); }

#[test]
#[ignore = "M44 wall, class C (process creation: exec-in-place), parked, not routed. /usr/bin/dyld_info: `openat_nocancel`(464) records (landmark 393, opening its own `/var/tmp/xcrun_db-XXXXXXXX`) and `rename`(128) records 3 landmarks later (landmark 396, installing `/var/tmp/xcrun_db`); the row now reaches, 7 landmarks later, `[retrace] refusing posix_spawn (syscall 244): exec-in-place is unmodelled; returning errno 14 without forwarding` — `posix_spawn`(244) at landmark 403 — and the guest runs itself to a clean, byte-identical exit(71) on both sides (rc/rp 71/71), printing `dyld_info: error: couldn't spawn '/Applications/Xcode.app/Contents/Developer/usr/bin/xcodebuild' (errno=No such file or directory)`; that 71 is NOT the native outcome (native `dyld_info </dev/null` exits 0, printing its usage on stderr: `Usage: dyld_info <options>* <mach-o file>+ | -all_dir <dir> | -all_dyld_cache` — M44 final review, docs/sweep-evidence/2026-09-27-m44/native-trio.txt), so asserting it would pin retrace's refusal, not the program. Their 464/128 reach depends on xcrun's host cache `/var/tmp/xcrun_db` (t0 Ruling T0-e): this re-measure swept each trio member alone after `rm -f /var/tmp/xcrun_db` (Ruling T4-a, a cold cache per member), which is the path that reaches both rows; from a stale cache (already written by another member's run) the first two rows are skipped and `posix_spawn` is reached directly instead — the WALL itself (`posix_spawn`, 244) is reached in both cache states. NOTE: desdp/dyld_info/flex are xcselect shims that dispatch by argv[0] (dyld_info and flex one hard-linked file, desdp another — measured by the M44 final review, docs/sweep-evidence/2026-09-27-m44/native-trio.txt; they were believed to be one), so the tempfile name is the guest's own nondeterminism, never retrace's. Evidence docs/sweep-evidence/2026-09-27-m44-t0/t4/dyld_info.{rec,rp}.{err,out}. UN-IGNORE when exec-in-place is modelled."]
fn dyld_info_records_and_replays() { records_and_replays_clean("/usr/bin/dyld_info"); }

#[test]
#[ignore = "M44 wall, class C (process creation: exec-in-place), parked, not routed. /usr/bin/flex: `openat_nocancel`(464) records (landmark 398, opening its own `/var/tmp/xcrun_db-XXXXXXXX`) and `rename`(128) records 3 landmarks later (landmark 401, installing `/var/tmp/xcrun_db`); the row now reaches, 7 landmarks later, `[retrace] refusing posix_spawn (syscall 244): exec-in-place is unmodelled; returning errno 14 without forwarding` — `posix_spawn`(244) at landmark 408 — and the guest runs itself to a clean, byte-identical exit(71) on both sides (rc/rp 71/71), printing `flex: error: couldn't spawn '/Applications/Xcode.app/Contents/Developer/usr/bin/xcodebuild' (errno=No such file or directory)`; that 71 is NOT the native outcome (native `flex </dev/null` exits 1, printing `<stdin>:1: premature EOF` on stderr and writing no `lex.yy.c` — M44 final review, docs/sweep-evidence/2026-09-27-m44/native-trio.txt), so asserting it would pin retrace's refusal, not the program. Their 464/128 reach depends on xcrun's host cache `/var/tmp/xcrun_db` (t0 Ruling T0-e): this re-measure swept each trio member alone after `rm -f /var/tmp/xcrun_db` (Ruling T4-a, a cold cache per member), which is the path that reaches both rows; from a stale cache (already written by another member's run) the first two rows are skipped and `posix_spawn` is reached directly instead — the WALL itself (`posix_spawn`, 244) is reached in both cache states. NOTE: desdp/dyld_info/flex are xcselect shims that dispatch by argv[0] (dyld_info and flex one hard-linked file, desdp another — measured by the M44 final review, docs/sweep-evidence/2026-09-27-m44/native-trio.txt; they were believed to be one), so the tempfile name is the guest's own nondeterminism, never retrace's. Evidence docs/sweep-evidence/2026-09-27-m44-t0/t4/flex.{rec,rp}.{err,out}. UN-IGNORE when exec-in-place is modelled."]
fn flex_records_and_replays() { records_and_replays_clean("/usr/bin/flex"); }

#[test]
#[ignore = "M44 wall, class C (new subsystem: the I/O Kit main port), parked, not routed. /usr/bin/dddiagnose: `statfs64`(345) records (landmark 433, `ret=2`/ENOENT — the path it stats does not exist, harmlessly); the row now stops 21 landmarks later at `RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 205 dest 0x1c03 (guest task port Some(515)) send_size 24` — msgh_id 205 = `host_get_io_main` (SDK `mach/mach_host.h:1313`, `{ \"host_get_io_main\", 205 }`), rc/rp 4/3, landmark 454 (replay's `DIVERGENCE at landmark 454 pc=0x1804adc34: expected recorded syscall, got None` is the trace with no terminal event the record error left, not a divergence of its own). Evidence docs/sweep-evidence/2026-09-27-m44-t0/t4/dddiagnose.{rec,rp}.err. M45 measured a second outcome, on M44's crates as well as M45's: in 1 of 4 traced runs per binary (swept 09a105f, base a78f28f) and in M45's sweep, the guest data-aborts at pc 0x180302eb0 (esr 0x92000045; far varying run to run, 0x4000050050 / 0x2000050040), about 9 landmarks after the refused receive and before 205, rc/rp 139/139 — M36's libsystem_malloc `mfm_alloc+0x230` face, which M37 retired as unreachable with a correctly forwarded pid, and M38 R3's losing `MACH_RCV_TIMED_OUT` face, now seen under the winning `MACH_RCV_INVALID_NAME`. Evidence docs/sweep-evidence/2026-09-28-m45/{dddiagnose-control.txt,dddiagnose-traps.txt,sweep/dddiagnose.rec.err}. UN-IGNORE when `host_get_io_main` (msgh_id 205) is serviced AND the `mfm_alloc` face is explained or measured absent — servicing 205 alone would leave a gate that fails on about one run in four. M46 re-measured both faces alternating on both binaries (base f907c33, t6 d369fcc): PASS 139/139 (the `mfm_alloc` fault) or FAIL 4/3 at msgh_id 205, which is host state. Evidence docs/sweep-evidence/2026-09-29-m46/{controls,samples}.txt."]
fn dddiagnose_records_and_replays() { records_and_replays_clean("/usr/bin/dddiagnose"); }

/// M44: new gate — `getattrlistbulk`(461) has an `arg_kinds` row (`Dest(Reg(3))`, t0 M2, no citable
/// cap ≤ 65,536 so the row widens the diff window instead) and `/bin/ls` records to a clean exit
/// and replays bit-for-bit (evidence docs/sweep-evidence/2026-09-27-m44-t0/t4/ls.{rec,rp}.err).
/// Native `ls` in `crates/retrace` (the cwd a gate runs in) also exits 0 (t0 M3); the shared helper
/// does not compare against that byte-for-byte, only record against replay, which is what M44 A2
/// makes true for the first time.
#[test]
fn ls_records_and_replays() { records_and_replays_clean("/bin/ls"); }

/// M44: new gate — `openat_nocancel`(464) and `unlink`(10) both have `arg_kinds` rows (t0 Ruling
/// T0-a) and `/bin/ed` records to a clean exit and replays bit-for-bit (evidence
/// docs/sweep-evidence/2026-09-27-m44-t0/t4/ed.{rec,rp}.err). Native `ed </dev/null` also exits 0
/// with empty stdout and stderr (t0 M3), which is exactly `records_and_replays_clean`'s `rc == 0`
/// shape — `ed` needs no launchctl-style special case.
#[test]
fn ed_records_and_replays() { records_and_replays_clean("/bin/ed"); }
