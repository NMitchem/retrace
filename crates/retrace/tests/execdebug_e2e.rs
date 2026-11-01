//! M54 task 6 (spec §3i, C6, C8): the debugger over a recording that holds an exec. Every row runs
//! `execself_dyn copy <target>` (the program replaces its image with `<target>`'s bytes, copied to a
//! path of its own), so a name a test resolves can only come from the epoch's own image: epoch 0 is
//! `execself_dyn`, epoch 1 the copy. The two share `_main` and differ everywhere else (`_do_exec` is
//! `execself_dyn`'s alone, `_hex` the target's), and every expected address comes from the binary or
//! from the recording, never from the debugger under test.
mod util;
use retrace_trace::Event;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const BOUND: u64 = 300;

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!("retrace-execdbg-{}-{}-{name}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
}

/// A copy of `src` at a path of its own: the image the program execs.
fn copy_of(src: &str, name: &str) -> PathBuf {
    let p = scratch(name);
    std::fs::copy(src, &p).unwrap();
    p
}

/// Record `execself_dyn copy <target>`; the trace and the copy's path (the Exec's path).
fn record_copy(src: &str, expect_code: i32) -> (PathBuf, PathBuf) {
    let target = copy_of(src, "target");
    let t = scratch("t.bin");
    let r = util::run_bounded(util::bin(), &["record-dyn", retrace_guest::EXECSELF_DYN, "-o", t.to_str().unwrap(),
                                             "--", "copy", target.to_str().unwrap()], &[], BOUND);
    assert_eq!(r.code, Some(expect_code), "the recording exits {expect_code}. stderr:\n{}", r.stderr);
    (t, target)
}

fn events(trace: &Path) -> Vec<Event> { retrace_trace::Reader::open(trace).unwrap().into_iter().collect() }

/// The landmark number an exec stop sits at: the Exec event's index, plus one (events[0] is the snapshot).
fn exec_n(trace: &Path) -> usize {
    events(trace).iter().position(|e| matches!(e, Event::Exec { .. })).expect("an Exec landmark") + 1
}

fn dbg(trace: &Path, script: &str) -> String {
    let (code, out, err) = util::debug_bounded(trace.to_str().unwrap(), script, BOUND);
    assert_eq!(code, Some(0), "debug {script:?} exits 0. stderr:\n{err}\nstdout:\n{out}");
    out
}

/// The address a `breakpoint at 0x… (name)` echo names, for the nth such line.
fn bp_addrs(out: &str) -> Vec<u64> {
    out.lines().filter_map(|l| l.strip_prefix("breakpoint at 0x"))
        .map(|r| u64::from_str_radix(r.split_whitespace().next().unwrap(), 16).unwrap()).collect()
}

fn dyld_entry(trace: &Path) -> u64 {
    let ev = events(trace);
    let Some(Event::Exec { dylinker, .. }) = ev.iter().find(|e| matches!(e, Event::Exec { .. })) else { panic!("no Exec") };
    retrace_guest::try_parse_macho(dylinker).unwrap().entry + retrace_box::DYLD_BASE
}

fn nm_addr(bin: &str, sym: &str) -> u64 {
    let o = std::process::Command::new("nm").args(["-arch", "arm64", bin]).output().expect("nm");
    String::from_utf8_lossy(&o.stdout).lines()
        .find_map(|l| { let mut f = l.split_whitespace(); let a = f.next()?; let _ = f.next()?; (f.next()? == sym).then(|| u64::from_str_radix(a, 16).unwrap()) })
        .unwrap_or_else(|| panic!("no {sym} in {bin}: nm said {:?} / {}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
}

#[test]
fn where_names_each_epochs_symbol() {
    let (t, target) = record_copy(retrace_guest::EXEC_TARGET_DYN, 0);
    let n = exec_n(&t);
    let tp = target.to_str().unwrap();
    // Before the exec: execself_dyn's symbols. `_do_exec` is its alone.
    let out = dbg(&t, "break _main; break _do_exec; continue; where");
    let a = bp_addrs(&out);
    assert_eq!(a[0], nm_addr(retrace_guest::EXECSELF_DYN, "_main"), "{out}");
    assert!(out.lines().any(|l| l.starts_with("at (") && l.contains(" in _main")), "where names execself's _main:\n{out}");
    // At the exec stop, the new image's entry, dyld's (Ruling T6-a, the debugger-side guard for I2), and
    // the new epoch's symbols from there on (T6-c): `break` resolves against the epoch of the position.
    let out = dbg(&t, "continue; where; break _main; break _hex; continue; where");
    assert!(out.contains(&format!("exec {tp} at ({n}, 0)")), "{out}");
    let entry = dyld_entry(&t);
    assert!(out.lines().any(|l| l.starts_with(&format!("at ({n}, 0) pc={entry:#x} ")) && l.contains(" in __dyld_start")),
            "at the exec stop `where` names dyld's entry {entry:#x}:\n{out}");
    let b = bp_addrs(&out);
    assert_eq!(b[0], nm_addr(target.to_str().unwrap(), "_main"), "`break _main` resolves in the new image:\n{out}");
    assert_ne!(b[0], a[0], "and not at execself's _main");
    assert!(out.lines().any(|l| l.starts_with("hit ") && l.contains(" in _main")), "the breakpoint set after the exec is hit:\n{out}");
    assert!(out.lines().rev().find(|l| l.starts_with("at (")).is_some_and(|l| l.contains(" in _main")), "{out}");
    // A name only the old image defines is no symbol once the position is in the new one.
    let (code, out, err) = util::debug_bounded(t.to_str().unwrap(), "continue; break _do_exec", BOUND);
    assert_ne!(code, Some(0), "`_do_exec` is not in the target:\n{out}");
    assert!(err.contains("no symbol \"_do_exec\""), "{err}");
    let _ = (std::fs::remove_file(&t), std::fs::remove_file(&target));
}

#[test]
fn continue_stops_at_the_exec_and_says_so() {
    let (t, target) = record_copy(retrace_guest::EXEC_TARGET_DYN, 0);
    let n = exec_n(&t);
    let tp = target.to_str().unwrap();
    let out = dbg(&t, "continue; continue");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[1], format!("exec {tp} at ({n}, 0)"), "the first continue stops at the exec:\n{out}");
    assert!(lines[3].starts_with("exited (code 0)"), "the second runs on to the end:\n{out}");
    // A breakpoint armed in the old image does not hide the exec.
    let out = dbg(&t, "break _main; continue; continue");
    assert!(out.contains(&format!("exec {tp} at ({n}, 0)")), "{out}");
    let _ = (std::fs::remove_file(&t), std::fs::remove_file(&target));
}

#[test]
fn reverse_continue_from_after_an_exec_stops_at_it() {
    // Reverse is followed in full by `retrace debug` (it holds every epoch's symbols), Ruling T6-e.
    let (t, target) = record_copy(retrace_guest::EXEC_TARGET_DYN, 0);
    let n = exec_n(&t);
    let tp = target.to_str().unwrap();
    let out = dbg(&t, "continue; continue; reverse-continue; where; reverse-continue");
    assert_eq!(out.matches(&format!("exec {tp} at ({n}, 0)")).count(), 2, "forward once, then backward from the end:\n{out}");
    assert!(out.lines().any(|l| l.starts_with(&format!("at ({n}, 0) pc="))), "parked at the exec:\n{out}");
    assert!(out.contains("no earlier hit"), "nothing before the exec but the start:\n{out}");
    // Step rows (Ruling T6-d): a stepi from the exec svc lands at (n, 0) with the exec line, and a reverse
    // stepi from there is the svc in the OLD epoch, where `where` names the old image's symbol.
    let out = dbg(&t, "continue; reverse-stepi; where; stepi; where");
    let m = n - 1;
    let wheres: Vec<&str> = out.lines().filter(|l| l.starts_with("at (")).collect();
    assert!(wheres[0].starts_with(&format!("at ({m}, ")) && !wheres[0].contains("__dyld_start"), "the svc, in the old epoch:\n{out}");
    assert!(out.contains(&format!("exec {tp} at ({n}, 0)")), "{out}");
    assert!(wheres[1].starts_with(&format!("at ({n}, 0) ")) && wheres[1].contains(" in __dyld_start"), "after the stepi:\n{out}");
    let _ = (std::fs::remove_file(&t), std::fs::remove_file(&target));
}

#[test]
fn a_seek_across_an_exec_from_a_checkpoint_matches_a_cold_seek() {
    let (t, target) = record_copy(retrace_guest::EXEC_TARGET_DYN, 0);
    let n = exec_n(&t);
    // A cache that keeps every position, so the second seek really restores one taken before the exec.
    let mut cache = retrace_core::CheckpointCache::new(256 * 1024 * 1024, 0);
    for (pn, pk) in [(n - 1, 3u64), (n, 0), (n + 2, 5), (n + 2, 9)] {
        // One VM per process: each session is read and dropped before the next opens.
        let warm = {
            let mut s = retrace_core::checkpointed_seek(&t, &mut cache, pn, pk).unwrap();
            let regs = s.dbg_regs();
            (regs, s.snapshot())
        };
        let cold = {
            let mut s = retrace_core::seek(&t, pn, pk).unwrap();
            let regs = s.dbg_regs();
            (regs, s.snapshot())
        };
        assert_eq!(warm.0, cold.0, "registers at ({pn}, {pk})");
        assert!(warm.1 == cold.1, "memory and registers at ({pn}, {pk}) match a cold seek");
    }
    assert!(!cache.is_empty(), "the cache held positions to restore from");
    let _ = (std::fs::remove_file(&t), std::fs::remove_file(&target));
}

#[test]
fn the_crash_line_names_the_last_epochs_function() {
    // The closing line comes from the final snapshot, whose memory is the last epoch's.
    let (t, target) = record_copy(retrace_guest::CRASHY, 139);
    let main = nm_addr(target.to_str().unwrap(), "_main");
    let rp = util::run_bounded(util::bin(), &["replay", t.to_str().unwrap()], &[], BOUND);
    assert_eq!(rp.code, Some(139), "{}", rp.stderr);
    let line = rp.stderr.lines().find(|l| l.starts_with("retrace: the program was killed by")).unwrap_or_else(|| panic!("{}", rp.stderr));
    assert!(line.contains(" in _main+0x"), "{line}");
    // `retrace debug` annotates the crash from the last epoch too, at the offset from crashy's own main.
    let out = dbg(&t, "continue; continue");
    let crash = out.lines().find(|l| l.starts_with("guest crashed: pc=")).unwrap_or_else(|| panic!("{out}"));
    let pc = u64::from_str_radix(crash["guest crashed: pc=0x".len()..].split_whitespace().next().unwrap(), 16).unwrap();
    assert!(crash.ends_with(&format!("  in _main+{:#x}", pc - main)), "{crash}");
    let _ = (std::fs::remove_file(&t), std::fs::remove_file(&target));
}

/// `LC_UUID` of a thin arm64 Mach-O, formatted as the gdbserver's image list prints it.
fn lc_uuid(path: &str) -> String {
    let b = std::fs::read(path).unwrap();
    let slice = if b[0..4] == [0xca, 0xfe, 0xba, 0xbe] { panic!("{path} is fat") } else { &b[..] };
    let ncmds = u32::from_le_bytes(slice[16..20].try_into().unwrap());
    let mut off = 32usize;
    for _ in 0..ncmds {
        let (cmd, size) = (u32::from_le_bytes(slice[off..off + 4].try_into().unwrap()), u32::from_le_bytes(slice[off + 4..off + 8].try_into().unwrap()) as usize);
        if cmd == 0x1b {
            let h: String = slice[off + 8..off + 24].iter().map(|x| format!("{x:02X}")).collect();
            return format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]);
        }
        off += size;
    }
    panic!("no LC_UUID in {path}");
}

#[test]
fn the_gdbserver_reports_an_exec_stop_and_the_new_image() {
    use util::rsp::{self as r, Rsp};
    let (t, target) = record_copy(retrace_guest::EXEC_TARGET_DYN, 0);
    let n = exec_n(&t);
    let (u_old, u_new) = (lc_uuid(retrace_guest::EXECSELF_DYN), lc_uuid(target.to_str().unwrap()));
    assert_ne!(u_old, u_new, "the fixtures differ");
    let fetch = r#"jGetLoadedDynamicLibrariesInfos:{"fetch_all_solibs":true}"#;
    let mut c = Rsp::spawn(&t, &[]);
    let before = c.send(fetch);
    assert!(before.contains(&u_old) && !before.contains(&u_new), "before the exec the image is execself_dyn's: {before}");
    let stop = c.send("c");
    assert!(stop.starts_with("T05thread:1;") && stop.contains("reason:exec;"), "{stop}");
    assert_eq!(c.send("?"), stop, "`?` repeats it");
    assert_eq!(c.where_().split(" pc=").next().unwrap(), format!("at ({n}, 0) phase=Bp"));
    let after = c.send(fetch);
    assert!(after.contains(&u_new) && !after.contains(&u_old), "after it, the target's: {after}");
    assert!(after.contains(target.to_str().unwrap()), "named by the path the guest passed: {after}");
    // Ruling T6-d: `s` from the exec svc is the exec stop. Standing on landmark n-1's svc, step.
    let mut c2 = Rsp::spawn(&t, &[]);
    let svc = r::trap_pc(&t, n - 1);
    assert_eq!(c2.send(&format!("Z0,{svc:x},4")), "OK");
    let (_, _) = r::continue_to_window(&mut c2, n - 1);
    let s = c2.send("s");
    assert!(s.contains("reason:exec;"), "a step across the exec is the exec stop: {s}");
    assert!(c2.where_().starts_with(&format!("at ({n}, 0) ")), "{}", c2.where_());
    let _ = (std::fs::remove_file(&t), std::fs::remove_file(&target));
}

#[test]
fn the_gdbserver_refuses_to_cross_an_exec_in_reverse_and_reports_the_stop_at_it() {
    use util::rsp::{self as r, Rsp};
    let (t, target) = record_copy(retrace_guest::EXEC_TARGET_DYN, 0);
    let n = exec_n(&t);
    let mut c = Rsp::spawn(&t, &[]);
    let fwd = c.send("c");
    assert!(fwd.contains("reason:exec;"), "{fwd}");
    let at = c.where_();
    // A reverse resume from the exec stop is refused by value, an exception stop that does not move.
    for p in ["bc", "bs"] {
        let s = c.send(p);
        assert_eq!(r::description(&s).as_deref(), Some("reverse execution across an exec is not supported"), "{p}: {s}");
        assert_eq!(c.where_(), at, "{p}: the position is unchanged");
    }
    // From the end, the reverse stop at the exec is reported as the exec stop, and it is then the refusal's position.
    let end = c.send("c");
    assert!(end.contains("replaylog:end;"), "{end}");
    let back = c.send("bc");
    assert!(back.contains("reason:exec;"), "the reverse stop at the exec: {back}");
    assert!(c.where_().starts_with(&format!("at ({n}, 0) phase=Bp")), "{}", c.where_());
    // One step back inside the new epoch is not a crossing: forward again, then back one instruction.
    assert!(c.send("s").contains("reason:trace;"));
    assert!(c.send("bs").contains("reason:trace;"), "a step back within the epoch is allowed");
    let _ = (std::fs::remove_file(&t), std::fs::remove_file(&target));
}
