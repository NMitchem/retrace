// M36 parked a gate per measured wall in the Apple sweep; M37 moved each to the wall it measured.
// Every test here is `#[ignore]`d ON PURPOSE, and each reason is the measurement that parks it —
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

#[test]
#[ignore = "M37 wall, class C, parked, not routed. /bin/launchctl: the B half (M34 §4b) is retired: with `Scalar` positions never probed, runs N/I/S (recpids 1601/18162/66837, non-colliding / [0x4000,0x10000) / [0x10000,0x18000) — the trampoline page and the guest's os_alloc_once slab, the two regimes whose M36 face was the libdispatch `brk`) all stop at `record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape`, rc/rp 4/3 (`mach_msg2_trap+8`, `Route::Unsupported`, the RCV-shaped message-queue call), landmark 338/330/331, 0 self-pid ESRCH in every kept trace (12 landmarks carry the recorder's pid and all succeed). Evidence docs/sweep-evidence/2026-09-13-m37/launchctl.{N,I,S}.{rec,rp}.err. UN-IGNORE when the box services the RCV-shaped message-queue call."]
fn launchctl_records_and_replays() { records_and_replays_clean("/bin/launchctl"); }

#[test]
#[ignore = "M37 wall, class C, parked, not routed. /usr/bin/automationmodetool: the B half (M34 §4b) is retired: with `Scalar` positions never probed, runs N/I/S (recpids 2593/19193/68104, non-colliding / [0x4000,0x10000) / [0x10000,0x18000) — the trampoline page and the guest's os_alloc_once slab, the two regimes whose M36 face was the libdispatch `brk`) all stop at `record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape`, rc/rp 4/3 (`mach_msg2_trap+8`, `Route::Unsupported`, the RCV-shaped message-queue call), landmark 343/340/339, 0 self-pid ESRCH in every kept trace (12 landmarks carry the recorder's pid and all succeed). Evidence docs/sweep-evidence/2026-09-13-m37/automationmodetool.{N,I,S}.{rec,rp}.err. UN-IGNORE when the box services the RCV-shaped message-queue call."]
fn automationmodetool_records_and_replays() { records_and_replays_clean("/usr/bin/automationmodetool"); }

#[test]
#[ignore = "M37 wall, class C, parked, not routed. /usr/bin/desdp: the B half (M34 §4b) is retired: with `Scalar` positions never probed, runs N/I/S (recpids 2626/19226/68136, non-colliding / [0x4000,0x10000) / [0x10000,0x18000) — the trampoline page and the guest's os_alloc_once slab, the two regimes whose M36 face was the libdispatch `brk`) all stop at `record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape`, rc/rp 4/3 (`mach_msg2_trap+8`, `Route::Unsupported`, the RCV-shaped message-queue call), landmark 366/366/365, 0 self-pid ESRCH in every kept trace (12 landmarks carry the recorder's pid and all succeed). Evidence docs/sweep-evidence/2026-09-13-m37/desdp.{N,I,S}.{rec,rp}.err. UN-IGNORE when the box services the RCV-shaped message-queue call."]
fn desdp_records_and_replays() { records_and_replays_clean("/usr/bin/desdp"); }

#[test]
#[ignore = "M37 wall, class C, parked, not routed. /usr/bin/dyld_info: the B half (M34 §4b) is retired: with `Scalar` positions never probed, runs N/I/S (recpids 2655/19255/68167, non-colliding / [0x4000,0x10000) / [0x10000,0x18000) — the trampoline page and the guest's os_alloc_once slab, the two regimes whose M36 face was the libdispatch `brk`) all stop at `record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape`, rc/rp 4/3 (`mach_msg2_trap+8`, `Route::Unsupported`, the RCV-shaped message-queue call), landmark 365/367/366, 0 self-pid ESRCH in every kept trace (12 landmarks carry the recorder's pid and all succeed). Evidence docs/sweep-evidence/2026-09-13-m37/dyld_info.{N,I,S}.{rec,rp}.err. UN-IGNORE when the box services the RCV-shaped message-queue call."]
fn dyld_info_records_and_replays() { records_and_replays_clean("/usr/bin/dyld_info"); }

#[test]
#[ignore = "M37 wall, class C, parked, not routed. /usr/bin/flex: the B half (M34 §4b) is retired: with `Scalar` positions never probed, runs N/I/S (recpids 2688/19285/68197, non-colliding / [0x4000,0x10000) / [0x10000,0x18000) — the trampoline page and the guest's os_alloc_once slab, the two regimes whose M36 face was the libdispatch `brk`) all stop at `record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape`, rc/rp 4/3 (`mach_msg2_trap+8`, `Route::Unsupported`, the RCV-shaped message-queue call), landmark 362/369/366, 0 self-pid ESRCH in every kept trace (12 landmarks carry the recorder's pid and all succeed). Evidence docs/sweep-evidence/2026-09-13-m37/flex.{N,I,S}.{rec,rp}.err. UN-IGNORE when the box services the RCV-shaped message-queue call."]
fn flex_records_and_replays() { records_and_replays_clean("/usr/bin/flex"); }

#[test]
#[ignore = "M37 wall, class C, parked, not routed. /usr/bin/dddiagnose: the B half (M34 §4b) is retired: with `Scalar` positions never probed, runs N/I/S (recpids 2718/19315/68226, non-colliding / [0x4000,0x10000) / [0x10000,0x18000) — the trampoline page and the guest's os_alloc_once slab, the two regimes whose M36 face was the libdispatch `brk`) all stop at `record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape`, rc/rp 4/3 (`mach_msg2_trap+8`, `Route::Unsupported`, the RCV-shaped message-queue call), landmark 383/380/384, 0 self-pid ESRCH in every kept trace (13 landmarks carry the recorder's pid and all succeed). Evidence docs/sweep-evidence/2026-09-13-m37/dddiagnose.{N,I,S}.{rec,rp}.err. UN-IGNORE when the box services the RCV-shaped message-queue call."]
fn dddiagnose_records_and_replays() { records_and_replays_clean("/usr/bin/dddiagnose"); }
