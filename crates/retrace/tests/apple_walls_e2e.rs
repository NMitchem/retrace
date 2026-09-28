// M36 parked a gate per measured wall in the Apple sweep; M37 moved each to the wall it measured;
// M38 un-parked `launchctl` and moved the other five past the RCV-only message-queue call. M44
// added `arg_kinds` rows the other five's new walls needed (`openat_nocancel`/464, `statfs64`/345,
// `getattrlistbulk`/461, `unlink`/10, `rename`/128): `ls` and `ed` now gate un-parked, and the
// remaining five are re-parked at the wall each now measurably reaches (none is still the M38 RCV
// receive — that call is refused and the run continues past it on every one of them). M45
// emulated `kevent_qos`(374)'s workqueue-kqueue init, which moved `automationmodetool` past it to
// the next call libdispatch makes, a second `kevent_qos` shape, where it is re-parked with that
// shape measured and not modelled (M45 §7 Halt 3).
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
#[ignore = "M37 wall, class C (new subsystem: process creation), parked, not routed. /bin/csh: `dup2` was the M36 wall and is modelled (M37 t2) — the four calls `dup2(0,16)`, `(1,17)`, `(2,18)`, `(16,19)`, each followed by `fcntl(new, F_SETFD, 1)`, now record and succeed (run N landmarks #263/#265/#267/#269); `pipe`'s pair is bound since M38 t1, so the guest now receives guest fds `(4, 5)` from it (M38 sweep, recpid 87861: landmark #327 `ret=4 ret1=5 err=false`, where M37 saw the raw host read-end `0x17` and a stale `x1`), moves each end above the C shell's `FSAFE` with `dup`/`close` (#328–#334: `dup(4)`→6, `close(4)`, `dup(5)`→4, `dup(4)`→7, `close(4)`, `close(5)`), and the `fcntl(F_SETFD, 1)` on each moved end now succeeds (#330 on fd 6 → 0, #335 on fd 7 → 0, where M37 had two `EBADF`s), the 3403 stop following at #336; a traced run the same day (recpid 91162) has the identical shape at #326/#329/#334, stop at 335; the row now stops ~60 landmarks later at `record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 3403 dest 0x203 (guest task port Some(515)) send_size 64`, rc/rp 4/3 — `mach_ports_register` (task.defs 3400+3, a complex message the router does not know) from libxpc `xpc_atfork_prepare` ← `libSystem_atfork_prepare` ← `fork` (spec §2a's backtrace); behind it `fork`(2) itself, which has no row. Identical in runs N/I/S (recpids 1005/17385/66385, non-colliding / [0x4000,0x10000) / [0x10000,0x18000); landmarks 331/330/334; 0 self-pid ESRCH in every kept trace). Evidence docs/sweep-evidence/2026-09-13-m37/csh.{N,I,S}.{rec,rp}.err. UN-IGNORE when the box models process creation."]
fn csh_records_and_replays() { records_and_replays_clean("/bin/csh"); }

#[test]
#[ignore = "M37 wall, class C (new subsystem: process creation), parked, not routed. /bin/tcsh: `dup2` was the M36 wall and is modelled (M37 t2) — the four calls `dup2(0,16)`, `(1,17)`, `(2,18)`, `(16,19)`, each followed by `fcntl(new, F_SETFD, 1)`, now record and succeed (run N landmarks #261/#263/#265/#267); `pipe`'s pair is bound since M38 t1, so the guest now receives guest fds `(4, 5)` from it (M38 sweep, recpid 89125: landmark #335 `ret=4 ret1=5 err=false`, where M37 saw the raw host read-end `0x17` and a stale `x1`), moves each end above the C shell's `FSAFE` with `dup`/`close` (#336–#342, the same six calls as csh), and the `fcntl(F_SETFD, 1)` on each moved end now succeeds (#338 on fd 6 → 0, #343 on fd 7 → 0, where M37 had two `EBADF`s), the 3403 stop following at #344; a traced run the same day (recpid 91174) has the identical shape at #328/#331/#336, stop at 337; the row now stops ~60 landmarks later at `record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 3403 dest 0x203 (guest task port Some(515)) send_size 64`, rc/rp 4/3 — `mach_ports_register` (task.defs 3400+3, a complex message the router does not know) from libxpc `xpc_atfork_prepare` ← `libSystem_atfork_prepare` ← `fork` (spec §2a's backtrace); behind it `fork`(2) itself, which has no row. Identical in runs N/I/S (recpids 2342/18983/67896, non-colliding / [0x4000,0x10000) / [0x10000,0x18000); landmarks 329/332/332; 0 self-pid ESRCH in every kept trace). Evidence docs/sweep-evidence/2026-09-13-m37/tcsh.{N,I,S}.{rec,rp}.err. UN-IGNORE when the box models process creation."]
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
#[ignore = "M45 wall, class C (new subsystem: libdispatch's kevent source registration through the workqueue kqueue, a second `kevent_qos` shape), parked, routed to its own milestone (M45 §7 Halt 3). /usr/bin/automationmodetool: `kevent_qos`(374)'s workqueue-kqueue init is emulated since M45 and records at landmark 362 (rc 0, no writes, thread 0). The run now continues to a second `kevent_qos`(374) at pc 0x1804afa48, landmark 363, rc/rp 101/n/a (no replay ran): `thread 'main' (8167022) panicked at crates/retrace-box/src/lib.rs:5222:13: M45: unmeasured kevent_qos shape: x3 (eventlist) is 0x27fedb8, measured 0x0. …`. Its arguments are `x0` (kq) = `-1`, one change, a 16-entry event list (`x3` = `0x27fedb8`, `x4` = `0x10`), and `x7` flags = `0x23` = `KEVENT_FLAG_WORKQ|KEVENT_FLAG_ERROR_EVENTS|KEVENT_FLAG_IMMEDIATE`. Its change entry, read by the debugger at the svc (pc 0x1804afa44, position (363, 1841), thread 0), is filter `0xfff2` (-14, which the public SDK's `sys/event.h` does not define), flags `0x0185` = `EV_ADD|EV_ENABLE|EV_DISPATCH|EV_UDATA_SPECIFIC`, qos `0x02000000`, udata `0x6c850`, fflags `0xf0000037`, and every other field zero. By those values (inferred, not symbolicated) this is libdispatch registering its own memory-pressure source right after the init, and it is not automationmodetool's own call: both t0 M2 GCD candidates (`timer`, `after`) stop at the same shape, with only udata and the stack addresses differing. Landmark numbers move with the host's `gettimeofday`(116) count (t0 M1). Evidence docs/sweep-evidence/2026-09-28-m45/automationmodetool.{rec.err,entry.txt} and landmarks.txt. UN-IGNORE when this second shape (a `KEVENT_FLAG_WORKQ|KEVENT_FLAG_ERROR_EVENTS` registration of one filter -14 `EV_DISPATCH|EV_UDATA_SPECIFIC` source, with an event list) is measured and modelled, and whatever the run reaches next is cleared."]
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
#[ignore = "M44 wall, class C (new subsystem: the I/O Kit main port), parked, not routed. /usr/bin/dddiagnose: `statfs64`(345) records (landmark 433, `ret=2`/ENOENT — the path it stats does not exist, harmlessly); the row now stops 21 landmarks later at `RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 205 dest 0x1c03 (guest task port Some(515)) send_size 24` — msgh_id 205 = `host_get_io_main` (SDK `mach/mach_host.h:1313`, `{ \"host_get_io_main\", 205 }`), rc/rp 4/3, landmark 454 (replay's `DIVERGENCE at landmark 454 pc=0x1804adc34: expected recorded syscall, got None` is the trace with no terminal event the record error left, not a divergence of its own). Evidence docs/sweep-evidence/2026-09-27-m44-t0/t4/dddiagnose.{rec,rp}.err. UN-IGNORE when `host_get_io_main` (msgh_id 205) is serviced."]
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
