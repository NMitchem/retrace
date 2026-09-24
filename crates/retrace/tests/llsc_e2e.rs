//! M42: stepping, seeking and debug stops inside AArch64 exclusive pairs (spec
//! `docs/superpowers/specs/2026-09-24-retrace-m42-llsc-design.md`). The fixture is
//! `retrace-guest/asm/llsc.s`: t0's four shapes (a)-(d) verbatim, then (e)-(i). Every CLI run goes
//! through `util::debug_bounded`, because t0 measured hangs on these scripts.
mod util;
use retrace_core::{Advance, Outcome, ReplaySession, SetBy};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use util::hits::{self, Phase};

/// Seconds before a debugger run is killed. t0 killed its hangs at 60 s; a green run takes under one.
const BOUND: u64 = 60;
/// What the guest writes, in order: one letter per shape that ends in a `write`.
const STDOUT: &[u8] = b"a\nb\nc\nd\ne\nf\ng\nh\n";

fn trace() -> &'static Path {
    static C: OnceLock<PathBuf> = OnceLock::new();
    C.get_or_init(|| {
        let (rec, t) = util::record(retrace_guest::LLSC);
        assert_eq!(rec.code, 0, "record llsc: {}", rec.stderr);
        t
    })
}
fn ts() -> &'static str { trace().to_str().unwrap() }

/// A fixture label's address, from `nm`. The labels are local symbols in LC_SYMTAB.
fn sym(name: &str) -> u64 {
    static M: OnceLock<HashMap<String, u64>> = OnceLock::new();
    let m = M.get_or_init(|| {
        let out = std::process::Command::new("nm").arg(retrace_guest::LLSC).output().expect("nm");
        assert!(out.status.success(), "nm llsc: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().lines().filter_map(|l| {
            let mut f = l.split_whitespace();
            let (a, _kind, n) = (f.next()?, f.next()?, f.next()?);
            Some((n.to_string(), u64::from_str_radix(a, 16).ok()?))
        }).collect()
    });
    *m.get(name).unwrap_or_else(|| panic!("no symbol {name} in llsc"))
}
fn h(name: &str) -> String { format!("{:#x}", sym(name)) }

/// A script's stdout. It must exit 0 inside the bound; anything else panics, naming the symptom.
fn run_ok(script: &str) -> String {
    let (code, out, err) = util::debug_bounded(ts(), script, BOUND);
    match code {
        Some(0) => out,
        None => panic!("killed at the {BOUND} s bound (t0's hang)\nscript: {script}\n{out}"),
        Some(c) => panic!("exit {c}\nscript: {script}\nstderr: {err}\n{out}"),
    }
}
/// The `where` answers of a transcript, annotations stripped, in order.
fn wheres(out: &str) -> Vec<&str> {
    out.lines().filter(|l| l.starts_with("at (")).map(util::strip_annot).collect()
}
fn has_line(out: &str, want: &str) -> bool { out.lines().map(util::strip_annot).any(|l| l == want) }
fn resolved(out: &str) -> Vec<String> {
    out.lines().filter(|l| l.starts_with("resolved (")).map(str::to_string).collect()
}

/// Seek to (n, k), then replay forward to the end: the outcome and the guest's stdout.
fn seek_then_finish(n: usize, k: u64) -> Result<(Outcome, Vec<u8>), String> {
    let mut s = retrace_core::seek(trace(), n, k)?;
    loop {
        if let Advance::Exited(r) = s.advance()
            .map_err(|d| format!("diverged at landmark {}: {}", d.landmark, d.detail))? {
            return Ok((r.outcome, r.stdout));
        }
    }
}

/// Every (n, k) with k = 0..=len replays to the recorded end, and k = len + 1 is refused by naming
/// exactly `len`. That pins the STEPPED length to the recorded path (t0: window 1 measured 9, not 16).
fn every_position_replays(n: usize, len: u64) {
    for k in 0..=len {
        let (o, out) = seek_then_finish(n, k).unwrap_or_else(|e| panic!("({n}, {k}): {e}"));
        assert_eq!(o, Outcome::Exit { code: 0 }, "({n}, {k})");
        assert_eq!(out, STDOUT, "({n}, {k}): stdout");
    }
    let e = retrace_core::seek(trace(), n, len + 1).map(|_| ()).unwrap_err();
    assert!(e.contains(&format!("window {n} ends after {len} instruction(s)")), "({n}, {}): {e}", len + 1);
}

/// `break at`, then `continue` once per expected hit and once more: each hit resolves at its
/// coordinate, and the answer after the last is the end.
fn break_forward(at: &str, want: &[(usize, u64)]) {
    let out = run_ok(&format!("break {}; {}", h(at), "continue; ".repeat(want.len() + 1)));
    let exp: Vec<String> = want.iter().map(|(n, k)| format!("resolved ({n}, {k})")).collect();
    assert_eq!(resolved(&out), exp, "{out}");
    assert!(out.contains("exited (code 0)"), "{out}");
}
/// From the end, `break at`, then `reverse-continue` once per expected hit and once more.
fn break_backward(at: &str, want: &[(usize, u64)]) {
    let out = run_ok(&format!("continue; break {}; {}", h(at), "reverse-continue; ".repeat(want.len() + 1)));
    let got: Vec<&str> = out.lines().filter(|l| l.starts_with("hit 0x")).map(util::strip_annot).collect();
    let exp: Vec<String> = want.iter().rev().map(|(n, k)| format!("hit {} at ({n}, {k})", h(at))).collect();
    assert_eq!(got, exp, "{out}");
    assert!(out.contains("no earlier hit"), "{out}");
}
/// `watch cell+off len`, then `continue` once per expected hit and once more. Every hit is the
/// store-exclusive `stx`.
fn watch_forward(cell: &str, off: u64, len: u64, stx: &str, want: &[(usize, u64)]) {
    let a = sym(cell) + off;
    let out = run_ok(&format!("watch {a:#x} {len}; {}", "continue; ".repeat(want.len() + 1)));
    assert_eq!(out.matches(&format!("hit watch {a:#x} (write at {})", h(stx))).count(), want.len(), "{out}");
    let exp: Vec<String> = want.iter().map(|(n, k)| format!("resolved ({n}, {k})")).collect();
    assert_eq!(resolved(&out), exp, "{out}");
    assert!(out.contains("exited (code 0)"), "{out}");
}
/// From the end, `watch cell+off len`, then `reverse-continue` once per expected hit and once more.
fn watch_backward(cell: &str, off: u64, len: u64, stx: &str, want: &[(usize, u64)]) {
    let a = sym(cell) + off;
    let out = run_ok(&format!("continue; watch {a:#x} {len}; {}", "reverse-continue; ".repeat(want.len() + 1)));
    let got: Vec<&str> = out.lines().filter(|l| l.starts_with("hit watch")).map(util::strip_annot).collect();
    let exp: Vec<String> = want.iter().rev()
        .map(|(n, k)| format!("hit watch {a:#x} (write at {}) at ({n}, {k})", h(stx))).collect();
    assert_eq!(got, exp, "{out}");
    assert!(out.contains("no earlier hit"), "{out}");
}

/// The recording's own landmarks: what each shape did natively. (e), (f) and (g) publish a FAILED
/// store (x3 = 1), because each puts an exit between its halves: `clrex`, a trapped timebase read,
/// a syscall. This test failing on (e), (f) or (g) is spec §7's first halt rule.
#[test]
fn the_recording_holds_what_each_shape_did_natively() {
    let mut s = ReplaySession::open(trace()).unwrap();
    let mut got = Vec::new();
    let outcome = loop {
        if let Some((num, a)) = s.peek_syscall() { got.push((s.landmark(), num, a[3], a[4])); }
        if let Advance::Exited(r) = s.advance().unwrap() { break r.outcome; }
    };
    assert_eq!(got, vec![
        (1, 4, 0x4242, 0), // (a) the cell filled; no getpid
        (2, 4, 3, 3),      // (b) counter 3 in 3 entries
        (3, 4, 9, 1),      // (c) cas 9 in 1 entry
        (4, 4, 11, 1),     // (d) pair[0] 11 in 1 entry
        (5, 4, 1, 0),      // (e) clrex: status 1, cell untouched
        (6, 4, 1, 0),      // (f) trapped timebase read: status 1
        (7, 20, 1, 0),     // (g) the getpid between the halves (x3/x4 still (f)'s)
        (8, 4, 1, 0),      // (g) status 1
        (9, 4, 0x4242, 0), // (h) no store: the cell still holds (a)'s value
    ]);
    assert_eq!(outcome, Outcome::Exit { code: 0 }, "(i): one entry, so exit(entries - 1) = 0");
}

// ---- Named regressions (spec §4), each RED on 1d95a93 ------------------------------------------

/// t0 M2: `stepi` past (a)'s `ldxr`, then `continue`. It diverged, with an extra `getpid` at
/// landmark 1. Green at Task 3. The final `where` is the terminal park on (i)'s window, which
/// single-steps a retry loop (Review Focus 5).
#[test]
fn m2_stepi_past_the_ldxr_then_continue_replays_to_the_end() {
    let out = run_ok("stepi 4; where; continue; where");
    let w = wheres(&out);
    assert_eq!(w.first().copied(), Some(format!("at (1, 4) pc={:#x} thread=0", sym("a_ldx") + 4).as_str()), "{out}");
    assert!(out.contains("exited (code 0)"), "{out}");
    assert_eq!(w.last().copied(), Some(format!("at (10, 10) pc={} thread=0", h("i_svc")).as_str()), "{out}");
}

/// t0 M3a: `stepi` through (b)'s retry loop. It livelocked: the counter never moved. Green at Task 3.
#[test]
fn m3a_stepi_through_the_retry_loop_reaches_b_done() {
    let out = run_ok(&format!("break {0}; continue; delete {0}; stepi 25; where; x {1} 8", h("b_start"), h("ctr")));
    assert_eq!(wheres(&out).last().copied(), Some(format!("at (2, 25) pc={} thread=0", h("b_done")).as_str()), "{out}");
    assert!(has_line(&out, &format!("{}: 03 00 00 00 00 00 00 00", h("ctr"))), "{out}");
}

/// t0 M3b: `continue` to a breakpoint after the loop. It hung. Green at Task 3.
#[test]
fn m3b_continue_to_a_breakpoint_after_the_loop() {
    let out = run_ok(&format!("break {}; continue; where", h("b_done")));
    assert_eq!(resolved(&out), vec!["resolved (2, 25)".to_string()], "{out}");
}

/// t0 M3c: `reverse-continue` to a breakpoint after the loop. It hung. Green at Task 3.
#[test]
fn m3c_reverse_continue_to_a_breakpoint_after_the_loop() {
    let out = run_ok(&format!("break {0}; continue; delete {0}; break {1}; reverse-continue; where",
        h("c_start"), h("b_done")));
    assert!(has_line(&out, &format!("hit {} at (2, 25)", h("b_done"))), "{out}");
}

/// t0 M3d: `reverse-stepi` into window 2 measures its length by stepping. It hung. Green at Task 3.
#[test]
fn m3d_reverse_stepi_into_the_loop_window_lands_on_its_svc() {
    let out = run_ok(&format!("break {0}; continue; delete {0}; reverse-stepi; where", h("c_start")));
    assert_eq!(wheres(&out).last().copied(), Some(format!("at (2, 33) pc={} thread=0", h("b_svc")).as_str()), "{out}");
}

/// t0's (1, 9) cross-check: `reverse-stepi` from (2, 0) parked on a `getpid` the recording never
/// ran, and the next `continue` diverged. Green at Task 3.
#[test]
fn m3d_prime_reverse_stepi_measures_window_1_on_the_recorded_path() {
    let out = run_ok(&format!("break {0}; continue; delete {0}; reverse-stepi; where; continue", h("b_start")));
    assert_eq!(wheres(&out).last().copied(), Some(format!("at (1, 16) pc={} thread=0", h("a_svc")).as_str()), "{out}");
    assert!(out.contains("exited (code 0)"), "{out}");
}

/// t0 M3e: a `reverse-continue` whose phase 2 steps window 2. It answered `no earlier hit`, silently,
/// with exit 0. Green at Task 3.
#[test]
fn m3e_reverse_continue_whose_phase_2_steps_the_loop() {
    let out = run_ok(&format!("break {0}; continue; delete {0}; stepi 30; where; break {1}; reverse-continue; where",
        h("b_start"), h("b_done")));
    assert_eq!(wheres(&out)[0], format!("at (2, 30) pc={:#x} thread=0", sym("b_done") + 0x14), "{out}");
    assert!(has_line(&out, &format!("hit {} at (2, 25)", h("b_done"))), "{out}");
    // `reverse-continue` re-seeks to the hit it reports, so the second `where` is on it.
    assert_eq!(wheres(&out)[1], format!("at (2, 25) pc={} thread=0", h("b_done")), "{out}");
}

/// t0 M4: a breakpoint on (a)'s discard-status `stxr`. It diverged. Forward: green at Task 4 or 5.
#[test]
fn m4_break_on_the_discard_status_store_forward() { break_forward("a_stx", &[(1, 5)]); }
/// Backward: green at Task 5.
#[test]
fn m4_break_on_the_discard_status_store_backward() { break_backward("a_stx", &[(1, 5)]); }
/// t0 M4d/M4e: a breakpoint on (b)'s `stlxr`. Forward gave phantom hits at (2, 12), (2, 17)…;
/// backward hung. Forward: green at Task 4 or 5.
#[test]
fn m4_break_on_the_retry_store_forward() { break_forward("b_stx", &[(2, 7), (2, 14), (2, 21)]); }
/// Backward: green at Task 5.
#[test]
fn m4_break_on_the_retry_store_backward() { break_backward("b_stx", &[(2, 7), (2, 14), (2, 21)]); }

/// t0 M5: a watch on (b)'s counter. Forward gave phantom hits that never ended; backward hung.
/// Forward: green at Task 4 or 5.
#[test]
fn m5_watch_the_retry_counter_forward() { watch_forward("ctr", 0, 8, "b_stx", &[(2, 7), (2, 14), (2, 21)]); }
/// Backward: green at Task 5.
#[test]
fn m5_watch_the_retry_counter_backward() { watch_backward("ctr", 0, 8, "b_stx", &[(2, 7), (2, 14), (2, 21)]); }
/// t0 M6: the same on (c), (d)'s second element, and (a). Forward: green at Task 4 or 5.
#[test]
fn m6_watch_the_cas_cell_forward() { watch_forward("cas", 0, 8, "c_stx", &[(3, 9)]); }
/// Backward: green at Task 5.
#[test]
fn m6_watch_the_cas_cell_backward() { watch_backward("cas", 0, 8, "c_stx", &[(3, 9)]); }
/// Forward: green at Task 4 or 5.
#[test]
fn m6_watch_the_pairs_second_element_forward() { watch_forward("pair", 8, 8, "d_stx", &[(4, 7)]); }
/// Backward: green at Task 5.
#[test]
fn m6_watch_the_pairs_second_element_backward() { watch_backward("pair", 8, 8, "d_stx", &[(4, 7)]); }
/// Forward: green at Task 4 or 5.
#[test]
fn m6_watch_the_discard_status_cell_forward() { watch_forward("cella", 0, 4, "a_stx", &[(1, 5)]); }
/// Backward: green at Task 5.
#[test]
fn m6_watch_the_discard_status_cell_backward() { watch_backward("cella", 0, 4, "a_stx", &[(1, 5)]); }

/// t0 M7: seek to every K of windows 1-4, then replay to the end. Every K from LDX+1 through STX+1
/// diverged. Green at Task 3.
#[test]
fn m7_every_position_of_windows_1_to_4_replays_to_the_end() {
    for (n, len) in [(1, 16), (2, 33), (3, 19), (4, 16)] { every_position_replays(n, len); }
}

/// t0 M7's CLI form, `stepi K; continue`, at each pair's LDX+1 and STX+1. Green at Task 3.
#[test]
fn m7_stepi_into_each_pair_then_continue_replays_to_the_end() {
    for (start, ldx_k, stx_k) in [(None, 3u64, 5u64), (Some("b_start"), 5, 7), (Some("c_start"), 6, 9), (Some("d_start"), 4, 7)] {
        for k in [ldx_k + 1, stx_k + 1] {
            let reach = start.map(|s| format!("break {0}; continue; delete {0}; ", h(s))).unwrap_or_default();
            let out = run_ok(&format!("{reach}stepi {k}; continue; where"));
            assert!(out.contains("exited (code 0)"), "{start:?} K={k}:\n{out}");
            assert_eq!(wheres(&out).last().copied(), Some(format!("at (10, 10) pc={} thread=0", h("i_svc")).as_str()),
                "{start:?} K={k}:\n{out}");
        }
    }
}

// ---- The clear rule's positive controls (spec §4) ----------------------------------------------
// Green on 1d95a93 and after: stepping these pairs fails their stores natively, exactly as record
// did. Task 3 shows each one ABLE to fail by deleting the clear it guards.

/// (e): `clrex` between the halves.
#[test]
fn control_e_every_position_of_the_clrex_window_replays() { every_position_replays(5, 14); }
/// (f): a trapped timebase read between the halves.
#[test]
fn control_f_every_position_of_the_trapped_read_window_replays() { every_position_replays(6, 14); }
/// (g): a syscall between the halves, across windows 7 and 8.
#[test]
fn control_g_every_position_across_the_syscall_replays() {
    every_position_replays(7, 5);
    every_position_replays(8, 9);
}

/// (h) and (i). Window 10's retry loop diverges on 1d95a93 (red), and window 9 is green. Green at
/// Task 3.
#[test]
fn every_position_of_the_branch_out_and_exit_windows_replays() {
    every_position_replays(9, 13);
    every_position_replays(10, 10);
}

// ---- The shadow's life cycle (spec §3a), at the session level ----------------------------------

/// (e): the stepped `ldxr` sets the shadow, and `clrex` clears it.
#[test]
fn the_shadow_is_set_by_a_stepped_ldx_and_cleared_by_clrex() {
    let mut s = retrace_core::seek(trace(), 5, 4).unwrap(); // (e)'s ldxr (K=3) has retired
    assert_eq!(s.pc(), sym("e_clrex"));
    let ex = s.dbg_excl().expect("set by the stepped ldxr");
    assert_eq!((ex.va, ex.size, ex.pair, ex.by), (sym("celle"), 4, false, SetBy::Stepped));
    s.step_insns(1).unwrap(); // clrex
    assert_eq!(s.dbg_excl(), None, "clrex clears the shadow");
}

/// (h): a branch out of the sequence leaves the monitor set, and so the shadow too, until the next
/// non-debug exit.
#[test]
fn the_shadow_outlives_a_branch_out_until_the_next_syscall() {
    let mut s = retrace_core::seek(trace(), 9, 4).unwrap(); // (h)'s ldxr has retired
    assert!(s.dbg_excl().is_some());
    s.step_insns(1).unwrap(); // cbnz, taken: no stxr follows
    assert_eq!(s.pc(), sym("h_after"));
    assert!(s.dbg_excl().is_some(), "the hardware monitor stays set past a branch-out, and so does the shadow");
    assert!(matches!(s.advance().unwrap(), Advance::Event)); // run() steps to the write svc and crosses it
    assert_eq!(s.dbg_excl(), None, "the syscall exit clears it");
}

/// (g): a syscall between the halves clears the shadow, so the store in window 8 fails as it did
/// natively.
#[test]
fn a_syscall_between_the_halves_clears_the_shadow() {
    let mut s = retrace_core::seek(trace(), 7, 4).unwrap(); // (g)'s ldxr has retired
    assert!(s.dbg_excl().is_some());
    assert!(matches!(s.advance().unwrap(), Advance::Event)); // the getpid
    assert_eq!(s.dbg_excl(), None, "the syscall exit clears the shadow");
    assert_eq!(s.pc(), sym("g_stx"));
}

/// (g), at the park: stepping the `svc` leaves the guest at EL1 on the unconsumed trap (the
/// debugger's `Stepped::AtTrap`), and the shadow must already be clear THERE, before any
/// `advance()`. The two tests above read the shadow only after `advance()`, where `run()`'s own
/// post-prologue drop would clear it anyway, so neither can see this clear go missing. If a shadow
/// survived this park, the next `run()` would single-step the guest at EL1.
#[test]
fn the_syscall_trap_clears_the_shadow_before_the_trap_is_consumed() {
    let mut s = retrace_core::seek(trace(), 7, 4).unwrap(); // (g)'s ldxr has retired
    assert!(s.dbg_excl().is_some(), "precondition: set by the stepped ldxr");
    // The `mov x16` retires; stepping the `svc` then ends the window, unconsumed, parked at EL1.
    let e = s.step_insns(2).unwrap_err();
    assert!(e.contains("window 7 ends after"), "{e}");
    assert_eq!(s.dbg_excl(), None, "the trapped svc's step exit clears the shadow at the park");
    // The park is still usable: the trap is consumed and the run finishes as recorded.
    let (outcome, out) = loop {
        if let Advance::Exited(r) = s.advance().unwrap() { break (r.outcome, r.stdout); }
    };
    assert_eq!(outcome, Outcome::Exit { code: 0 });
    assert_eq!(out, STDOUT);
}

// ---- Review Focus (plan) -----------------------------------------------------------------------

/// Review Focus 1: step into the pair, step back, then continue. `reverse-stepi` re-seeks by
/// stepping, which recomputes the shadow from scratch.
#[test]
fn reverse_stepi_inside_a_pair_then_continue_replays_to_the_end() {
    let out = run_ok("stepi 5; reverse-stepi; where; continue; where");
    assert_eq!(wheres(&out)[0], format!("at (1, 4) pc={:#x} thread=0", sym("a_ldx") + 4), "{out}");
    assert!(out.contains("exited (code 0)"), "{out}");
}

/// Review Focus 2: a checkpoint captured inside a pair carries the shadow, and a session restored
/// from it finishes the pair's store. The research's §4c: without the shadow, every position after
/// a stepped load-exclusive is poison.
#[test]
fn a_checkpoint_taken_inside_a_pair_restores_the_shadow() {
    let s = retrace_core::seek(trace(), 2, 6).unwrap(); // (b)'s first ldaxr (K=5) has retired
    let ex = s.dbg_excl().expect("set by the stepped ldaxr");
    let cp = s.checkpoint();
    drop(s); // one VM per process
    let mut r = ReplaySession::from_checkpoint(trace(), &cp).unwrap();
    assert_eq!(r.dbg_excl(), Some(ex));
    loop {
        if let Advance::Exited(rep) = r.advance().unwrap() {
            assert_eq!(rep.outcome, Outcome::Exit { code: 0 });
            break;
        }
    }
}

/// Review Focus 3: `x` shows the cell before and after the emulated store.
#[test]
fn x_shows_the_cell_before_and_after_the_emulated_store() {
    let out = run_ok(&format!("stepi 5; x {0} 4; stepi; x {0} 4", h("cella")));
    assert!(has_line(&out, &format!("{}: 00 00 00 00", h("cella"))), "before the stxr:\n{out}");
    assert!(has_line(&out, &format!("{}: 42 42 00 00", h("cella"))), "after the stxr:\n{out}");
}

// ---- The hit oracle's ground truth on llsc (spec §4) ------------------------------------------
// `enumerate_hits` single-steps from (1, 0) with everything armed, so it steps every pair. Each
// list below is derived from the fixture SOURCE, not from the emulator: a Bp comes before a Watch
// at one coordinate (M41 §3b).

fn keys(hs: &[hits::Hit]) -> Vec<(usize, u64, Phase, u64)> { hs.iter().map(|h| (h.n, h.k, h.phase, h.pc)).collect() }

/// A1: {b_stx, i_stx} + watch ctr. (b)'s stlxr at K = 7, 14, 21 is a breakpoint, then a watched
/// store, each time. (i)'s stlxr at (10, 6) is a breakpoint on an unwatched cell.
fn a1() -> (Vec<u64>, Vec<(u64, u64)>) { (vec![sym("b_stx"), sym("i_stx")], vec![(sym("ctr"), 8)]) }
/// A2: watch pair+8 + break i_svc. The stxp at (4, 7) covers pair[1]; the exit svc is (10, 10).
fn a2() -> (Vec<u64>, Vec<(u64, u64)>) { (vec![sym("i_svc")], vec![(sym("pair") + 8, 8)]) }
/// A3: {a_ldx, a_stx, g_stx, i_svc} + watch cella. The breakpoint on the LOAD itself comes first,
/// then (a)'s stxr (a Bp and a Watch), then g_stx at (8, 0), where the syscall has cleared the
/// shadow so the hardware breakpoint fires, then the exit svc. (h) stores nothing.
fn a3() -> (Vec<u64>, Vec<(u64, u64)>) {
    (vec![sym("a_ldx"), sym("a_stx"), sym("g_stx"), sym("i_svc")], vec![(sym("cella"), 4)])
}

#[test]
fn oracle_a1_lists_every_hit_the_source_implies() {
    let (bps, ws) = a1();
    let b = sym("b_stx");
    assert_eq!(keys(&hits::enumerate_hits(trace(), &bps, &ws, 1)), vec![
        (2, 7, Phase::Bp, b), (2, 7, Phase::Watch, b), (2, 14, Phase::Bp, b), (2, 14, Phase::Watch, b),
        (2, 21, Phase::Bp, b), (2, 21, Phase::Watch, b), (10, 6, Phase::Bp, sym("i_stx")),
    ]);
}

#[test]
fn oracle_a2_lists_every_hit_the_source_implies() {
    let (bps, ws) = a2();
    assert_eq!(keys(&hits::enumerate_hits(trace(), &bps, &ws, 1)),
        vec![(4, 7, Phase::Watch, sym("d_stx")), (10, 10, Phase::Bp, sym("i_svc"))]);
}

#[test]
fn oracle_a3_lists_every_hit_the_source_implies() {
    let (bps, ws) = a3();
    let a = sym("a_stx");
    assert_eq!(keys(&hits::enumerate_hits(trace(), &bps, &ws, 1)), vec![
        (1, 3, Phase::Bp, sym("a_ldx")), (1, 5, Phase::Bp, a), (1, 5, Phase::Watch, a),
        (8, 0, Phase::Bp, sym("g_stx")), (10, 10, Phase::Bp, sym("i_svc")),
    ]);
}
