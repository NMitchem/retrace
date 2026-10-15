//! M49 Task 3 (spec §3d F2–F4, §4): NULL, low, unmapped, wild and overflowing accesses through
//! the real record loop, each against the same fixture run natively. `faultcase_dyn <case> [h|a]`
//! (its source's header lists the cases). With no handler native dies of SIGSEGV (139), and a
//! recording must end in `Event::Crash`. With `h` (or `a`, on an alternate stack) native's
//! SA_SIGINFO handler prints one line and `_exit(42)`s, and a recording must deliver the same
//! signal, `si_code` and `si_addr`. A recorder error also exits nonzero, so every assertion is on
//! the trace or on the guest's own line, never on an exit code alone (spec §4).
mod util;

use retrace_core::{Advance, Armed, Outcome};
use retrace_guest::FAULTCASE_DYN;
use retrace_trace::{Event, Reader};
use std::os::unix::process::ExitStatusExt;

/// `bin <args…>` run natively: its stdout and its exit code, a fatal signal reported as the shell
/// reports it, 128 + the signal, which is what retrace exits with for a crash (M6).
fn run_native(bin: &str, args: &[&str]) -> (Vec<u8>, i32) {
    let out = std::process::Command::new(bin).args(args).output().unwrap();
    let code = out.status.code().or_else(|| out.status.signal().map(|s| 128 + s)).expect("an exit code or a signal");
    (out.stdout, code)
}

fn native(args: &[&str]) -> (Vec<u8>, i32) { run_native(FAULTCASE_DYN, args) }

/// The main thread's stack guard page, `[GUARD_PAGE_IPA, GUARD_TOP)` in
/// `crates/retrace-box/src/lib.rs` (not exported). A stack overflow's first touch below M21's
/// window lands in it.
const GUARD: std::ops::Range<u64> = 0x0200_4000..0x0200_8000;

/// Where a case's fault must be.
enum Addr {
    /// A fixed address.
    At(u64),
    /// This far into the page the guest mapped and released just before the access. Its address
    /// is ASLR's natively and the box's under retrace, so it is read from the trace: the last
    /// 16 KiB `munmap` before the terminal event is the guest's own.
    Released(u64),
    /// Anywhere in the main stack's guard page.
    Guard,
}

struct Case { name: &'static str, esr: u64, addr: Addr }

/// The cases with no handler of their own test (`thread1g`, `jump1g` and `tsd` have one each).
/// The ESR is retrace's, which differs from native's only in the DFSC level (R2): 0x07 is a
/// level-3 translation fault, bit 6 is WnR, EC 0x24 a data abort and 0x20 an instruction abort.
/// - stage 1 since F3 (page 0 and retrace's table pages are no longer mapped): `null10`,
///   `null0st`, `call0`, `low8000`;
/// - converted by F2 (stage 1 maps the address, nothing backs it): `unmapped`, `low1g`,
///   `jumpgone`, `deeprec`;
/// - stage 1 since M6 (above the 64 GiB identity map, L1 index 0x400, level 1): `garbage`.
const CASES: &[Case] = &[
    Case { name: "null10",   esr: 0x9200_0007, addr: Addr::At(0x10) },
    Case { name: "null0st",  esr: 0x9200_0047, addr: Addr::At(0) },
    Case { name: "call0",    esr: 0x8200_0007, addr: Addr::At(0) },
    Case { name: "low8000",  esr: 0x9200_0047, addr: Addr::At(0x8000) },
    Case { name: "unmapped", esr: 0x9200_0007, addr: Addr::Released(0x8) },
    Case { name: "low1g",    esr: 0x9200_0007, addr: Addr::At(0x4000_0000) },
    Case { name: "jumpgone", esr: 0x8200_0007, addr: Addr::Released(0x10) },
    Case { name: "deeprec",  esr: 0x9200_0047, addr: Addr::Guard },
    Case { name: "garbage",  esr: 0x9200_0005, addr: Addr::At(0x4000_dead_0000) },
];

fn released_page(events: &[Event]) -> u64 {
    events.iter().rev().find_map(|e| match e {
        Event::Syscall { num, args, .. } if *num == retrace_arch::SYS_MUNMAP && args[1] == 0x4000 => Some(args[0]),
        _ => None,
    }).expect("the guest's own munmap is in the trace")
}

fn check_addr(case: &str, addr: &Addr, got: u64, events: &[Event]) {
    match addr {
        Addr::At(a) => assert_eq!(got, *a, "{case}: the fault's address"),
        Addr::Released(off) => assert_eq!(got, released_page(events) + off, "{case}: the released page + {off:#x}"),
        Addr::Guard => assert!(GUARD.contains(&got), "{case}: {got:#x} is not in the guard page {GUARD:x?}"),
    }
}

/// The trace's terminal pair: the `Crash` (its pc, esr, far and thread), then the final snapshot.
fn terminal_crash(case: &str, events: &[Event]) -> (u64, u64, u64, u32) {
    assert!(matches!(events[events.len() - 1], Event::Snapshot { .. }), "{case}: the final snapshot last");
    match &events[events.len() - 2] {
        Event::Crash { pc, esr, far, thread, .. } => (*pc, *esr, *far, *thread),
        other => panic!("{case}: the second-to-last event must be the Crash, got {other:?}"),
    }
}

#[test]
fn each_fault_case_records_a_terminal_crash_and_replays_it_twice() {
    for c in CASES {
        let (want, ncode) = native(&[c.name]);
        assert_eq!(ncode, 139, "{}: native dies of SIGSEGV", c.name);
        let (rec, trace) = util::record_dynamic_args(FAULTCASE_DYN, &[c.name]);
        assert_eq!(rec.code, 139, "{}: a recorded crash, not a recorder error: {}", c.name, rec.stderr);
        assert_eq!(rec.stdout, want, "{}: the guest's output up to the fault", c.name);
        let ev = Reader::open(&trace).unwrap();
        let (pc, esr, far, thread) = terminal_crash(c.name, &ev);
        assert_eq!(esr, c.esr, "{}: ESR {esr:#x}", c.name);
        assert_eq!(thread, 0, "{}: main faulted", c.name);
        check_addr(c.name, &c.addr, far, &ev);
        if esr >> 26 == 0x20 {
            assert_eq!(pc, far, "{}: an instruction abort faults at its own pc", c.name);
        }
        assert!(!ev.iter().any(|e| matches!(e, Event::SignalDelivery { .. })), "{}: no handler, no delivery", c.name);
        for i in 1..=2 {
            let rp = util::replay(&trace);
            assert_eq!(rp.code, 139, "{} replay {i}: {}", c.name, rp.stderr);
            assert_eq!(rp.stdout, rec.stdout, "{} replay {i} stdout", c.name);
            assert!(rp.stderr.contains(&format!("(fault at {far:#x}, pc {pc:#x}")),
                "{} replay {i}: {}", c.name, rp.stderr);
        }
        let _ = std::fs::remove_file(&trace);
    }
}

#[test]
fn each_fault_case_with_a_handler_delivers_natives_signal_code_and_address() {
    // `deeprec` has no alternate stack here, so native's handler never runs; it has two tests below.
    for c in CASES.iter().filter(|c| c.name != "deeprec") {
        let (want, ncode) = native(&[c.name, "h"]);
        assert_eq!(ncode, 42, "{}: native's handler ran", c.name);
        let (rec, trace) = util::record_dynamic_args(FAULTCASE_DYN, &[c.name, "h"]);
        assert_eq!(rec.code, 42, "{}: {}", c.name, rec.stderr);
        // The handler's line: signal, si_code, si_addr (relative where ASLR moves native's), the
        // ESR's class and WnR, and the thread. Byte-identical to native's.
        assert_eq!(rec.stdout, want, "{}: recorded {:?}, native {:?}", c.name,
            String::from_utf8_lossy(&rec.stdout), String::from_utf8_lossy(&want));
        let ev = Reader::open(&trace).unwrap();
        let ds: Vec<_> = ev.iter().filter_map(|e| match e {
            Event::SignalDelivery { sig, si_code, si_addr, writes, thread, .. } => Some((*sig, *si_code, *si_addr, writes, *thread)),
            _ => None,
        }).collect();
        assert_eq!(ds.len(), 1, "{}: exactly one delivery", c.name);
        let (sig, si_code, si_addr, writes, thread) = ds[0];
        assert_eq!((sig, si_code, thread), (11, retrace_arch::SEGV_ACCERR, 0), "{}: SIGSEGV/SEGV_ACCERR to main", c.name);
        check_addr(c.name, &c.addr, si_addr, &ev);
        // The frame the handler read, from the recording (`crates/retrace-box/src/sig.rs`): siginfo
        // at 0 (si_signo +0, si_code +8, si_addr +24), the mcontext at 160 (__far +0, __esr +8).
        assert_eq!(writes.len(), 1, "{}: the frame is one region", c.name);
        let f = &writes[0].bytes;
        let u32_at = |o: usize| u32::from_le_bytes(f[o..o + 4].try_into().unwrap()) as u64;
        let u64_at = |o: usize| u64::from_le_bytes(f[o..o + 8].try_into().unwrap());
        assert_eq!((u32_at(0), u32_at(8), u64_at(24)), (11, 2, si_addr), "{}: siginfo", c.name);
        assert_eq!((u64_at(160), u32_at(168)), (si_addr, c.esr), "{}: the mcontext's __far and __esr", c.name);
        for i in 1..=2 {
            let rp = util::replay(&trace);
            assert_eq!(rp.code, 42, "{} replay {i}: {}", c.name, rp.stderr);
            assert_eq!(rp.stdout, rec.stdout, "{} replay {i} stdout", c.name);
        }
        let _ = std::fs::remove_file(&trace);
    }
}

/// Spec §4's `fc_null_call` guard. Before F3, page 0 was a stage-1 data page (UXN), so a call
/// through NULL took a permission fault, ESR 0x8200000f, which `signal_of_esr` answers SIGBUS.
/// That also dies with no handler, so the signal is read from the handler's own line and the
/// fault's class from the trace.
#[test]
fn a_call_through_a_null_function_pointer_is_sigsegv_not_sigbus() {
    let (want, _) = native(&["call0", "h"]);
    assert!(String::from_utf8_lossy(&want).contains("HANDLER sig=11 si_code=2 si_addr=0 ec=0x20"),
        "native: {:?}", String::from_utf8_lossy(&want));
    let (rec, trace) = util::record_dynamic_args(FAULTCASE_DYN, &["call0", "h"]);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), String::from_utf8_lossy(&want), "{}", rec.stderr);
    let ev = Reader::open(&trace).unwrap();
    assert!(ev.iter().any(|e| matches!(e, Event::SignalDelivery { sig: 11, si_addr: 0, .. })), "a SIGSEGV delivery at 0");
    let _ = std::fs::remove_file(&trace);

    let (rec, trace) = util::record_dynamic_args(FAULTCASE_DYN, &["call0"]);
    assert_eq!(rec.code, 139, "{}", rec.stderr);
    let ev = Reader::open(&trace).unwrap();
    let (pc, esr, far, _) = terminal_crash("call0", &ev);
    assert_eq!((pc, esr, far), (0, 0x8200_0007, 0), "a translation fault at 0, not the permission fault 0x8200000f");
    let _ = std::fs::remove_file(&trace);
}

/// Spec §4's `fc_low_store` guard. Before F3 a store to 0x8000 wrote retrace's own L2 table: the
/// record exited 0, and with a handler replay diverged at an S1PTW abort (`faults-retrace.txt`).
/// Now stage 1 leaves the table page unmapped, so the store faults, native's handler line is
/// printed, and the table holds what it held.
#[test]
fn a_store_to_retraces_page_table_page_faults_and_its_handler_replays() {
    let (want, ncode) = native(&["low8000", "h"]);
    assert_eq!(ncode, 42);
    let (rec, trace) = util::record_dynamic_args(FAULTCASE_DYN, &["low8000", "h"]);
    assert_eq!(rec.code, 42, "{}", rec.stderr);
    assert_eq!(rec.stdout, want);
    let ev = Reader::open(&trace).unwrap();
    assert!(ev.iter().any(|e| matches!(e, Event::SignalDelivery { sig: 11, si_addr: 0x8000, .. })), "a delivery at 0x8000");
    // L2[0], the first word of the table page at 0x8000, is block 0's L3 table descriptor: the
    // same in the opening and the final snapshot, and never the 7 the guest stored.
    let l2_0 = |e: &Event| match e {
        Event::Snapshot { mem, .. } => mem.iter()
            .find(|r| r.ipa <= 0x8000 && 0x8000 < r.ipa + r.bytes.len() as u64)
            .map(|r| { let o = (0x8000 - r.ipa) as usize; u64::from_le_bytes(r.bytes[o..o + 8].try_into().unwrap()) })
            .expect("the L2 table page is in the snapshot"),
        other => panic!("not a snapshot: {other:?}"),
    };
    let (first, last) = (l2_0(&ev[0]), l2_0(&ev[ev.len() - 1]));
    assert_eq!(first, last, "L2[0] is unchanged");
    assert_ne!(last, 7, "the guest's store did not reach retrace's table");
    for i in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 42, "replay {i}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "replay {i} stdout");
    }
    let _ = std::fs::remove_file(&trace);
}

/// Review Focus 2: a converted fault on a thread that is not main. `thread1g`'s child loads from
/// 1 GiB (stage 1 maps it, nothing backs it: F2) while main is blocked in `pthread_join`. The
/// conversion builds the exception entry from the live vCPU, which holds the child, so the
/// terminal `Crash` and the delivery carry the child's tag and the handler runs on the child.
#[test]
fn a_converted_fault_on_a_child_thread_is_that_threads_crash() {
    let (want, ncode) = native(&["thread1g"]);
    assert_eq!(ncode, 139);
    let (rec, trace) = util::record_dynamic_args(FAULTCASE_DYN, &["thread1g"]);
    assert_eq!(rec.code, 139, "{}", rec.stderr);
    assert_eq!(rec.stdout, want);
    let ev = Reader::open(&trace).unwrap();
    let (_, esr, far, thread) = terminal_crash("thread1g", &ev);
    assert_eq!((esr, far), (0x9200_0007, 0x4000_0000), "the child's load, converted");
    assert_ne!(thread, 0, "the crash is the child's, not main's");
    for i in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 139, "replay {i}: {}", rp.stderr);
    }
    let _ = std::fs::remove_file(&trace);

    let (want, ncode) = native(&["thread1g", "h"]);
    assert_eq!(ncode, 42);
    assert!(String::from_utf8_lossy(&want).contains("on=child"), "native's handler runs on the child");
    let (rec, trace) = util::record_dynamic_args(FAULTCASE_DYN, &["thread1g", "h"]);
    assert_eq!(rec.code, 42, "{}", rec.stderr);
    assert_eq!(rec.stdout, want, "the handler's line, `on=child` included");
    let ev = Reader::open(&trace).unwrap();
    let tags: Vec<u32> = ev.iter().filter_map(|e| match e {
        Event::SignalDelivery { thread, si_code: 2, si_addr: 0x4000_0000, .. } => Some(*thread),
        _ => None,
    }).collect();
    assert_eq!(tags.len(), 1, "one SEGV_ACCERR delivery at 1 GiB");
    assert_ne!(tags[0], 0, "delivered to the child");
    for i in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 42, "replay {i}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "replay {i} stdout");
    }
    let _ = std::fs::remove_file(&trace);
}

/// Review Focus 3, as measured at plan time. A jump to 1 GiB does NOT reach stage 2: stage 1 maps
/// that address as a data block, whose UXN makes the fetch a stage-1 permission fault (ESR
/// 0x8200000e, level 2). `signal_of_esr` answers a permission fault SIGBUS/BUS_ADRALN (M13), where
/// native delivers SIGSEGV/SEGV_ACCERR. F does not touch a permission fault (spec §7), so this
/// pins the deviation by value, green before and after Task 3; Task 9 names it under Known limits.
/// `jumpgone`, in `CASES`, is the instruction abort that does reach stage 2, and it matches native.
#[test]
fn a_jump_into_unbacked_data_memory_is_still_a_stage1_permission_fault() {
    let (want, ncode) = native(&["jump1g", "h"]);
    assert_eq!(ncode, 42);
    assert_eq!(String::from_utf8_lossy(&want),
        "case jump1g\nHANDLER sig=11 si_code=2 si_addr=0x40000000 ec=0x20 wnr=0 on=main\n");
    let (rec, trace) = util::record_dynamic_args(FAULTCASE_DYN, &["jump1g", "h"]);
    assert_eq!(rec.code, 42, "{}", rec.stderr);
    assert_eq!(String::from_utf8_lossy(&rec.stdout),
        "case jump1g\nHANDLER sig=10 si_code=1 si_addr=0x40000000 ec=0x20 wnr=0 on=main\n",
        "retrace's SIGBUS/BUS_ADRALN, the deviation this test pins");
    for i in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 42, "replay {i}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "replay {i} stdout");
    }
    let _ = std::fs::remove_file(&trace);

    let (rec, trace) = util::record_dynamic_args(FAULTCASE_DYN, &["jump1g"]);
    assert_eq!(rec.code, 138, "{}", rec.stderr); // M51 Task 2: a permission fault's SIGBUS, until J1 (Task 4)
    let ev = Reader::open(&trace).unwrap();
    let (pc, esr, far, _) = terminal_crash("jump1g", &ev);
    assert_eq!((pc, esr, far), (0x4000_0000, 0x8200_000e, 0x4000_0000), "a stage-1 permission fault at level 2");
    let _ = std::fs::remove_file(&trace);
}

/// `fc_rs_null` (Task 1's corpus): a Rust null read meets libstd's SIGSEGV handler, which finds
/// the address outside every guard page, restores the default action and returns, so the read
/// faults again and the process dies, as `segv_rust_e2e`'s wild store does. Before F3 the read was
/// a recorder error.
#[test]
fn a_rust_null_read_runs_libstds_handler_then_crashes() {
    let (want, ncode) = run_native(retrace_guest::FC_RS_NULL, &[]);
    assert_eq!(ncode, 139, "native");
    let (rec, trace) = util::record_dynamic(retrace_guest::FC_RS_NULL);
    assert_eq!(rec.code, 139, "{}", rec.stderr);
    assert_eq!(rec.stdout, want);
    let ev = Reader::open(&trace).unwrap();
    let ds: Vec<(usize, u64, u64)> = ev.iter().enumerate().filter_map(|(i, e)| match e {
        Event::SignalDelivery { sig: 11, si_code: 2, si_addr, resume_pc, .. } => Some((i, *si_addr, *resume_pc)),
        _ => None,
    }).collect();
    assert_eq!(ds.len(), 1, "one SIGSEGV/SEGV_ACCERR delivery, to libstd's handler");
    let (di, si_addr, resume_pc) = ds[0];
    assert!(si_addr < 0x4000, "a null read: si_addr {si_addr:#x} is in page 0");
    assert!(ev[di..].iter().any(|e| matches!(e, Event::Syscall { num, .. } if *num == retrace_arch::SYS_SIGRETURN)),
        "the handler returned");
    let (pc, esr, far, _) = terminal_crash("fc_rs_null", &ev);
    assert_eq!((pc, far), (resume_pc, si_addr), "the same access faults again, now fatally");
    // A load (WnR clear) or a store: the class and level are what this test asserts.
    assert_eq!(esr & !0x40, 0x9200_0007, "a level-3 translation fault: {esr:#x}");
    for i in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 139, "replay {i}: {}", rp.stderr);
    }
    let _ = std::fs::remove_file(&trace);
}

/// A named limit (spec §3d, "Left as limits"): retrace's own pages in the low 4 GiB stay readable,
/// so a read at 0x30000 returns the TSD where native dies of SIGSEGV. Pinned so that a change to it
/// is seen.
#[test]
fn the_tsd_page_is_still_readable_a_named_limit() {
    let (_, ncode) = native(&["tsd"]);
    assert_eq!(ncode, 139, "native: 0x30000 is inside __PAGEZERO");
    let (rec, trace) = util::record_dynamic_args(FAULTCASE_DYN, &["tsd"]);
    assert_eq!(rec.code, 0, "{}", rec.stderr);
    assert!(String::from_utf8_lossy(&rec.stdout).starts_with("case tsd\nsurvived v="),
        "{:?}", String::from_utf8_lossy(&rec.stdout));
    for i in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 0, "replay {i}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "replay {i} stdout");
    }
    let _ = std::fs::remove_file(&trace);
}

/// Ruling T3-b. With a handler but no alternate stack, the overflowing thread's handler frame would
/// sit in the guard page, which nothing backs: native cannot push it and dies of the signal (139,
/// measured). F2 brings this fault to the delivery arm, so `Box_::fault_handler` must answer
/// "terminal" rather than let `write_guest` panic on the frame.
#[test]
fn a_stack_overflow_with_a_handler_but_no_alternate_stack_dies_as_native_does() {
    let (want, ncode) = native(&["deeprec", "h"]);
    assert_eq!(ncode, 139, "native: the frame cannot be pushed");
    let (rec, trace) = util::record_dynamic_args(FAULTCASE_DYN, &["deeprec", "h"]);
    assert_eq!(rec.code, 139, "{}", rec.stderr);
    assert_eq!(rec.stdout, want);
    let ev = Reader::open(&trace).unwrap();
    assert!(!ev.iter().any(|e| matches!(e, Event::SignalDelivery { .. })), "no frame was pushed");
    let (_, esr, far, _) = terminal_crash("deeprec h", &ev);
    assert_eq!(esr, 0x9200_0047, "the overflowing store, converted");
    assert!(GUARD.contains(&far), "{far:#x} is in the guard page");
    for i in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 139, "replay {i}: {}", rp.stderr);
    }
    let _ = std::fs::remove_file(&trace);
}

/// The other half of Ruling T3-b: on an alternate stack the frame fits, so the handler runs.
#[test]
fn a_stack_overflow_with_an_alternate_stack_handler_delivers_sigsegv() {
    let (want, ncode) = native(&["deeprec", "a"]);
    assert_eq!(ncode, 42);
    let (rec, trace) = util::record_dynamic_args(FAULTCASE_DYN, &["deeprec", "a"]);
    assert_eq!(rec.code, 42, "{}", rec.stderr);
    assert_eq!(rec.stdout, want, "the handler's line");
    let ev = Reader::open(&trace).unwrap();
    let ds: Vec<u64> = ev.iter().filter_map(|e| match e {
        Event::SignalDelivery { sig: 11, si_code: 2, si_addr, .. } => Some(*si_addr),
        _ => None,
    }).collect();
    assert_eq!(ds.len(), 1, "one SIGSEGV/SEGV_ACCERR delivery");
    assert!(GUARD.contains(&ds[0]), "{:#x} is in the guard page", ds[0]);
    for i in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 42, "replay {i}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "replay {i} stdout");
    }
    let _ = std::fs::remove_file(&trace);
}

/// Spec §4's seek guard. A checkpoint taken before a converted fault, restored, and advanced across
/// it equals a cold seek past it: the conversion and the delivery both run on a box built by
/// `from_checkpoint`.
#[test]
fn a_seek_across_a_converted_fault_matches_a_cold_seek() {
    let (rec, trace) = util::record_dynamic_args(FAULTCASE_DYN, &["low1g", "h"]);
    assert_eq!(rec.code, 42, "{}", rec.stderr);
    let di = Reader::open(&trace).unwrap().iter()
        .position(|e| matches!(e, Event::SignalDelivery { .. })).expect("the delivery");
    let cp = retrace_core::seek(&trace, di, 0).unwrap().checkpoint();
    let warm = {
        let mut s = retrace_core::ReplaySession::from_checkpoint(&trace, &cp).unwrap();
        s.advance_to_landmark(di + 1).unwrap_or_else(|d| panic!("warm: diverged at {}: {}", d.landmark, d.detail));
        (s.current_thread(), s.dbg_regs(), s.dbg_fp_regs(), s.dbg_internal_state(), s.snapshot().1)
    };
    let cold = retrace_core::seek(&trace, di + 1, 0).unwrap();
    assert_eq!(warm.0, cold.current_thread(), "thread: checkpointed vs cold");
    assert_eq!(warm.1, cold.dbg_regs(), "registers: checkpointed vs cold");
    assert_eq!(warm.2, cold.dbg_fp_regs(), "FP/SIMD: checkpointed vs cold");
    assert_eq!(warm.3, cold.dbg_internal_state(), "internal state: checkpointed vs cold");
    assert!(cold.diff_memory(&warm.4).is_none(), "memory: checkpointed vs cold");
    let _ = std::fs::remove_file(&trace);
}

/// Review Focus 6: stepping onto a converted fault. The step loops now receive
/// `Stage2::Converted`, which parks the guest at its EL1 vector exactly as a stepped stage-1 fault
/// does (spike F2). Each loop must report the crash at the faulting pc rather than step the
/// vector's `hvc`, and the parked session must then deliver the recorded outcome. `low1g`'s load
/// is a converted fault; with `h`, the delivery replays to the handler's exit, which also checks
/// that the converted SPSR is the PSTATE record saw (a delivery's frame carries it).
#[test]
fn stepping_onto_a_converted_fault_reports_the_crash_at_the_faulting_pc() {
    let (rec, trace) = util::record_dynamic_args(FAULTCASE_DYN, &["low1g"]);
    assert_eq!(rec.code, 139, "{}", rec.stderr);
    let ev = Reader::open(&trace).unwrap();
    let ci = ev.len() - 2;
    let (pc, esr, far, _) = terminal_crash("low1g", &ev);
    assert_eq!((esr, far), (0x9200_0007, 0x4000_0000), "low1g's load is a converted fault");
    let n = retrace_core::seek(&trace, ci, 0).unwrap().window_len_here().expect("the crash window ends at the fault");
    {
        let mut s = retrace_core::seek(&trace, ci, n).unwrap();
        assert_eq!(s.pc(), pc, "({ci}, {n}) is parked on the faulting load");
        assert_eq!(s.step_insns(1), Err(format!("guest crashed at step 0/1: pc={pc:#x} far={far:#x}")));
    }
    {
        let mut s = retrace_core::seek(&trace, ci, n).unwrap();
        assert_eq!(s.step_watched(), Err(format!("guest crashed during a watch-aware step: pc={pc:#x} far={far:#x}")));
    }
    {
        let mut s = retrace_core::seek(&trace, ci, n).unwrap();
        assert_eq!(s.step_armed(), Ok(Armed::Fault));
        match s.advance().unwrap_or_else(|d| panic!("after the step: diverged at {}: {}", d.landmark, d.detail)) {
            Advance::Exited(r) => assert!(matches!(r.outcome, Outcome::Crash { pc: p, esr: e, far: f, .. } if (p, e, f) == (pc, esr, far)), "{:?}", r.outcome),
            _ => panic!("the session parked on the converted fault must end in the recorded crash"),
        }
    }
    let _ = std::fs::remove_file(&trace);

    let (rec, trace) = util::record_dynamic_args(FAULTCASE_DYN, &["low1g", "h"]);
    assert_eq!(rec.code, 42, "{}", rec.stderr);
    let di = Reader::open(&trace).unwrap().iter()
        .position(|e| matches!(e, Event::SignalDelivery { .. })).expect("the delivery");
    let n = retrace_core::seek(&trace, di, 0).unwrap().window_len_here().expect("the delivery's window ends at the fault");
    let mut s = retrace_core::seek(&trace, di, n).unwrap();
    assert_eq!(s.step_armed(), Ok(Armed::Fault));
    loop {
        match s.advance().unwrap_or_else(|d| panic!("with h, after the step: diverged at {}: {}", d.landmark, d.detail)) {
            Advance::Exited(r) => {
                assert_eq!(r.outcome, Outcome::Exit { code: 42 }, "the handler ran and exited");
                break;
            }
            _ => continue,
        }
    }
    let _ = std::fs::remove_file(&trace);
}
