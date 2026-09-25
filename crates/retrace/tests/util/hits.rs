//! M41: the debugger's ground truth — every hit a recording holds for one arming, found by brute
//! force — and the three checks that compare the debugger's own answers against it (spec §3e).
//!
//! Deliberately shares none of the debugger's hit-finding machinery: no `resolve_nth`, no
//! pre-step, no scan/resolve split, no pc-based counting. Every hit is a stop taken while
//! single-stepping with everything armed (`ReplaySession::step_armed`), read AFTER the stop. It is
//! a HARDWARE stop, except at an emulated store-exclusive (since M42), where it is the stop
//! `Box_::raise_debug_stop` raises in the hardware's place. The debugger shares that function.
//! `Box_::step()` switches threads on entry, so the oracle sees the running thread whether or not
//! M41 §3a's settle is in place: it is independent of the fix it checks.
//!
//! Since M42 a pair no longer limits the oracle. Stepping an exclusive pair keeps its store
//! (`Box_`'s shadow monitor), and the raised stops come in hardware order. Because the oracle gets
//! those stops from the same `raise_debug_stop` the debugger does, it cannot check them
//! independently. What pins them independently is `llsc_e2e`'s ground-truth lists, derived from the
//! fixture source.
use retrace_core::{Advance, Armed, ReplaySession};
use std::path::Path;

/// Where a hit sits within one coordinate (n, k), in the hardware's order (spec §3b): a syscall's
/// write (k = 0 only), then a breakpoint, then a watched store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase { Sys, Bp, Watch }

/// One hit. For `Sys`, `pc` and `thread` are whatever the session shows at (n, 0) and are never
/// compared: a syscall's write has no instruction (see `Key`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit { pub n: usize, pub k: u64, pub phase: Phase, pub pc: u64, pub thread: u32 }

/// What the checks compare: the coordinate and phase always; pc and thread for an instruction hit
/// (both read from the `where` after the answer). NOT compared: which watch range a hit line named
/// (`hit watch <addr> …`) or its `write at` pc. That is sound while every arming watches one
/// range; an arming with two would need a `watched` field here, or a hit on the wrong range
/// would pass.
pub type Key = (usize, u64, Phase, Option<(u64, u32)>);

impl Hit {
    pub fn key(&self) -> Key {
        (self.n, self.k, self.phase, (self.phase != Phase::Sys).then_some((self.pc, self.thread)))
    }
}

/// Every hit of `bps` and `ws` from `(from_n, 0)` to the end of the recording, in order. Panics on
/// a guest fault: no oracle fixture faults, and a fault's crossing is not part of this proof.
pub fn enumerate_hits(trace: &Path, bps: &[u64], ws: &[(u64, u64)], from_n: usize) -> Vec<Hit> {
    let mut s = retrace_core::seek(trace, from_n, 0).expect("oracle: seek");
    let mut hits = Vec::new();
    let (mut n, mut k) = (from_n, 0u64);
    s.arm_breakpoints(bps);
    s.arm_watchpoints(ws);
    loop {
        match s.step_armed().expect("oracle: armed step") {
            Armed::Retired => k += 1,
            Armed::Break => {
                hits.push(Hit { n, k, phase: Phase::Bp, pc: s.pc(), thread: s.current_thread() });
                // Step the breakpointed instruction with the watches still armed: it may be a
                // watched store too, and then it is a second hit at the same coordinate.
                s.clear_breakpoints();
                match s.step_armed().expect("oracle: step off a breakpoint") {
                    Armed::Retired => k += 1,
                    Armed::Watch => {
                        hits.push(Hit { n, k, phase: Phase::Watch, pc: s.pc(), thread: s.current_thread() });
                        step_over(&mut s, ws);
                        k += 1;
                    }
                    Armed::AtTrap => {
                        if cross(&mut s, &mut hits) { return hits; }
                        (n, k) = (s.landmark(), 0);
                    }
                    Armed::Fault => panic!("oracle: a guest fault at ({n}, {k})"),
                    Armed::Break => unreachable!("breakpoints are disarmed"),
                }
                s.arm_breakpoints(bps);
            }
            Armed::Watch => {
                hits.push(Hit { n, k, phase: Phase::Watch, pc: s.pc(), thread: s.current_thread() });
                s.clear_breakpoints();
                step_over(&mut s, ws);
                s.arm_breakpoints(bps);
                k += 1;
            }
            Armed::AtTrap => {
                s.clear_breakpoints();
                if cross(&mut s, &mut hits) { return hits; }
                (n, k) = (s.landmark(), 0);
                s.arm_breakpoints(bps);
            }
            Armed::Fault => panic!("oracle: a guest fault at ({n}, {k})"),
        }
    }
}

/// Retire a watched store with nothing watched, then re-arm. A second arm without a clear would
/// duplicate the session's syscall-watch ranges.
fn step_over(s: &mut ReplaySession, ws: &[(u64, u64)]) {
    s.clear_watchpoints();
    s.step_insns(1).expect("oracle: step over a watched store");
    s.arm_watchpoints(ws);
}

/// Consume the event the guest is parked at, watches armed and breakpoints off. True at the end.
fn cross(s: &mut ReplaySession, hits: &mut Vec<Hit>) -> bool {
    match s.advance().expect("oracle: advance") {
        Advance::Exited(_) => true,
        Advance::WatchSyscall { .. } => {
            hits.push(Hit { n: s.landmark(), k: 0, phase: Phase::Sys, pc: s.pc(), thread: s.current_thread() });
            false
        }
        Advance::Event => false,
        Advance::Break | Advance::Watch { .. } => panic!("oracle: an instruction retired during a crossing"),
    }
}

/// `retrace debug <trace> --script <script>` on the codesigned copy: (exit code, stdout, stderr).
/// Bounded since M42 (`util::debug_bounded`, 600 s): a chain that hangs fails, naming the script.
pub fn debug(trace: &str, script: &str) -> (i32, String, String) {
    let (code, out, err) = super::debug_bounded(trace, script, 600);
    let code = code.unwrap_or_else(|| panic!("debug killed at the 600 s bound (a hang):\n{script}\n{out}"));
    (code, out, err)
}

/// Parse a transcript in which every `continue` / `reverse-continue` is followed by `where` into
/// one answer per such command: `Some(key)` for a hit, `None` for an end ("exited", "guest
/// crashed", "guest terminated", "no earlier hit"). Stops early if the debugger died mid-script.
pub fn answers(transcript: &str) -> Vec<Option<Key>> {
    let lines: Vec<&str> = transcript.lines().map(super::strip_annot).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i] != "> continue" && lines[i] != "> reverse-continue" { i += 1; continue; }
        let mut j = i + 1;
        while j < lines.len() && !lines[j].starts_with("> ") { j += 1; }
        if lines.get(j).copied() != Some("> where") || j + 1 >= lines.len() { break; }
        out.push(parse_answer(&lines[i + 1..j], lines[j + 1], transcript));
        i = j + 2;
    }
    out
}

fn parse_answer(body: &[&str], wh: &str, all: &str) -> Option<Key> {
    let first = body.first().copied().unwrap_or("");
    if ["exited", "guest crashed", "guest terminated", "no earlier hit"].iter().any(|p| first.starts_with(p)) {
        return None;
    }
    let phase = if first.contains("(syscall write)") { Phase::Sys }
        else if first.starts_with("hit watch ") { Phase::Watch }
        else if first.starts_with("hit 0x") { Phase::Bp }
        else { panic!("unrecognised answer {first:?} in:\n{all}") };
    let (n, k) = coord_of(body).unwrap_or_else(|| panic!("no coordinate in {body:?}:\n{all}"));
    let (wn, wk, pc, thread) = parse_where(wh).unwrap_or_else(|| panic!("bad `where` {wh:?}:\n{all}"));
    assert_eq!((wn, wk), (n, k), "`where` disagrees with the reported coordinate:\n{all}");
    Some((n, k, phase, (phase != Phase::Sys).then_some((pc, thread))))
}

/// `resolved (N, K)` if present, else the `at (N, K)` that ends the hit line.
fn coord_of(body: &[&str]) -> Option<(usize, u64)> {
    if let Some(r) = body.iter().find_map(|l| l.strip_prefix("resolved ")) { return pair(r); }
    let first = body.first()?;
    pair(&first[first.rfind(" at ")? + 4..])
}

/// `(N, K)` → (N, K).
fn pair(s: &str) -> Option<(usize, u64)> {
    let (a, b) = s.trim().strip_prefix('(')?.strip_suffix(')')?.split_once(", ")?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// `at (N, K) pc=0x… thread=T` → (N, K, pc, T).
fn parse_where(l: &str) -> Option<(usize, u64, u64, u32)> {
    let rest = l.strip_prefix("at ")?;
    let close = rest.find(')')?;
    let (n, k) = pair(&rest[..=close])?;
    let (mut pc, mut thread) = (None, None);
    for tok in rest[close + 1..].split_whitespace() {
        if let Some(h) = tok.strip_prefix("pc=0x") { pc = u64::from_str_radix(h, 16).ok(); }
        if let Some(t) = tok.strip_prefix("thread=") { thread = t.parse().ok(); }
    }
    Some((n, k, pc?, thread?))
}

/// Spec §3e's three checks of one arming (a `;`-separated `break`/`watch` script) against `hits`,
/// the oracle's list for that arming.
pub fn check_chains(trace: &str, arming: &str, hits: &[Hit]) {
    let keys: Vec<Key> = hits.iter().map(Hit::key).collect();
    let len = keys.len();
    assert!(len >= 2, "an arming needs two hits for the zig-zag to have neighbours: {hits:?}");

    // 1. Forward: `continue` from the opening position visits every hit in order, then ends.
    let mut want: Vec<Option<Key>> = keys.iter().copied().map(Some).collect();
    want.push(None);
    chain(trace, "forward chain", &format!("{arming}; {}", "continue; where; ".repeat(len + 1)), &want);

    // 2. Backward: from the terminal, `reverse-continue` visits them in reverse, then ends. The
    //    leading `continue` runs to the terminal with nothing armed yet; its answer is the end.
    let mut want = vec![None];
    want.extend(keys.iter().rev().copied().map(Some));
    want.push(None);
    chain(trace, "backward chain",
          &format!("continue; where; {arming}; {}", "reverse-continue; where; ".repeat(len + 1)), &want);

    // 3. Zig-zag: a hit reached by `continue` gives its predecessor on `reverse-continue`, and one
    //    reached by `reverse-continue` gives its successor on `continue`.
    let mut script = format!("{arming}; continue; where; ");
    let mut want = vec![Some(keys[0])];
    for pair in keys.windows(2) {
        script += "continue; where; reverse-continue; where; continue; where; ";
        want.extend([Some(pair[1]), Some(pair[0]), Some(pair[1])]);
    }
    chain(trace, "zig-zag", &script, &want);
}

fn chain(trace: &str, what: &str, script: &str, want: &[Option<Key>]) {
    let (code, out, err) = debug(trace, script);
    let got = answers(&out);
    if let Some(i) = (0..want.len().max(got.len())).find(|&i| want.get(i) != got.get(i)) {
        panic!("{what}: answer #{i} is {:?}; the oracle says {:?}\n\
                ({} answers wanted, {} given; debugger exit {code}; stderr: {err})\n\
                every answer given: {got:?}",
               got.get(i), want.get(i), want.len(), got.len());
    }
    assert_eq!(code, 0, "{what}: the debugger exited {code}; stderr: {err}");
}
