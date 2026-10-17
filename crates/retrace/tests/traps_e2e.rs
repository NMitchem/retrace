//! M51 §3b: the trap classes native defines are recorded as the crashes native reports, with
//! native's signal, si_code, si_addr and exit code, and a handler the program installed runs as it
//! does natively. A trap native does not define stays a wall (R6).
mod util;
use retrace_core::{Advance, Armed, Outcome};
use retrace_trace::{Event, Reader};
use std::path::{Path, PathBuf};

const BOUND: u64 = 120;
const P: &str = retrace_guest::FID_TRAPS;

/// Each converted mode: the exception class its `Crash` carries, and native's exit code (static
/// `native-traps.txt`).
const CLASSES: &[(&str, u64, i32)] = &[("brk", 0x3c, 133), ("udf", 0x00, 132), ("udfword", 0x00, 132),
    ("hlt", 0x00, 132), ("pcalign", 0x22, 138), ("spalign", 0x26, 138)];

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("retrace-traps-{}-{name}", std::process::id()))
}

fn record(mode: &str, extra: &[&str], t: &Path) -> util::Bounded {
    let mut argv = vec!["record-dyn", P, "-o", t.to_str().unwrap(), "--", mode];
    argv.extend_from_slice(extra);
    util::run_bounded(util::bin(), &argv, &[], BOUND)
}

/// The recording's terminal `Crash` as `(pc, esr, far, sig)`, and the function its pc is in, by the
/// recording's own symbols.
fn crash_of(t: &Path) -> (u64, u64, u64, u64, String) {
    let (events, torn) = Reader::open_checked(t).unwrap();
    assert!(!torn, "a complete recording");
    let Event::Snapshot { mem, .. } = &events[events.len() - 1] else { panic!("the final snapshot last") };
    match &events[events.len() - 2] {
        Event::Crash { pc, esr, far, sig, .. } => {
            let f = retrace_core::symbols::Symbols::from_snapshot(mem).resolve(*pc).map_or(String::new(), |(n, _)| n.to_owned());
            (*pc, *esr, *far, *sig, f)
        }
        other => panic!("the second-to-last event must be the Crash, got {other:?}"),
    }
}

/// Spec §4 "Traps": each class, with no handler, is the crash native reports: the same exit code and
/// output, a `Crash` whose class and signal are native's, at a pc in the trapping function, and a FAR
/// of 0 where it is architecturally UNKNOWN (C7). Replayed twice.
#[test]
fn each_trap_without_a_handler_is_the_crash_native_reports() {
    for &(mode, ec, code) in CLASSES {
        let native = util::run_bounded(P, &[mode], &[], BOUND);
        assert_eq!((native.code, native.stdout.as_slice()), (Some(code), &b"start\n"[..]), "{mode}: native");
        let t = scratch(&format!("{mode}.bin"));
        let rec = record(mode, &[], &t);
        assert_eq!((rec.code, rec.stdout.as_slice()), (native.code, native.stdout.as_slice()), "{mode}: record. stderr:\n{}", rec.stderr);
        let (pc, esr, far, sig, f) = crash_of(&t);
        assert_eq!(((esr >> 26) & 0x3f, sig as i32 + 128), (ec, code), "{mode}: the Crash's class and signal");
        assert_eq!(f, format!("_fid_{mode}"), "{mode}: the pc is in the trapping function");
        assert_eq!(far, if ec == 0x22 { pc } else { 0 }, "{mode}: FAR");
        for i in 1..=2 {
            let rp = util::run_bounded(util::bin(), &["replay", t.to_str().unwrap()], &[], BOUND);
            assert_eq!((rp.code, rp.stdout.as_slice()), (rec.code, rec.stdout.as_slice()), "{mode}: replay {i}. stderr:\n{}", rp.stderr);
        }
        let _ = std::fs::remove_file(&t);
    }
}

/// Spec §4 "Traps": with an `SA_SIGINFO` handler, each class delivers what native delivers. The
/// handler's line pins the signal, si_code, si_addr and FAR relative to the pc, and the frame's ESR,
/// byte for byte against native's.
#[test]
fn each_trap_with_a_handler_delivers_what_native_delivers() {
    for &(mode, _, code) in CLASSES {
        let native = util::run_bounded(P, &[mode, "h"], &[], BOUND);
        assert_eq!(native.code, Some(42), "{mode} h: native: {}", String::from_utf8_lossy(&native.stdout));
        let t = scratch(&format!("{mode}-h.bin"));
        let rec = record(mode, &["h"], &t);
        assert_eq!((rec.code, String::from_utf8_lossy(&rec.stdout)), (Some(42), String::from_utf8_lossy(&native.stdout)),
            "{mode} h: record. stderr:\n{}", rec.stderr);
        let (events, _) = Reader::open_checked(&t).unwrap();
        assert!(events.iter().any(|e| matches!(e, Event::SignalDelivery { sig, .. } if *sig as i32 + 128 == code)),
            "{mode} h: the delivery is of the signal native kills with");
        for i in 1..=2 {
            let rp = util::run_bounded(util::bin(), &["replay", t.to_str().unwrap()], &[], BOUND);
            assert_eq!((rp.code, rp.stdout.as_slice()), (rec.code, rec.stdout.as_slice()), "{mode} h: replay {i}. stderr:\n{}", rp.stderr);
        }
        let _ = std::fs::remove_file(&t);
    }
}

/// Review Focus 5: the debugger reaches the `brk`'s crash, steps back onto the `mov` before it, and
/// steps forward onto the `brk` again. The step loops convert through the code `run()` does.
#[test]
fn the_debugger_steps_back_across_a_converted_brk() {
    let t = scratch("dbg.bin");
    let rec = record("brk", &[], &t);
    assert_eq!(rec.code, Some(133), "{}", rec.stderr);
    let (pc, ..) = crash_of(&t);
    let (code, out, err) = util::debug_bounded(t.to_str().unwrap(), "continue; reverse-stepi; where; stepi; where", BOUND);
    assert_eq!(code, Some(0), "debug: {err}\n{out}");
    assert!(out.contains(&format!("guest crashed: pc={pc:#x} far=0x0 ")), "continue parks at the brk:\n{out}");
    let back = out.find(&format!("pc={:#x} thread=", pc - 4))
        .unwrap_or_else(|| panic!("reverse-stepi reaches the mov before the brk:\n{out}"));
    assert!(out[back..].contains(&format!("pc={pc:#x} thread=")), "stepi reaches the brk again:\n{out}");
    let _ = std::fs::remove_file(&t);
}

/// A session seeked onto the `brk` reports the crash at its pc from each step loop, and the session
/// parked by an armed step ends in the recorded crash (nullfault_e2e's converted-fault test, for a
/// trap).
#[test]
fn a_seek_onto_a_converted_brk_reports_the_crash_at_its_pc() {
    let t = scratch("seek.bin");
    let rec = record("brk", &[], &t);
    assert_eq!(rec.code, Some(133), "{}", rec.stderr);
    let ci = Reader::open(&t).unwrap().len() - 2;
    let (pc, ..) = crash_of(&t);
    let n = retrace_core::seek(&t, ci, 0).unwrap().window_len_here().expect("the crash window ends at the brk");
    let mut s = retrace_core::seek(&t, ci, n).unwrap();
    assert_eq!(s.pc(), pc, "({ci}, {n}) is parked on the brk");
    assert_eq!(s.step_insns(1), Err(format!("guest crashed at step 0/1: pc={pc:#x} far=0x0")));
    drop(s); // one VM per process: the first session must go before the second opens
    let mut s = retrace_core::seek(&t, ci, n).unwrap();
    assert_eq!(s.step_armed(), Ok(Armed::Fault));
    match s.advance().unwrap_or_else(|d| panic!("after the step: diverged at {}: {}", d.landmark, d.detail)) {
        Advance::Exited(r) => assert!(matches!(r.outcome, Outcome::Crash { pc: p, sig: 5, .. } if p == pc), "{:?}", r.outcome),
        _ => panic!("the session parked on the brk must end in the recorded crash"),
    }
    let _ = std::fs::remove_file(&t);
}

/// A trap the EL1 vector takes (an undefined word, a misaligned SP), unlike the `brk` that goes
/// straight to EL2, reaches the stepped `hvc` of `run_one_for_step`'s own trampoline arm. A step onto
/// each reports the crash at its pc, with FAR 0.
#[test]
fn a_step_onto_a_trap_the_vector_takes_reports_the_crash_at_its_pc() {
    for (mode, sig) in [("udf", 4), ("spalign", 10)] {
        let t = scratch(&format!("seek-{mode}.bin"));
        let rec = record(mode, &[], &t);
        assert_eq!(rec.code, Some(128 + sig), "{mode}: {}", rec.stderr);
        let ci = Reader::open(&t).unwrap().len() - 2;
        let (pc, _, far, ..) = crash_of(&t);
        assert_eq!(far, 0, "{mode}: FAR");
        let n = retrace_core::seek(&t, ci, 0).unwrap().window_len_here().expect("the crash window ends at the trap");
        let mut s = retrace_core::seek(&t, ci, n).unwrap();
        assert_eq!(s.pc(), pc, "{mode}: ({ci}, {n}) is parked on the trapping instruction");
        assert_eq!(s.step_insns(1), Err(format!("guest crashed at step 0/1: pc={pc:#x} far=0x0")), "{mode}");
        drop(s); // one VM per process
        let _ = std::fs::remove_file(&t);
    }
}

/// A wall (R6, §3b): native runs the mode to `survived`; retrace stops at one line in `exceptions`
/// naming `what`, and exits 4.
fn assert_still_a_wall(mode: &str, what: &str) {
    let native = util::run_bounded(P, &[mode], &[], BOUND);
    assert_eq!((native.code, native.stdout.as_slice()), (Some(0), &b"start\nsurvived\n"[..]), "{mode}: native");
    let t = scratch(&format!("{mode}.bin"));
    let rec = record(mode, &[], &t);
    let lines: Vec<&str> = rec.stderr.lines().filter(|l| l.starts_with("retrace:")).collect();
    assert_eq!((rec.code, lines.len()), (Some(4), 1), "{mode}: one wall line, exit 4. stderr:\n{}", rec.stderr);
    assert!(lines[0].starts_with("retrace: unsupported: ") && lines[0].contains(what) && lines[0].contains("#exceptions"),
        "{mode}: {}", lines[0]);
    let _ = std::fs::remove_file(&t);
}

/// R6: an Apple implementation-defined instruction (AMX) native runs stays a wall, rather than a
/// SIGILL native never gives. It arrives as EC 0x3f (t0 M2, T0-b).
#[test]
fn an_amx_instruction_is_still_a_wall() {
    assert_still_a_wall("amx", "an implementation-defined instruction (such as Apple's AMX) at pc 0x");
}

/// §3b: a system register HVF traps and native allows (t0 M2, `cntfrq_el0`) stays a wall.
#[test]
fn a_trapped_system_register_is_still_a_wall() {
    assert_still_a_wall("cntfrq", "a trapped system register access at pc 0x");
}

/// Spec §3c: the closing line says which trap, and in which function.
#[test]
fn the_closing_line_names_each_trap_and_its_function() {
    for (mode, sig, what) in [("brk", "SIGTRAP", "brk at pc 0x"), ("udf", "SIGILL", "undefined instruction at pc 0x"),
                              ("pcalign", "SIGBUS", "misaligned pc 0x"), ("spalign", "SIGBUS", "misaligned stack pointer at pc 0x")] {
        let t = scratch(&format!("line-{mode}.bin"));
        let rec = record(mode, &[], &t);
        let lines: Vec<&str> = rec.stderr.lines().filter(|l| l.starts_with("retrace:")).collect();
        assert_eq!(lines.len(), 1, "{mode}: {}", rec.stderr);
        assert!(lines[0].starts_with(&format!("retrace: the program was killed by {sig} ({what}"))
            && lines[0].contains(&format!(" in _fid_{mode}")), "{mode}: {}", lines[0]);
        let _ = std::fs::remove_file(&t);
    }
}
