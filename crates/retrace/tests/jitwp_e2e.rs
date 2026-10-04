//! M48 Task 6 gate (spec §3f, §3h, §4): JIT write-protect on a repo-owned fixture, so the mechanism
//! is guarded on a machine without node. Every assertion is on the difference J1 makes, never on an
//! exit code a weaker failure also produces:
//! - `basic`: native's register sequence and 42 from JIT code. A flip-on-fault model (J2) returns 42
//!   too, which is why `fault` exists.
//! - `v8`: V8's shape, `PROT_NONE` then an RWX commit inside it (Review Focus item 4).
//! - `twothreads`: B runs the page while A is write-enabled; a process-wide view would fault.
//! - `fault`: a store to a protected page is the crash native takes, where J2 would exit 0.
//! - a seek into a write-enabled window, a single-step across a toggle (P9), and a
//!   `reverse-continue` to the code write, checked by its effect.
//! - `sprrprobe`: the static probe reads 0, runs `ic ivau`, and is refused at its `msr` by value.
mod util;

use retrace_trace::{Event, Reader};
use std::path::{Path, PathBuf};

/// The fixture run natively.
fn native(mode: &str) -> std::process::Output {
    std::process::Command::new(retrace_guest::JITWP_DYN).arg(mode).output().expect("run jitwp_dyn natively")
}

/// Record `mode`, assert exit 0, and replay twice byte-identically.
fn records_and_replays(mode: &str) -> (util::RunOut, PathBuf) {
    let (rec, trace) = util::record_dynamic_args(retrace_guest::JITWP_DYN, &[mode]);
    assert_eq!(rec.code, 0, "{mode}: record: {}", rec.stderr);
    for n in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 0, "{mode}: replay {n}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "{mode}: replay {n} stdout");
    }
    (rec, trace)
}

/// The page out of the guest's own `JITWP page=0x…` marker.
fn page(stdout: &str) -> u64 {
    let at = stdout.find("JITWP page=0x").unwrap_or_else(|| panic!("no page marker in {stdout:?}")) + 13;
    let rest = &stdout[at..];
    u64::from_str_radix(&rest[..rest.find('\n').unwrap()], 16).unwrap()
}

/// Every line but the page marker, whose address differs natively.
fn without_marker(stdout: &str) -> Vec<String> {
    stdout.lines().filter(|l| !l.starts_with("JITWP page=")).map(str::to_string).collect()
}

/// `(landmark, thread)` of every write to stdout, in order. stdout is line-buffered, so each is one line.
fn stdout_writes(trace: &Path) -> Vec<(usize, u32)> {
    Reader::open(trace).unwrap().into_iter().enumerate().filter_map(|(i, e)| match e {
        Event::Syscall { num, args, thread, .. } if retrace_arch::is_write_syscall(num) && args[0] == 1 => Some((i, thread)),
        _ => None,
    }).collect()
}

/// `basic`'s landmarks: the write of the `main write-en` line, made inside the write-enabled window,
/// and the write of `jit call=42`, made after the protect toggle and the call into the page.
fn basic_landmarks(stdout: &str, trace: &Path) -> (usize, usize) {
    let lines: Vec<&str> = stdout.lines().collect();
    let writes = stdout_writes(trace);
    assert_eq!(writes.len(), lines.len(), "one write per line");
    let we = lines.iter().position(|l| l.starts_with("main write-en sprr=")).expect("the write-enabled line");
    assert!(lines[we + 1].starts_with("main protect sprr=") && lines[we + 2] == "jit call=42", "{lines:?}");
    (writes[we].0, writes[we + 2].0)
}

#[test]
fn basic_runs_natives_jit_sequence_and_replays() {
    let nat = native("basic");
    assert!(nat.status.success(), "native basic: {nat:?}");
    let nat_out = String::from_utf8(nat.stdout).unwrap();
    let (rec, trace) = records_and_replays("basic");
    let out = String::from_utf8(rec.stdout).unwrap();
    // R1's named deviation: native's register starts at the commpage's +0x118, retrace's at 0. Both
    // mean protected (§2c), so every other line, the register around each toggle included, is native's.
    let protect = nat_out.split("+0x118=").nth(1).and_then(|r| r.split_whitespace().next())
        .unwrap_or_else(|| panic!("no commpage line in {nat_out:?}")).to_string();
    let want: Vec<String> = without_marker(&nat_out).into_iter().map(|l| {
        for who in ["main initial sprr=", "child initial sprr="] {
            if let Some(v) = l.strip_prefix(who) {
                assert_eq!(v, protect, "native starts every thread protected (§2c): {l}");
                return format!("{who}0");
            }
        }
        l
    }).collect();
    assert_eq!(without_marker(&out), want, "the guest's lines are native's, R1's two lines aside");
    assert!(out.contains("jit call=42\n"), "{out}");
    assert!(!rec.stderr.contains("anon PROT_EXEC mmap"),
        "a MAP_JIT map is exempt from the anon-exec warning (§11a item 2):\n{}", rec.stderr);
    let maps = Reader::open(&trace).unwrap().iter().filter(|e| matches!(e,
        Event::Syscall { num, args, err: false, .. } if *num == retrace_arch::SYS_MMAP && args[3] & retrace_arch::MAP_JIT != 0)).count();
    assert_eq!(maps, 1, "the recording holds the one MAP_JIT mapping");
}

/// Review Focus item 4 (e2e half): V8 maps `PROT_NONE`, then `mprotect`s a sub-range RWX, which
/// `guest_mprotect` routes through `unprotect`, and `unprotect` stamps `ATTR_DATA`. The call follows
/// two toggles, whose flips restamp the range anyway, so this pins V8's shape end to end, not the
/// restamp itself: natively a committed MAP_JIT page refuses every further `mprotect` (EACCES,
/// t6-jitprobe), so no native flow calls into a page between its commit and a toggle. The restamp
/// is pinned at the box level (`tests/jit.rs`, `an_unprotect_inside_a_jit_range_…`).
#[test]
fn the_v8_shape_none_mapped_then_mprotected_rwx_runs_its_code() {
    let nat = native("v8");
    assert!(nat.status.success(), "native v8: {nat:?}");
    let (rec, trace) = records_and_replays("v8");
    assert_eq!(rec.stdout, nat.stdout, "native's lines");
    assert_eq!(rec.stdout, b"v8 call=42\nv8 unmapped\n");
    let evs = Reader::open(&trace).unwrap();
    let (base, len) = evs.iter().find_map(|e| match e {
        Event::Syscall { num, args, ret, err: false, .. }
            if *num == retrace_arch::SYS_MMAP && args[3] & retrace_arch::MAP_JIT != 0 && args[2] == 0 => Some((*ret, args[1])),
        _ => None,
    }).expect("a PROT_NONE MAP_JIT mapping");
    assert!(evs.iter().any(|e| matches!(e, Event::Syscall { num, args, .. }
            if *num == retrace_arch::SYS_MPROTECT && args[2] == 7 && args[0] > base && args[0] < base + len)),
        "an RWX mprotect strictly inside it, V8's commit");
}

/// Spec §4: B runs the page while A is write-enabled. With a process-wide view, B would execute an
/// `ATTR_DATA` page and fault.
#[test]
fn b_runs_the_page_while_a_is_write_enabled() {
    let nat = native("twothreads");
    assert!(nat.status.success(), "native twothreads: {nat:?}");
    let (rec, trace) = records_and_replays("twothreads");
    let out = String::from_utf8(rec.stdout).unwrap();
    assert_eq!(without_marker(&out), without_marker(&String::from_utf8(nat.stdout).unwrap()),
        "native's lines, in native's order, A's write-enabled register included");
    let writes = stdout_writes(&trace);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), writes.len(), "one write per line");
    for (l, &(_, t)) in lines.iter().zip(&writes) {
        let want = if l.starts_with("B ran") { 1 } else { 0 };
        assert_eq!(t, want, "{l:?} was written by thread {t}: B's lines are thread 1's, A's thread 0's");
    }
    assert!(lines.contains(&"B ran 1") && lines.contains(&"B ran 2"), "{out}");
}

/// Spec §4: a store to a protected `MAP_JIT` page is the crash native takes. J2's flip-on-fault
/// would repair it silently and print `UNREACHED`.
#[test]
fn a_store_to_a_protected_jit_page_is_the_recorded_crash_native_takes() {
    use std::os::unix::process::ExitStatusExt;
    let nat = native("fault");
    assert!(nat.status.signal().is_some(), "natively the store kills the process with a signal: {:?}", nat.status);
    assert!(!String::from_utf8_lossy(&nat.stdout).contains("UNREACHED"));
    let (rec, trace) = util::record_dynamic_args(retrace_guest::JITWP_DYN, &["fault"]);
    assert_eq!(rec.code, 139, "a recorded crash exits 139 (M6). stderr: {}", rec.stderr);
    let out = String::from_utf8_lossy(&rec.stdout).into_owned();
    assert!(!out.contains("UNREACHED"), "the store must not succeed:\n{out}");
    let p = page(&out);
    let (esr, far) = Reader::open(&trace).unwrap().iter().find_map(|e| match e {
        Event::Crash { esr, far, .. } => Some((*esr, *far)),
        _ => None,
    }).expect("the protected store ends the recording in an Event::Crash");
    assert_eq!(far, p, "the fault is the store into the MAP_JIT page");
    assert_eq!(esr >> 26, 0x24, "a data abort from EL0: {esr:#x}");
    assert_eq!(esr & 0x3f, 0x0f, "DFSC 0x0f, a permission fault on the ATTR_CODE page, not a translation fault: {esr:#x}");
    assert!(esr & (1 << 6) != 0, "WnR: the access was a write: {esr:#x}");
    assert_eq!(nat.status.signal(), Some(retrace_arch::signal_of_esr(esr).0 as i32),
        "the signal this crash maps to is the one native died of (a permission fault is SIGBUS, M13)");
    for n in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 139, "replay {n}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "replay {n} stdout");
    }
}

/// Spec §3j's restore parity for the JIT: a checkpoint taken inside the write-enabled window (just
/// after the guest printed its write-enabled register), continued across the protect toggle and the
/// call into the page, must equal a cold seek there. A checkpoint that lost the set or the view trips
/// `assert_jit_stamped`, or leaves the page `ATTR_DATA` so the call faults.
#[test]
fn a_seek_into_a_write_enabled_window_matches_a_cold_seek() {
    let (rec, trace) = records_and_replays("basic");
    let out = String::from_utf8(rec.stdout).unwrap();
    let write_enable = out.split("+0x110=").nth(1).and_then(|r| r.split_whitespace().next()).unwrap();
    assert!(out.contains(&format!("main write-en sprr={write_enable}\n")),
        "precondition: the window is write-enabled by the guest's own read-back:\n{out}");
    let (inside, call) = basic_landmarks(&out, &trace);
    let cp = retrace_core::seek(&trace, inside + 1, 0).unwrap().checkpoint();
    let warm = {
        let mut s = retrace_core::ReplaySession::from_checkpoint(&trace, &cp).unwrap();
        s.advance_to_landmark(call + 1).unwrap_or_else(|d| panic!("warm: diverged at {}: {}", d.landmark, d.detail));
        (s.current_thread(), s.dbg_regs(), s.dbg_fp_regs(), s.dbg_internal_state(), s.snapshot().1)
    };
    let cold = retrace_core::seek(&trace, call + 1, 0).unwrap();
    assert_eq!(warm.0, cold.current_thread(), "thread: checkpointed vs cold");
    assert_eq!(warm.1, cold.dbg_regs(), "registers: checkpointed vs cold");
    assert_eq!(warm.2, cold.dbg_fp_regs(), "FP/SIMD: checkpointed vs cold");
    assert_eq!(warm.3, cold.dbg_internal_state(), "internal state: checkpointed vs cold");
    assert!(cold.diff_memory(&warm.4).is_none(), "memory: checkpointed vs cold");
}

/// P9: single-stepping the window that holds the protect toggle runs the view flip, and so
/// `flush_guest_tlb`, inside `step()`. Before M48 that panicked (`tlbi stub faulted at EL1:
/// EC=SoftStep`), which is what `reverse-continue` into JIT code would hit. The window runs from
/// the `main write-en` line's write to the `main protect` line's: the code stores, the protect
/// toggle and its read-back.
#[test]
fn stepping_across_a_toggle_does_not_step_the_tlbi_stub() {
    let (rec, trace) = records_and_replays("basic");
    let out = String::from_utf8(rec.stdout).unwrap();
    let p = page(&out);
    let (inside, _) = basic_landmarks(&out, &trace);
    // `window_len_here` steps the whole window and spends its session (parked at the trap).
    let n = {
        let mut s = retrace_core::seek(&trace, inside + 1, 0).unwrap();
        assert_eq!(s.read_mem(p, 8).unwrap(), [0u8; 8], "precondition: the code is not written yet");
        s.window_len_here().unwrap_or_else(|e| panic!("stepping the protect toggle's window: {e}"))
    };
    assert!(n > 0, "the window holds the stores, the msr and the read-back");
    // A seek to the window's last instruction single-steps every one before it (`step_insns`), the
    // flip included, and leaves a session that can still run.
    let mut s = retrace_core::seek(&trace, inside + 1, n - 1)
        .unwrap_or_else(|e| panic!("a seek stepped across the protect toggle: {e}"));
    assert_eq!(s.read_mem(p, 8).unwrap(), [0x40, 0x05, 0x80, 0xd2, 0xc0, 0x03, 0x5f, 0xd6],
        "the stepped stores landed: mov x0, #42; ret");
    // The stepped flip left the box sound: the call into the page, protected, replays to exit 0.
    loop {
        match s.advance() {
            Ok(retrace_core::Advance::Exited(r)) => {
                assert!(matches!(r.outcome, retrace_core::Outcome::Exit { code: 0 }), "exit after the step");
                break;
            }
            Ok(_) => {}
            Err(d) => panic!("after stepping across the toggle, replay diverged at {}: {}", d.landmark, d.detail),
        }
    }
}

/// Spec §3h: `reverse-continue` to the code write, checked by its effect. The last write to the
/// page's first eight bytes is the `ret` word: before it retires the first word is there and the
/// second is not; one `stepi` later both are. The search single-steps across both toggles (P9).
#[test]
fn reverse_continue_lands_on_the_jit_code_write_by_its_effect() {
    let (rec, trace) = records_and_replays("basic");
    let p = page(&String::from_utf8_lossy(&rec.stdout));
    let (code, out, err) = util::debug_bounded(trace.to_str().unwrap(),
        &format!("continue; watch 0x{p:x} 8; reverse-continue; x 0x{p:x} 8; stepi; x 0x{p:x} 8"), 300);
    assert_eq!(code, Some(0), "debug exited {code:?} (None: killed at the bound). stderr: {err}\nstdout: {out}");
    assert!(out.contains(&format!("hit watch 0x{p:x} (write at ")), "reverse-continue must find the code write:\n{out}");
    let xs: Vec<&str> = out.lines().filter(|l| l.starts_with(&format!("0x{p:x}:"))).collect();
    assert_eq!(xs.len(), 2, "two x dumps expected:\n{out}");
    assert!(xs[0].ends_with("40 05 80 d2 00 00 00 00"), "before the store only `mov x0, #42` is there:\n{out}");
    assert!(xs[1].ends_with("40 05 80 d2 c0 03 5f d6"), "one stepi later `ret` landed:\n{out}");
}

/// §11a item 8: the static probe reaches its `msr` only if the `mrs` read 0 (R1; otherwise it
/// exits 2) and the `ic ivau` ran at EL0 (SCTLR.UCI; otherwise it stops at EC 0x18). It has no
/// commpage, so the write is refused by value (Ruling T6-f).
#[test]
fn sprrprobe_reads_zero_runs_ic_ivau_and_is_refused_at_its_write_by_value() {
    let (rec, trace) = util::record(retrace_guest::SPRRPROBE);
    let _ = std::fs::remove_file(&trace);
    assert!(!rec.stderr.contains("non-syscall exit"), "the ic ivau must run at EL0, not trap:\n{}", rec.stderr);
    assert!(rec.stderr.contains("M48: SPRR write 0x1 at pc ") && rec.stderr.contains("no SPRR commpage"),
        "the msr must be refused by value (code {}):\n{}", rec.code, rec.stderr);
    assert!(rec.code != 0 && rec.code != 2, "exit 2 would mean the mrs read nonzero; 0 that nothing was refused: {}", rec.code);
}
