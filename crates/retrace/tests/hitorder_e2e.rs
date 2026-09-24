// M41: every hit counted once, in one order, on the thread that runs it (spec
// docs/superpowers/specs/2026-09-24-retrace-m41-hitorder-design.md). Three kinds of test:
//
// - a NAMED REGRESSION for each t0 measurement (M1–M8 in the companion), pinned to exact
//   coordinates;
// - spec §3a's INVARIANT: at a blocking boundary the position shows the thread that runs next;
// - the HIT ORACLE's three checks (util::hits::check_chains) on five armings.
//
// Every address and landmark is DISCOVERED. threadrust's are shared-cache addresses, valid only for
// this host's cache. Each fixture is recorded once per test process and shared: a debug session
// only reads its trace.
mod util;
use retrace_core::{Advance, ReplaySession};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use util::hits::{self, Phase};

fn recorded(cell: &'static OnceLock<PathBuf>, guest: &str, dynamic: bool) -> &'static Path {
    cell.get_or_init(|| {
        let (rec, trace) = if dynamic { util::record_dynamic(guest) } else { util::record(guest) };
        assert_eq!(rec.code, 0, "record {guest} failed: {}", rec.stderr);
        trace
    })
}
fn ws_trace() -> &'static Path {
    static C: OnceLock<PathBuf> = OnceLock::new();
    recorded(&C, retrace_guest::WATCHSWEEP, false)
}
fn fio_trace() -> &'static Path {
    static C: OnceLock<PathBuf> = OnceLock::new();
    recorded(&C, retrace_guest::FILEIO, false)
}
fn tr_trace() -> &'static Path {
    static C: OnceLock<PathBuf> = OnceLock::new();
    recorded(&C, retrace_guest::THREADRUST, true)
}
fn ts(p: &Path) -> &str { p.to_str().unwrap() }
fn last_line(out: &str) -> &str { util::strip_annot(out.trim_end().lines().last().unwrap_or("")) }

/// The K in window `n` at which `pc` first executes.
fn k_of_pc(tp: &Path, n: usize, pc: u64) -> u64 {
    let mut s = retrace_core::seek(tp, n, 0).unwrap();
    let mut k = 0;
    while s.pc() != pc {
        s.step_insns(1).unwrap_or_else(|e| panic!("pc {pc:#x} is not in window {n}: {e}"));
        k += 1;
    }
    k
}

/// watchsweep, discovered. `t` = &buf[40]; `store` = the sweeping `str`, first run at K =
/// `first_run`, writing buf[40] at K = `k_sweep`; `second` = the second writer, at K = `k_second`.
struct Ws { t: u64, store: u64, first_run: u64, k_sweep: u64, second: u64, k_second: u64 }

fn discover_ws(tp: &Path) -> Ws {
    let t = {
        let mut s = ReplaySession::open(tp).unwrap();
        loop {
            if let Some((4, a)) = s.peek_syscall() { if a[0] == 1 { break a[1]; } }
            s.advance().unwrap();
        }
    };
    // Ground truth by memory diff, independent of every watch mechanism.
    let mut s = retrace_core::seek(tp, 1, 0).unwrap();
    let (mut prev, mut k, mut ks, mut pcs) = (s.read_mem(t, 8).unwrap(), 0u64, vec![], vec![]);
    loop {
        let pc = s.pc();
        if s.step_insns(1).is_err() { break; }
        let cur = s.read_mem(t, 8).unwrap();
        if cur != prev { ks.push(k); pcs.push(pc); prev = cur; }
        k += 1;
    }
    assert_eq!(ks.len(), 2, "watchsweep writes buf[40] exactly twice: {ks:?}");
    drop(s);
    Ws { t, store: pcs[0], first_run: k_of_pc(tp, 1, pcs[0]), k_sweep: ks[0], second: pcs[1], k_second: ks[1] }
}

/// fileio, discovered. `bpc` = the first instruction after `read`'s `svc` = (after_read, 0);
/// `read_svc` = that `svc`, at (read_win, k_read); `buf` = what `read` writes.
struct Fio { after_read: usize, read_win: usize, buf: u64, bpc: u64, read_svc: u64, k_read: u64 }

fn discover_fio(tp: &Path) -> Fio {
    let mut s = ReplaySession::open(tp).unwrap();
    let (after_read, buf, bpc) = loop {
        if let Some((3, a)) = s.peek_syscall() {
            s.advance().unwrap();
            break (s.landmark(), a[1], s.pc());
        }
        s.advance().unwrap();
    };
    let read_svc = bpc - 4; // arm64: a syscall returns to svc + 4
    drop(s);
    Fio { after_read, read_win: after_read - 1, buf, bpc, read_svc, k_read: k_of_pc(tp, after_read - 1, read_svc) }
}

/// threadrust, discovered. `n_create` = after `bsdthread_create` (360); `n_block` = after main's
/// blocking `__ulock_wait` (515); `n_exit` = after the child's `bsdthread_terminate` (361).
/// `resume` = main's resume pc, `child` = the child's first pc — read from the thread table, never
/// from `position()`: after M41 §3a the boundary holds the INCOMING thread, so ELR is not main's.
struct Tr { n_create: usize, n_block: usize, n_exit: usize, resume: u64, child: u64 }

fn pc_of(s: &ReplaySession, tid: usize) -> u64 {
    let dump = s.dbg_regs_of(tid).expect("the thread exists");
    dump.split_whitespace().find_map(|t| t.strip_prefix("pc=0x"))
        .and_then(|h| u64::from_str_radix(h, 16).ok())
        .unwrap_or_else(|| panic!("no pc in {dump}"))
}

fn discover_tr(tp: &Path) -> Tr {
    let mut s = ReplaySession::open(tp).unwrap();
    let (mut n_create, mut n_block, mut n_exit, mut resume, mut child) = (0, 0, 0, 0, 0);
    loop {
        let num = s.peek_syscall().map(|(num, _)| num);
        if let Advance::Exited(_) = s.advance().unwrap() { break; }
        match num {
            Some(360) if n_create == 0 => n_create = s.landmark(),
            Some(515) if n_block == 0 => {
                n_block = s.landmark();
                resume = pc_of(&s, 0);
                child = pc_of(&s, 1);
            }
            Some(361) if n_exit == 0 => n_exit = s.landmark(),
            _ => {}
        }
    }
    assert!(0 < n_create && n_create < n_block && n_block < n_exit,
        "threadrust: create {n_create}, block {n_block}, exit {n_exit}");
    Tr { n_create, n_block, n_exit, resume, child }
}

/// The landmark the threadrust oracle starts from: `t.n_create` (R13). Neither armed address can
/// execute before `n_create`: `t.child` is the new thread's first instruction, and `t.resume`
/// follows the trace's only 515, which comes after `n_create`. So the hit list from `n_create` is
/// the whole list. Starting at 1 single-steps through dyld's LL/SC pairs, which diverges (M41 t1,
/// `t1-diverge-diag.md`): an exclusive pair at or after the start would surface as a loud
/// `oracle: advance: Divergence`, never as a wrong hit list.
fn tr_oracle_from(t: &Tr) -> usize { t.n_create }

// ---- Named regressions (spec §4) ----------------------------------------------------------------

/// t0 M1 (M40's R11, silent): a `continue` parked on one breakpoint pre-steps onto a second. The
/// hit is AT the landing; resolving from `kctx + 1` named the next pass of the loop instead.
#[test]
fn m1_a_pre_step_that_lands_on_a_second_breakpoint_resolves_to_it() {
    let tp = ws_trace();
    let w = discover_ws(tp);
    let next = w.store + 4;
    let (code, out, err) = hits::debug(ts(tp),
        &format!("break 0x{:x}; break 0x{next:x}; continue; continue; where", w.store));
    assert_eq!(code, 0, "stderr: {err}");
    let k = w.first_run + 1;
    assert!(out.contains(&format!("resolved (1, {k})")),
        "the pre-step lands on 0x{next:x} at K={k}; t0 M1 named K={}:\n{out}", k + 5);
    assert!(last_line(&out).ends_with(&format!("at (1, {k}) pc=0x{next:x} thread=0")), "{out}");
}

/// t0 M2 (M40's R11, loud): the same shape across a boundary — a breakpoint on `read`'s `svc` and
/// one on the next instruction. It exited 5, with a message that was not a window length.
#[test]
fn m2_a_pre_step_across_a_boundary_onto_a_breakpoint_reports_it() {
    let tp = fio_trace();
    let f = discover_fio(tp);
    let (code, out, err) = hits::debug(ts(tp),
        &format!("break 0x{:x}; break 0x{:x}; continue; continue; where", f.read_svc, f.bpc));
    assert_eq!(code, 0, "t0 M2 exited 5 here; stderr: {err}\n{out}");
    assert!(out.contains(&format!("hit 0x{:x} at ({}, 0)", f.bpc, f.after_read)), "{out}");
    assert!(last_line(&out).ends_with(&format!("at ({}, 0) pc=0x{:x} thread=0", f.after_read, f.bpc)), "{out}");
}

/// t0 M3: a breakpoint on a watched store is TWO hits at one coordinate, breakpoint then watch.
/// Forward lost the watch: the pre-step re-seeked past the store with nothing armed.
#[test]
fn m3_continue_from_a_breakpoint_on_a_watched_store_reports_the_store() {
    let tp = ws_trace();
    let w = discover_ws(tp);
    let (code, out, err) = hits::debug(ts(tp), &format!(
        "watch 0x{:x}; break 0x{:x}; continue; continue; continue; where", w.t, w.second));
    assert_eq!(code, 0, "stderr: {err}");
    assert!(out.contains(&format!("resolved (1, {})", w.k_second)), "the breakpoint first:\n{out}");
    assert!(out.contains(&format!("hit watch 0x{:x} (write at 0x{:x}) at (1, {})", w.t, w.second, w.k_second)),
        "then the store under it (t0 M3 printed `exited`):\n{out}");
    assert!(!out.contains("exited"), "{out}");
}

/// t0 M4: the same pair, backward. From the store's watch hit, the breakpoint AT that coordinate
/// precedes it; "strictly before (1, K)" excluded it.
#[test]
fn m4_reverse_continue_from_a_watch_hit_finds_the_breakpoint_on_its_store() {
    let tp = ws_trace();
    let w = discover_ws(tp);
    let (code, out, err) = hits::debug(ts(tp), &format!(
        "continue; watch 0x{:x}; break 0x{:x}; reverse-continue; reverse-continue; where", w.t, w.second));
    assert_eq!(code, 0, "stderr: {err}");
    let watch = out.find(&format!("hit watch 0x{:x} (write at 0x{:x}) at (1, {})", w.t, w.second, w.k_second));
    let bp = out.find(&format!("hit 0x{:x} at (1, {})", w.second, w.k_second));
    assert!(watch.is_some() && bp.is_some() && watch < bp,
        "the watch, then the breakpoint under it (t0 M4 jumped to K={}):\n{out}", w.k_sweep);
}

/// t0 M5: a syscall write and a breakpoint at one (n, 0). The write comes first: the event ends the
/// window before. Forward lost the breakpoint: the pre-step stepped off it unreported.
#[test]
fn m5_continue_after_a_syscall_hit_reports_the_breakpoint_at_the_same_position() {
    let tp = fio_trace();
    let f = discover_fio(tp);
    let (code, out, err) = hits::debug(ts(tp), &format!(
        "watch 0x{:x}; break 0x{:x}; continue; continue; where", f.buf, f.bpc));
    assert_eq!(code, 0, "stderr: {err}");
    let sys = out.find(&format!("hit watch 0x{:x} (syscall write) at ({}, 0)", f.buf, f.after_read));
    let bp = out.find(&format!("hit 0x{:x} at ({}, 0)", f.bpc, f.after_read));
    assert!(sys.is_some() && bp.is_some() && sys < bp,
        "the write, then the breakpoint (t0 M5 printed `exited`):\n{out}");
}

/// t0 M6: the same pair, backward. From the breakpoint at (n, 0), the syscall write at (n, 0) is
/// behind it; `(n, 0) < (pn, pk)` excluded it and said "no earlier hit".
#[test]
fn m6_reverse_continue_from_a_boundary_breakpoint_finds_the_syscall_write_behind_it() {
    let tp = fio_trace();
    let f = discover_fio(tp);
    let (code, out, err) = hits::debug(ts(tp), &format!(
        "continue; watch 0x{:x}; break 0x{:x}; reverse-continue; reverse-continue; where", f.buf, f.bpc));
    assert_eq!(code, 0, "stderr: {err}");
    let bp = out.find(&format!("hit 0x{:x} at ({}, 0)", f.bpc, f.after_read));
    let sys = out.find(&format!("hit watch 0x{:x} (syscall write) at ({}, 0)", f.buf, f.after_read));
    assert!(bp.is_some() && sys.is_some() && bp < sys, "the breakpoint, then the write behind it:\n{out}");
    assert!(!out.contains("no earlier hit"), "t0 M6 said `no earlier hit`:\n{out}");
}

/// t0 M7 (R5): arriving at (n, 0) by stepping puts the cursor at the breakpoint phase, so the
/// syscall write just behind it is found. M40 excluded it for every P at k = 0; M41 excludes it
/// only for the hit itself.
#[test]
fn m7_reverse_continue_after_stepping_back_to_a_boundary_finds_the_syscall_write() {
    let tp = fio_trace();
    let f = discover_fio(tp);
    let next = f.bpc + 4;
    let (code, out, err) = hits::debug(ts(tp), &format!(
        "watch 0x{:x}; break 0x{next:x}; continue; continue; reverse-stepi; reverse-continue; where", f.buf));
    assert_eq!(code, 0, "stderr: {err}");
    let sys = format!("hit watch 0x{:x} (syscall write) at ({}, 0)", f.buf, f.after_read);
    assert_eq!(out.matches(&sys).count(), 2, "once going forward, once going back:\n{out}");
    assert!(!out.contains("no earlier hit"), "t0 M7 said `no earlier hit`:\n{out}");
}

/// t0 M8, forward: a breakpoint on main's resume pc after its blocking `__ulock_wait`. It runs once,
/// when main resumes after the child exits. At the block the boundary check read the OUTGOING
/// thread's pc and reported a phantom; at the resume the real hit was unresolvable (exit 5).
#[test]
fn m8_continue_reports_main_resuming_once_and_no_phantom_at_its_block() {
    let tp = tr_trace();
    let t = discover_tr(tp);
    let (code, out, err) = hits::debug(ts(tp), &format!("break 0x{:x}; continue; where; continue", t.resume));
    assert_eq!(code, 0, "stderr: {err}\n{out}");
    assert!(!out.contains(&format!("at ({}, ", t.n_block)), "no phantom at main's block (t0 M8):\n{out}");
    assert!(out.contains(&format!("hit 0x{:x} at ({}, 0)", t.resume, t.n_exit)), "{out}");
    assert!(out.contains(&format!("at ({}, 0) pc=0x{:x} thread=0", t.n_exit, t.resume)), "{out}");
    assert!(out.trim_end().ends_with("exited (code 0)"), "exactly one hit:\n{out}");
}

/// t0 M8, backward: the same single hit, found from the exit (it exited 5).
#[test]
fn m8_reverse_continue_finds_main_resuming_after_the_child() {
    let tp = tr_trace();
    let t = discover_tr(tp);
    let (code, out, err) = hits::debug(ts(tp),
        &format!("continue; break 0x{:x}; reverse-continue; where; reverse-continue", t.resume));
    assert_eq!(code, 0, "stderr: {err}\n{out}");
    assert!(out.contains(&format!("hit 0x{:x} at ({}, 0)", t.resume, t.n_exit)), "{out}");
    assert!(out.trim_end().ends_with("no earlier hit"), "exactly one hit:\n{out}");
}

/// t0 M8, the missed-hit shape: a breakpoint on the child's first instruction, which runs at main's
/// block (it exited 5).
#[test]
fn m8_continue_reports_the_childs_first_instruction_on_the_child() {
    let tp = tr_trace();
    let t = discover_tr(tp);
    let (code, out, err) = hits::debug(ts(tp), &format!("break 0x{:x}; continue; where", t.child));
    assert_eq!(code, 0, "stderr: {err}\n{out}");
    assert!(out.contains(&format!("hit 0x{:x} at ({}, 0)", t.child, t.n_block)), "{out}");
    assert!(last_line(&out).ends_with(&format!("at ({}, 0) pc=0x{:x} thread=1", t.n_block, t.child)), "{out}");
}

// ---- Spec §3a's invariant -------------------------------------------------------------------------

/// At the boundary after main blocks, the position already shows the thread that retires the next
/// instruction. M15 defined it as the thread that had just blocked.
#[test]
fn at_a_blocking_boundary_the_position_shows_the_thread_that_runs_next() {
    let tp = tr_trace();
    let t = discover_tr(tp);
    let s = retrace_core::seek(tp, t.n_block, 0).unwrap();
    let sum = s.thread_summaries();
    assert!(matches!(sum[0].state, retrace_core::ThreadState::Blocked(_)), "main has blocked: {sum:?}");
    assert_eq!(s.current_thread(), 1, "so the child runs next (M15 said 0 here): {sum:?}");
    assert_eq!(s.pc(), t.child, "from its first instruction");
    assert!(sum[1].is_current && !sum[0].is_current, "{sum:?}");
}

// ---- The hit oracle (spec §3e) ------------------------------------------------------------------

#[test]
fn oracle_watchsweep_two_adjacent_breakpoints() {
    let tp = ws_trace();
    let w = discover_ws(tp);
    let bps = [w.store, w.store + 4];
    let hits = hits::enumerate_hits(tp, &bps, &[], 1);
    // Self-check from the fixture's source: 64 passes through the loop, once through each.
    assert_eq!(hits.len(), 128, "{hits:?}");
    assert!(hits.iter().all(|h| (h.n, h.phase, h.thread) == (1, Phase::Bp, 0)), "{hits:?}");
    assert_eq!((hits[0].k, hits[1].k, hits[2].k), (w.first_run, w.first_run + 1, w.first_run + 5));
    hits::check_chains(ts(tp), &format!("break 0x{:x}; break 0x{:x}", bps[0], bps[1]), &hits);
}

#[test]
fn oracle_watchsweep_a_breakpoint_on_a_watched_store() {
    let tp = ws_trace();
    let w = discover_ws(tp);
    let hits = hits::enumerate_hits(tp, &[w.second], &[(w.t, 8)], 1);
    let shape: Vec<(u64, Phase)> = hits.iter().map(|h| (h.k, h.phase)).collect();
    assert_eq!(shape, vec![(w.k_sweep, Phase::Watch), (w.k_second, Phase::Bp), (w.k_second, Phase::Watch)],
        "{hits:?}");
    hits::check_chains(ts(tp), &format!("watch 0x{:x}; break 0x{:x}", w.t, w.second), &hits);
}

#[test]
fn oracle_fileio_breakpoints_either_side_of_a_boundary() {
    let tp = fio_trace();
    let f = discover_fio(tp);
    let hits = hits::enumerate_hits(tp, &[f.read_svc, f.bpc], &[], 1);
    let shape: Vec<(usize, u64, Phase)> = hits.iter().map(|h| (h.n, h.k, h.phase)).collect();
    assert_eq!(shape, vec![(f.read_win, f.k_read, Phase::Bp), (f.after_read, 0, Phase::Bp)], "{hits:?}");
    hits::check_chains(ts(tp), &format!("break 0x{:x}; break 0x{:x}", f.read_svc, f.bpc), &hits);
}

#[test]
fn oracle_fileio_a_syscall_write_and_a_breakpoint_at_one_boundary() {
    let tp = fio_trace();
    let f = discover_fio(tp);
    let hits = hits::enumerate_hits(tp, &[f.bpc], &[(f.buf, 8)], 1);
    let shape: Vec<(usize, u64, Phase)> = hits.iter().map(|h| (h.n, h.k, h.phase)).collect();
    assert_eq!(shape, vec![(f.after_read, 0, Phase::Sys), (f.after_read, 0, Phase::Bp)], "{hits:?}");
    hits::check_chains(ts(tp), &format!("watch 0x{:x}; break 0x{:x}", f.buf, f.bpc), &hits);
}

#[test]
fn oracle_threadrust_breakpoints_at_both_switches() {
    let tp = tr_trace();
    let t = discover_tr(tp);
    let hits = hits::enumerate_hits(tp, &[t.resume, t.child], &[], tr_oracle_from(&t));
    let shape: Vec<(usize, u64, Phase, u64, u32)> = hits.iter().map(|h| (h.n, h.k, h.phase, h.pc, h.thread)).collect();
    assert_eq!(shape, vec![(t.n_block, 0, Phase::Bp, t.child, 1), (t.n_exit, 0, Phase::Bp, t.resume, 0)],
        "the child's first instruction on the child at main's block, then main resuming: {hits:?}");
    hits::check_chains(ts(tp), &format!("break 0x{:x}; break 0x{:x}", t.resume, t.child), &hits);
}
