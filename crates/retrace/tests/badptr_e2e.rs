//! M51 §3h: a pointer the program could not pass natively gets native's error under retrace, whether
//! retrace forwards the call or services it, and nothing is written through it. Every expected line is
//! native's (t0 M5); each test also compares the recording's output with a native run's.
mod util;
use retrace_trace::{Event, Reader};
use std::path::{Path, PathBuf};

const BOUND: u64 = 120;
const P: &str = retrace_guest::FID_BADPTR;

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("retrace-badptr-{}-{name}", std::process::id()))
}

/// Native, record and two replays of one mode: the same stdout, exit 0, no wall and no bug line.
/// Returns stdout and the recording's events.
fn agree(mode: &str, t: &Path) -> (String, Vec<Event>) {
    let native = util::run_bounded(P, &[mode], &[], BOUND);
    assert_eq!(native.code, Some(0), "{mode}: native. stderr:\n{}", native.stderr);
    let rec = util::run_bounded(util::bin(), &["record-dyn", P, "-o", t.to_str().unwrap(), "--", mode], &[], BOUND);
    assert!(!rec.stderr.contains("retrace: "), "{mode}: no line of retrace's own. stderr:\n{}", rec.stderr);
    assert_eq!((rec.code, rec.stdout.as_slice()), (Some(0), native.stdout.as_slice()), "{mode}: record against native");
    for i in 1..=2 {
        let rp = util::run_bounded(util::bin(), &["replay", t.to_str().unwrap()], &[], BOUND);
        assert_eq!((rp.code, rp.stdout.as_slice()), (Some(0), rec.stdout.as_slice()), "{mode}: replay {i}. stderr:\n{}", rp.stderr);
    }
    (String::from_utf8_lossy(&native.stdout).into_owned(), Reader::open(t).unwrap())
}

/// The recording's last landmark of syscall `num` that failed.
fn failed(events: &[Event], num: u64) -> (u64, usize) {
    events.iter().rev().find_map(|e| match e {
        Event::Syscall { num: n, ret, err: true, writes, .. } if *n == num => Some((*ret, writes.len())),
        _ => None,
    }).unwrap_or_else(|| panic!("no failed syscall {num} in the recording"))
}

/// Spec §4: a read into a page the program unmapped. Natively `EFAULT`; under retrace the call is
/// refused before the forward, recorded with no writes.
#[test]
fn a_read_into_unmapped_memory_returns_efault() {
    let t = scratch("unmapped.bin");
    let (out, events) = agree("readunmapped", &t);
    assert_eq!(out, "read(hole): rc=-1 errno=14\n");
    assert_eq!(failed(&events, 3), (retrace_arch::EFAULT, 0), "EFAULT, nothing written");
    let _ = std::fs::remove_file(&t);
}

/// Spec §4 "Bad pointers" (C1: the read-only page retrace models is the program's own code). Without
/// the check the forward writes into the code's backing and the call succeeds.
#[test]
fn a_read_into_the_programs_own_code_returns_efault_and_writes_nothing() {
    let t = scratch("code.bin");
    let (out, _) = agree("readcode", &t);
    assert_eq!(out, "read(code): rc=-1 errno=14\nunchanged=1\n");
    let _ = std::fs::remove_file(&t);
}

/// A forwarded call whose out-pointer is unmapped.
#[test]
fn a_stat_into_unmapped_memory_returns_efault() {
    let t = scratch("stat.bin");
    let (out, _) = agree("statbad", &t);
    assert_eq!(out, "stat(hole): rc=-1 errno=14\n");
    let _ = std::fs::remove_file(&t);
}

/// The console write, which retrace services rather than forwards: `EFAULT`, and nothing reaches stdout.
#[test]
fn a_console_write_from_unmapped_memory_returns_efault() {
    let t = scratch("console.bin");
    let (out, events) = agree("consolebad", &t);
    assert_eq!(out, "write(1, hole): rc=-1 errno=14\n", "the report line alone: the failed write printed nothing");
    assert_eq!(failed(&events, 4), (retrace_arch::EFAULT, 0));
    let _ = std::fs::remove_file(&t);
}

/// Spec §3h.2: two calls retrace emulates, a BSD one and a Mach trap, each with native's error
/// (t0 M5: `EFAULT` for the raw `sigaltstack`, `KERN_MEMORY_ERROR` for the allocate trap).
#[test]
fn an_emulated_call_with_a_bad_pointer_returns_natives_error() {
    // Each line is native's (t0 M6 and the fix round's own measurement). `sigprocmaskold` and
    // `sigpendingbad` are swallowed natively: ret 0, and the new mask still applies. `mach2bad` is a
    // forwarded task_info send, answered 0x10004002. `straddle` is a 24-byte struct whose second half is unmapped.
    for (mode, want) in [("altstackbad", "sigaltstack(hole): rc=-1 errno=14\n"),
                         ("machbad", "mach_vm_allocate_trap(hole): kr=0xa\n"),
                         ("sigactionnew", "sigaction(new hole): rc=-1 errno=14\n"),
                         ("sigactionold", "sigaction(old hole): rc=-1 errno=14\n"),
                         ("sigprocmaskset", "sigprocmask(set hole): rc=-1 errno=14\n"),
                         ("sigprocmaskold", "sigprocmask(oldset hole): rc=0 errno=0\nblocked=1\n"),
                         ("sigpendingbad", "sigpending(hole): rc=0 errno=0\n"),
                         ("getrlimitbad", "getrlimit(hole): rc=-1 errno=14\n"),
                         ("mach2bad", "mach_msg2(hole): ret=0x10004002\n"),
                         // I-1: a `Ptr` out-struct aimed at the program's own code page; nothing is written.
                         ("statcode", "stat(code): rc=-1 errno=14\nunchanged=1\n"),
                         // I-2/I-3: a 36-byte 3409 request ending at a page edge, the next page unmapped; the
                         // 40-byte reply does not fit. Native is MACH_RCV_INVALID_DATA, measured.
                         ("mach3409edge", "mach_msg2(3409 at edge): ret=0x10004008\n"),
                         // I-4: a page the program has not made accessible is EFAULT natively, and under
                         // retrace the program's own load from it faults too (SIGBUS, as natively), so the
                         // check agrees with it.
                         ("nonemmap", "nonemmap: rc=-1 errno=14\n"),
                         ("noneprotect", "noneprotect: rc=-1 errno=14\n"),
                         ("straddle", "sigaltstack(straddle): rc=-1 errno=14\n"),
                         // FW-1: NULL plus a field offset, and a top-byte-tagged pointer, to the structs retrace
                         // services itself. Native EFAULTs for all of them (a tagged VA is no TBI address to the
                         // kernel's copyout, measured), where they used to reach the bug banner.
                         ("oact10", "sigaction(oact 0x10): rc=-1 errno=14\n"),
                         ("rawss10", "sigaltstack(ss 0x10): rc=-1 errno=14\n"),
                         ("oacttag", "sigaction(oact tagged): rc=-1 errno=14\n"),
                         ("rawsstag", "sigaltstack(oss tagged): rc=-1 errno=14\n"),
                         ("sigmask10", "sigprocmask(set 0x10): rc=-1 errno=14\n"),
                         // FW-2: proc_info SETCONTROL copies the name IN, so a literal in read-only __TEXT succeeds.
                         ("setnamelit", "rc=0 name=\"worker\"\n"),
                         // FW-3: F_SETOWN's argument is an integer pid; native reads it back negated.
                         // FW-R: a no-argument command's third word is not a pointer; native succeeds.
                         ("getlease", "fcntl(F_GETLEASE, 0x12345678): rc=2 errno=0\n"),
                         ("fullfsync","fcntl(F_FULLFSYNC, 0x12345678): rc=0 errno=0\n"),
                         ("setown","setown: rc=0 errno=0 own=-70000\n")] {
        let t = scratch(&format!("{mode}.bin"));
        let (out, _) = agree(mode, &t);
        assert_eq!(out, want, "{mode}");
        let _ = std::fs::remove_file(&t);
    }
}

/// C3: replay makes the same check at the same point and compares its answer with the recording's. A
/// recording that says the bad read succeeded diverges at that landmark.
#[test]
fn a_recording_that_forwarded_a_pointer_replay_refuses_is_a_divergence() {
    let t = scratch("tamper.bin");
    agree("readunmapped", &t);
    let mut events = Reader::open(&t).unwrap();
    let i = events.iter().rposition(|e| matches!(e, Event::Syscall { num: 3, err: true, .. })).expect("the refused read");
    if let Event::Syscall { ret, err, .. } = &mut events[i] { *ret = 16; *err = false; }
    let mut w = retrace_trace::Writer::create(&t).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);
    let rp = util::run_bounded(util::bin(), &["replay", t.to_str().unwrap()], &[], BOUND);
    assert_eq!(rp.code, Some(3), "a divergence. stderr:\n{}", rp.stderr);
    assert!(rp.stderr.contains("replay refuses a pointer"), "the check's own compare names it. stderr:\n{}", rp.stderr);
    let _ = std::fs::remove_file(&t);
}

/// FW-5: a handler that rewrites its frame's `uc_mcontext` to unmapped memory ends the recording at a wall
/// (exit 4, the `unsupported` line), never at the bug banner (exit 70). Natively the kernel kills the process;
/// retrace does not model that, and says so.
#[test]
fn a_handler_that_smashes_its_mcontext_pointer_is_a_wall_not_a_bug() {
    let t = scratch("badmctx.bin");
    let rec = util::run_bounded(util::bin(), &["record-dyn", P, "-o", t.to_str().unwrap(), "--", "badmctx"], &[], BOUND);
    assert_eq!(rec.code, Some(4), "a wall. stderr:\n{}", rec.stderr);
    assert!(rec.stderr.contains("retrace: unsupported:") && rec.stderr.contains("mcontext at 0x10"),
        "the wall names the frame's mcontext. stderr:\n{}", rec.stderr);
    assert!(!rec.stderr.contains("retrace's bug"), "no bug banner. stderr:\n{}", rec.stderr);
    let _ = std::fs::remove_file(&t);
}
