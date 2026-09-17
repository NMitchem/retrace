// M36 parked a gate per measured wall in the Apple sweep; M37 moved each to the wall it measured;
// M38 un-parked `launchctl` and moved the other five past the RCV-only message-queue call.
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
        eprintln!("SKIPPED: {path} is not present on this machine");
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
#[ignore = "M37 wall, class C (new subsystem: process creation), parked, not routed. /bin/csh: `dup2` was the M36 wall and is modelled (M37 t2) — the four calls `dup2(0,16)`, `(1,17)`, `(2,18)`, `(16,19)`, each followed by `fcntl(new, F_SETFD, 1)`, now record and succeed (run N landmarks #263/#265/#267/#269); the row now stops ~60 landmarks later at `record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 3403 dest 0x203 (guest task port Some(515)) send_size 64`, rc/rp 4/3 — `mach_ports_register` (task.defs 3400+3, a complex message the router does not know) from libxpc `xpc_atfork_prepare` ← `libSystem_atfork_prepare` ← `fork` (spec §2a's backtrace); behind it `fork`(2) itself, which has no row. Identical in runs N/I/S (recpids 1005/17385/66385, non-colliding / [0x4000,0x10000) / [0x10000,0x18000); landmarks 331/330/334; 0 self-pid ESRCH in every kept trace). Evidence docs/sweep-evidence/2026-09-13-m37/csh.{N,I,S}.{rec,rp}.err. UN-IGNORE when the box models process creation."]
fn csh_records_and_replays() { records_and_replays_clean("/bin/csh"); }

#[test]
#[ignore = "M37 wall, class C (new subsystem: process creation), parked, not routed. /bin/tcsh: `dup2` was the M36 wall and is modelled (M37 t2) — the four calls `dup2(0,16)`, `(1,17)`, `(2,18)`, `(16,19)`, each followed by `fcntl(new, F_SETFD, 1)`, now record and succeed (run N landmarks #261/#263/#265/#267); the row now stops ~60 landmarks later at `record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 3403 dest 0x203 (guest task port Some(515)) send_size 64`, rc/rp 4/3 — `mach_ports_register` (task.defs 3400+3, a complex message the router does not know) from libxpc `xpc_atfork_prepare` ← `libSystem_atfork_prepare` ← `fork` (spec §2a's backtrace); behind it `fork`(2) itself, which has no row. Identical in runs N/I/S (recpids 2342/18983/67896, non-colliding / [0x4000,0x10000) / [0x10000,0x18000); landmarks 329/332/332; 0 self-pid ESRCH in every kept trace). Evidence docs/sweep-evidence/2026-09-13-m37/tcsh.{N,I,S}.{rec,rp}.err. UN-IGNORE when the box models process creation."]
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
        eprintln!("SKIPPED: {path} is not present on this machine");
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
#[ignore = "M38 wall, class B (known-unmodelled: an M33 fail-loud that an `arg_kinds` row closes), parked, not routed. /usr/bin/automationmodetool: the RCV-only message-queue call is refused (M38 t5, MACH_RCV_REFUSAL = MACH_RCV_INVALID_NAME) and records; the receive is landmark #343 and the row now stops 20 landmarks later at `thread 'main' panicked at crates/retrace-arch/src/lib.rs:944:38: M33: syscall 374 (374) has no arg_kinds row in crates/retrace-arch/src/lib.rs — it cannot be forwarded unclassified (an untranslated guest fd would act on retrace's own descriptor of that number). …` — `kevent_qos`(2) at pc 0x1804afa48, a guest syscall the box has never classified, rc/rp 101/3 (replay's `DIVERGENCE at landmark 363 pc=0x1804afa48: expected recorded syscall, got None` is the truncated trace the panic left, not a divergence of its own), landmark 363, run N (recpid 55162; one regime — §4b is retired, M37; measured on this trace: 0 self-pid ESRCH, every self-pid csops/proc_info succeeds). The same kevent_qos(2) wall under all three candidate codes (TIMED_OUT/INVALID_NAME/PORT_DIED divergence landmarks 361/363/358). Evidence docs/sweep-evidence/2026-09-16-m38/automationmodetool.MACH_RCV_INVALID_NAME.{rec,rp}.err. UN-IGNORE when `kevent_qos` (374) has an `arg_kinds` row and the row records past it — a row alone is unmeasured; libdispatch's kevent workloop may be a subsystem of its own behind it."]
fn automationmodetool_records_and_replays() { records_and_replays_clean("/usr/bin/automationmodetool"); }

#[test]
#[ignore = "M38 wall, class B (known-unmodelled: an M33 fail-loud that an `arg_kinds` row closes), parked, not routed. /usr/bin/desdp: the RCV-only message-queue call is refused (M38 t5, MACH_RCV_REFUSAL = MACH_RCV_INVALID_NAME) and records; the receive is landmark #363 and the row now stops 29 landmarks later at `thread 'main' panicked at crates/retrace-arch/src/lib.rs:944:38: M33: syscall 464 (464) has no arg_kinds row in crates/retrace-arch/src/lib.rs — it cannot be forwarded unclassified (an untranslated guest fd would act on retrace's own descriptor of that number). …` — `openat_nocancel`(2) at pc 0x1804b3954, a guest syscall the box has never classified, rc/rp 101/3 (replay's `DIVERGENCE at landmark 392 pc=0x1804b3954: expected recorded syscall, got None` is the truncated trace the panic left, not a divergence of its own), landmark 392, run N (recpid 55174; one regime — §4b is retired, M37; measured on this trace: 0 self-pid ESRCH, every self-pid csops/proc_info succeeds). The same openat_nocancel(2) wall under all three candidate codes (TIMED_OUT/INVALID_NAME/PORT_DIED divergence landmarks 398/392/392). NOTE: these three binaries are the `xcrun` trampoline (desdp/dyld_info/flex are hardlinks to one Xcode stub; it opens a random-named /var/tmp/xcrun_db-XXXXXX), so the intervening path shifts run-to-run — the WALL syscall number is stable across both the measurement and the gate run, the guest's own tempfile nondeterminism is not retrace's. Evidence docs/sweep-evidence/2026-09-16-m38/desdp.MACH_RCV_INVALID_NAME.{rec,rp}.err. UN-IGNORE when `openat_nocancel` (464) has an `arg_kinds` row and the row records past it — a row alone is unmeasured."]
fn desdp_records_and_replays() { records_and_replays_clean("/usr/bin/desdp"); }

#[test]
#[ignore = "M38 wall, class B (known-unmodelled: an M33 fail-loud that an `arg_kinds` row closes), parked, not routed. /usr/bin/dyld_info: the RCV-only message-queue call is refused (M38 t5, MACH_RCV_REFUSAL = MACH_RCV_INVALID_NAME) and records; the receive is landmark #365 and the row now stops 28 landmarks later at `thread 'main' panicked at crates/retrace-arch/src/lib.rs:944:38: M33: syscall 464 (464) has no arg_kinds row in crates/retrace-arch/src/lib.rs — it cannot be forwarded unclassified (an untranslated guest fd would act on retrace's own descriptor of that number). …` — `openat_nocancel`(2) at pc 0x1804b3954, a guest syscall the box has never classified, rc/rp 101/3 (replay's `DIVERGENCE at landmark 393 pc=0x1804b3954: expected recorded syscall, got None` is the truncated trace the panic left, not a divergence of its own), landmark 393, run N (recpid 55190; one regime — §4b is retired, M37; measured on this trace: 0 self-pid ESRCH, every self-pid csops/proc_info succeeds). The same openat_nocancel(2) wall under all three candidate codes (TIMED_OUT/INVALID_NAME/PORT_DIED divergence landmarks 392/393/395). NOTE: these three binaries are the `xcrun` trampoline (desdp/dyld_info/flex are hardlinks to one Xcode stub; it opens a random-named /var/tmp/xcrun_db-XXXXXX), so the intervening path shifts run-to-run — the WALL syscall number is stable across both the measurement and the gate run, the guest's own tempfile nondeterminism is not retrace's. Evidence docs/sweep-evidence/2026-09-16-m38/dyld_info.MACH_RCV_INVALID_NAME.{rec,rp}.err. UN-IGNORE when `openat_nocancel` (464) has an `arg_kinds` row and the row records past it — a row alone is unmeasured."]
fn dyld_info_records_and_replays() { records_and_replays_clean("/usr/bin/dyld_info"); }

#[test]
#[ignore = "M38 wall, class B (known-unmodelled: an M33 fail-loud that an `arg_kinds` row closes), parked, not routed. /usr/bin/flex: the RCV-only message-queue call is refused (M38 t5, MACH_RCV_REFUSAL = MACH_RCV_INVALID_NAME) and records; the receive is landmark #369 and the row now stops 29 landmarks later at `thread 'main' panicked at crates/retrace-arch/src/lib.rs:944:38: M33: syscall 464 (464) has no arg_kinds row in crates/retrace-arch/src/lib.rs — it cannot be forwarded unclassified (an untranslated guest fd would act on retrace's own descriptor of that number). …` — `openat_nocancel`(2) at pc 0x1804b3954, a guest syscall the box has never classified, rc/rp 101/3 (replay's `DIVERGENCE at landmark 398 pc=0x1804b3954: expected recorded syscall, got None` is the truncated trace the panic left, not a divergence of its own), landmark 398, run N (recpid 55202; one regime — §4b is retired, M37; measured on this trace: 0 self-pid ESRCH, every self-pid csops/proc_info succeeds). The same openat_nocancel(2) wall under all three candidate codes (TIMED_OUT/INVALID_NAME/PORT_DIED divergence landmarks 398/398/396). NOTE: these three binaries are the `xcrun` trampoline (desdp/dyld_info/flex are hardlinks to one Xcode stub; it opens a random-named /var/tmp/xcrun_db-XXXXXX), so the intervening path shifts run-to-run — the WALL syscall number is stable across both the measurement and the gate run, the guest's own tempfile nondeterminism is not retrace's. Evidence docs/sweep-evidence/2026-09-16-m38/flex.MACH_RCV_INVALID_NAME.{rec,rp}.err. UN-IGNORE when `openat_nocancel` (464) has an `arg_kinds` row and the row records past it — a row alone is unmeasured."]
fn flex_records_and_replays() { records_and_replays_clean("/usr/bin/flex"); }

#[test]
#[ignore = "M38 wall, class B (known-unmodelled: an M33 fail-loud that an `arg_kinds` row closes), parked, not routed. /usr/bin/dddiagnose: the RCV-only message-queue call is refused (M38 t5, MACH_RCV_REFUSAL = MACH_RCV_INVALID_NAME — the code THIS binary chose the sweep on: under MACH_RCV_TIMED_OUT it crashes in the guest 10 landmarks after the receive (#379), `guest crashed: pc=0x180302eb0 far=0x2000050050 esr=0x92000045` at landmark 389, and under MACH_RCV_PORT_DIED, 10 after the receive (#382), `guest crashed: pc=0x193bbbca0 far=0xfffffffffffffff0 esr=0x92000004` at landmark 392, rc/rp 139/139 both, recorded and replayed identically; evidence dddiagnose.MACH_RCV_{TIMED_OUT,PORT_DIED}.{rec,rp}.err beside the file below) and records; the receive is landmark #381 and the row now stops 50 landmarks later at `thread 'main' panicked at crates/retrace-arch/src/lib.rs:944:38: M33: syscall 345 (345) has no arg_kinds row in crates/retrace-arch/src/lib.rs — it cannot be forwarded unclassified (an untranslated guest fd would act on retrace's own descriptor of that number). …` — `statfs64`(2) at pc 0x1804bd0cc, a guest syscall the box has never classified, rc/rp 101/3 (replay's `DIVERGENCE at landmark 431 pc=0x1804bd0cc: expected recorded syscall, got None` is the truncated trace the panic left, not a divergence of its own), landmark 431, run N (recpid 55214; one regime — §4b is retired, M37; measured on this trace: 0 self-pid ESRCH, every self-pid csops/proc_info succeeds). Evidence docs/sweep-evidence/2026-09-16-m38/dddiagnose.MACH_RCV_INVALID_NAME.{rec,rp}.err. UN-IGNORE when `statfs64` (345) has an `arg_kinds` row and the row records past it — a row alone is unmeasured."]
fn dddiagnose_records_and_replays() { records_and_replays_clean("/usr/bin/dddiagnose"); }
