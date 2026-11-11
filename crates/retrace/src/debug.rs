//! `retrace debug <trace> --script '…'` — a scripted, deterministic reverse-debugger over a
//! recorded trace. Commands are `;`-separated; each echoes itself (`> <command>`) then its output.
//! Every printed byte derives from guest state, the script, or fixed strings — no host pointers, no
//! timing, no map-iteration order — so a transcript is bit-reproducible (the Task 6 e2e contract).

use std::io::Write;
use crate::json::{hex, J};
use std::path::Path;
use retrace_core::{checkpointed_seek, Advance, Armed, CheckpointCache, Outcome, ReplayReport, ReplaySession, Stepped, ThreadState};
use retrace_core::symbols::Symbols;
use retrace_core::{exec_epochs, Epoch};

/// The `x <addr> <len>` length ceiling: a larger span is a *parse* error (deterministic Err → exit
/// 5), guarding the inherited u64 span-overflow edge at the CLI boundary before any VM work.
const MAX_EXAMINE_LEN: usize = 65536;

/// Checkpoint cache sizing (M4): a byte budget generous enough to hold several tens of mid-run
/// checkpoints without unbounded growth on a long debug session, and a single-step cost gate that
/// only bothers caching positions genuinely expensive to reach (landmark replay is native-speed and
/// excluded from this count).
const CHECKPOINT_BYTE_BUDGET: usize = 256 * 1024 * 1024;
const CHECKPOINT_COST_GATE_STEPS: u64 = 64;

/// One parsed debugger command. `Break`/`Delete` carry a guest VA; `Examine` carries (VA, len);
/// `Stepi`/`ReverseStepi` carry a repeat count (default 1); `RegsOf` carries a thread id (M15).
/// `Watch` carries (VA, len, an optional thread scope — M15 Task 8): the hardware slot is global
/// (one vCPU, no per-thread DBGW), so a thread scope is a debugger-side filter applied at the hit
/// sites, never armed in hardware.
#[derive(Debug, PartialEq)]
pub enum Cmd {
    Break(Operand),
    Delete(Operand),
    Continue,
    ReverseContinue,
    Stepi(u64),
    ReverseStepi(u64),
    Regs,
    Examine(u64, usize),
    Where,
    Watch(u64, u64, Option<u32>),
    Unwatch(u64),
    Threads,
    RegsOf(u32),
    /// M58 B4: a frame-pointer backtrace of a thread (the current one when omitted).
    Bt(Option<u32>),
    /// M58 B5: move the cursor to position (n, k).
    Goto(usize, u64),
    /// M58 B5: the last write to `len` bytes at an address or symbol, before the cursor.
    LastWrite(Operand, u64),
    /// M58 B6: how the run ended and every Rust panic, from the recording's start.
    Summary,
}

/// The non-empty, trimmed command segments of a script, in order. `break;;continue;` → the two
/// commands (empty segments skipped). Shared by `parse_script` and the executor so the echoed
/// text is exactly the source segment.
fn segments(script: &str) -> impl Iterator<Item = &str> {
    script.split(';').map(str::trim).filter(|s| !s.is_empty())
}

/// A guest address: hex, `0x`-prefix optional (the brief writes `<hex-addr>`). `zzz` → Err.
///
/// Still the parser for every operand M20 did **not** take symbols for (`watch`, `unwatch`, `x`) —
/// see `Operand` for why those were left alone.
fn parse_addr(tok: &str) -> Result<u64, String> {
    let hex = tok.strip_prefix("0x").or_else(|| tok.strip_prefix("0X")).unwrap_or(tok);
    u64::from_str_radix(hex, 16).map_err(|_| format!("bad hex address: {tok}"))
}

/// An unresolved `break`/`delete` operand: either a literal address or a symbol name (M20).
///
/// **Why the name is not resolved here.** `parse_script` runs to completion *before* `Exec::new`
/// opens the trace (S1), so the symbol table does not exist during parsing. Resolution therefore
/// happens at execution, in `Exec::addr_of`. The alternative — construct `Exec` first and parse
/// afterwards — would move every parse diagnostic behind VM setup, and this file deliberately
/// raises an over-long `x` span as a *parse* error "before any VM work" (see `MAX_EXAMINE_LEN`).
///
/// Only `break` and `delete` carry this. `watch`/`unwatch`/`x` keep `parse_addr` because a symbol
/// cannot supply their **length**: `nlist_64` has no size field (S5), and a watch of invented width
/// silently misses writes to the bytes it failed to cover. A uniform operand type across every
/// command would have been tidier and would have implied support that does not exist.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Operand {
    Addr(u64),
    Sym(String),
}

/// Classify a `break`/`delete` operand. **Total — never fails**, because an unknown name is not a
/// parse error under M20; it is a lookup that may find nothing at execution time.
///
/// The rule is ordered, and the order is the decision (S2):
/// 1. `0x`/`0X` prefix ⇒ address. Explicit prefix, explicit intent.
/// 2. Parses completely as hex ⇒ address. **This is the backward-compatibility guarantee**: every
///    script that worked before M20 parses identically after it.
/// 3. Otherwise ⇒ symbol name.
///
/// Rule 2 costs one thing, documented rather than hidden: a symbol literally named `deadbeef` is
/// unreachable, because that token is also valid hex. Mach-O C symbols carry a leading underscore
/// (M1), so `_deadbeef` lands on rule 3, and mangled Rust names are never all-hex. An escape sigil
/// is the additive follow-up if one is ever needed; M20 does not build it speculatively.
fn parse_operand(tok: &str) -> Operand {
    match tok.strip_prefix("0x").or_else(|| tok.strip_prefix("0X")) {
        Some(hex) => match u64::from_str_radix(hex, 16) {
            Ok(a) => Operand::Addr(a),
            Err(_) => Operand::Sym(tok.to_string()),
        },
        None => match u64::from_str_radix(tok, 16) {
            Ok(a) => Operand::Addr(a),
            Err(_) => Operand::Sym(tok.to_string()),
        },
    }
}

/// A byte length: decimal, or hex with a `0x` prefix. Capped at `MAX_EXAMINE_LEN` (a larger value
/// is a parse error, not a clamp — determinism over silent truncation).
fn parse_len(tok: &str) -> Result<usize, String> {
    let v = match tok.strip_prefix("0x").or_else(|| tok.strip_prefix("0X")) {
        Some(hex) => usize::from_str_radix(hex, 16),
        None => tok.parse::<usize>(),
    }.map_err(|_| format!("bad length: {tok}"))?;
    if v > MAX_EXAMINE_LEN {
        return Err(format!("length {v} exceeds max {MAX_EXAMINE_LEN}"));
    }
    Ok(v)
}

/// A repeat count for `stepi`/`reverse-stepi`: decimal, defaulting to 1 when omitted.
fn parse_count(tok: Option<&str>) -> Result<u64, String> {
    match tok {
        None => Ok(1),
        Some(t) => t.parse::<u64>().map_err(|_| format!("bad count: {t}")),
    }
}

/// Parse one trimmed, non-empty command segment. Unknown verb / bad operand → Err (the caller
/// turns that into exit 5). Extra trailing tokens are rejected (fail-loud over silent ignore).
fn parse_one(seg: &str) -> Result<Cmd, String> {
    let mut it = seg.split_whitespace();
    let verb = it.next().ok_or_else(|| "empty command".to_string())?;
    // Collect the operands so a trailing-garbage token can be rejected per command.
    let ops: Vec<&str> = it.collect();
    let expect_none = |cmd: Cmd, ops: &[&str]| -> Result<Cmd, String> {
        if ops.is_empty() { Ok(cmd) } else { Err(format!("`{verb}` takes no arguments")) }
    };
    match verb {
        "break"           => { one_operand(verb, &ops)?; Ok(Cmd::Break(parse_operand(ops[0]))) }
        "delete"          => { one_operand(verb, &ops)?; Ok(Cmd::Delete(parse_operand(ops[0]))) }
        "watch"           => {
            // Grammar: `watch <addr> [len] [thread <n>]` — `len` stays purely positional (as
            // before Task 8); `thread <n>` is a trailing keyword clause so it composes with an
            // omitted `len` without ambiguity (`ops[1]` is only ever read as a length when it is
            // NOT the literal `thread`).
            if ops.is_empty() || ops.len() > 4 {
                return Err(format!(
                    "`watch` takes <addr> [len] [thread <n>]; got {} operand(s)", ops.len()));
            }
            let addr = parse_addr(ops[0])?;
            let mut idx = 1;
            let len = if idx < ops.len() && ops[idx] != "thread" {
                let l = ops[idx].parse::<u64>().map_err(|_| format!("bad watch len: {}", ops[idx]))?;
                idx += 1;
                l
            } else {
                8u64
            };
            let thread = if idx < ops.len() {
                if ops[idx] != "thread" {
                    return Err(format!("unexpected token after watch operands: {}", ops[idx]));
                }
                idx += 1;
                let t = ops.get(idx).ok_or_else(|| "`thread` requires a thread id".to_string())?;
                idx += 1;
                Some(t.parse::<u32>().map_err(|_| format!("bad thread id: {t}"))?)
            } else {
                None
            };
            if idx != ops.len() {
                return Err("`watch` has trailing garbage after the thread id".to_string());
            }
            if !matches!(len, 1 | 2 | 4 | 8) {
                return Err(format!("watch len must be 1, 2, 4, or 8; got {len}"));
            }
            if addr % len != 0 {
                return Err(format!("watch address {addr:#x} must be {len}-byte aligned"));
            }
            Ok(Cmd::Watch(addr, len, thread))
        }
        "unwatch"         => { one_operand(verb, &ops)?; Ok(Cmd::Unwatch(parse_addr(ops[0])?)) }
        "summary"         => expect_none(Cmd::Summary, &ops),
        "continue"        => expect_none(Cmd::Continue, &ops),
        "reverse-continue"=> expect_none(Cmd::ReverseContinue, &ops),
        "stepi"           => { at_most_one(verb, &ops)?; Ok(Cmd::Stepi(parse_count(ops.first().copied())?)) }
        "reverse-stepi"   => { at_most_one(verb, &ops)?; Ok(Cmd::ReverseStepi(parse_count(ops.first().copied())?)) }
        "threads"         => expect_none(Cmd::Threads, &ops),
        "regs"            => {
            at_most_one(verb, &ops)?;
            match ops.first() {
                None => Ok(Cmd::Regs),
                Some(t) => Ok(Cmd::RegsOf(t.parse::<u32>()
                    .map_err(|_| format!("bad thread id: {t}"))?)),
            }
        }
        "where"           => expect_none(Cmd::Where, &ops),
        "bt"              => {
            at_most_one(verb, &ops)?;
            match ops.first() {
                None => Ok(Cmd::Bt(None)),
                Some(t) => Ok(Cmd::Bt(Some(t.parse::<u32>().map_err(|_| format!("bad thread id: {t}"))?))),
            }
        }
        "goto"            => {
            // `goto 3 5`, `goto (3, 5)` and `goto 3:5` are one command.
            let joined = ops.join(" ").replace(['(', ')', ',', ':'], " ");
            let nums: Vec<&str> = joined.split_whitespace().collect();
            if nums.len() != 2 { return Err("`goto` takes <n> <k>, (n, k) or n:k".to_string()); }
            let n = nums[0].parse::<usize>().map_err(|_| format!("bad landmark: {}", nums[0]))?;
            let k = nums[1].parse::<u64>().map_err(|_| format!("bad step: {}", nums[1]))?;
            if n == 0 { return Err("landmarks start at 1".to_string()); }
            Ok(Cmd::Goto(n, k))
        }
        "last-write"      => {
            if ops.is_empty() || ops.len() > 2 {
                return Err(format!("`last-write` takes <addr|symbol> [len]; got {} operand(s)", ops.len()));
            }
            let len = match ops.get(1) {
                Some(l) => l.parse::<u64>().map_err(|_| format!("bad watch len: {l}"))?,
                None => 8,
            };
            check_watch_len(len)?;
            let op = parse_operand(ops[0]);
            if let Operand::Addr(a) = &op { check_watch_align(*a, len)?; }
            Ok(Cmd::LastWrite(op, len))
        }
        "x"               => {
            if ops.len() != 2 { return Err(format!("`x` takes <addr> <len>; got {} operand(s)", ops.len())); }
            Ok(Cmd::Examine(parse_addr(ops[0])?, parse_len(ops[1])?))
        }
        other             => Err(format!("unknown command: {other}")),
    }
}

/// The `last-write` line when an exec at landmark `n` comes before any write: the image that held the address is gone.
fn no_write_since_exec(addr: u64, n: usize) -> String { format!("no write to {addr:#x} since the exec at ({n}, 0)") }

fn check_watch_len(len: u64) -> Result<(), String> {
    if matches!(len, 1 | 2 | 4 | 8) { Ok(()) } else { Err(format!("watch len must be 1, 2, 4, or 8; got {len}")) }
}
fn check_watch_align(addr: u64, len: u64) -> Result<(), String> {
    if addr.is_multiple_of(len) { Ok(()) } else { Err(format!("watch address {addr:#x} must be {len}-byte aligned")) }
}

fn one_operand(verb: &str, ops: &[&str]) -> Result<(), String> {
    if ops.len() == 1 { Ok(()) } else { Err(format!("`{verb}` takes one operand; got {}", ops.len())) }
}
fn at_most_one(verb: &str, ops: &[&str]) -> Result<(), String> {
    if ops.len() <= 1 { Ok(()) } else { Err(format!("`{verb}` takes at most one operand; got {}", ops.len())) }
}

/// Parse a whole `;`-separated script into commands. All-or-nothing: any bad command aborts the
/// parse (so the CLI exits 5 before producing partial output).
pub fn parse_script(script: &str) -> Result<Vec<Cmd>, String> {
    segments(script).map(parse_one).collect()
}

/// Write one line (formatted content + `\n`), mapping any I/O error into the `run_script` error type.
fn line<W: Write>(out: &mut W, args: std::fmt::Arguments) -> Result<(), String> {
    out.write_fmt(args).map_err(|e| format!("write error: {e}"))?;
    out.write_all(b"\n").map_err(|e| format!("write error: {e}"))
}

/// What `resolve_nth` counts (M40 §3a). A watch hit is identified BY ADDRESS: the hardware's own
/// watchpoint stop, while single-stepping with `Watch`'s ranges armed. A breakpoint hit is
/// identified by pc membership in `Break`'s addresses, stepped with breakpoints DISARMED (armed,
/// one fires before retire at the current pc, forever).
enum HitKind<'a> { Watch(&'a [(u64, u64)]), Break(&'a [u64]) }

/// Replay window `n` from `(n, from_k)` and return the K of the `ordinal`-th (1-based) hit of
/// `kind`, checking that the instruction there is at `expect_pc` (the pc the scan saw). That check
/// makes a scan/resolver disagreement fail loud only when the disagreement lands on a DIFFERENT
/// instruction. An ordinal off by one among hits at the same pc — every hit of one breakpoint,
/// every run of one loop store — passes it and names the wrong run silently. Replaces M3's
/// `resolve_hit_k`, which matched a watch hit BY PC: a store that ran on other addresses first
/// resolved to an earlier run that never wrote the watched range (t0 M6/M7: rung 8's `continue`
/// landed 1.7 M instructions early). Deterministic; runs on its own transient session, so the
/// caller must hold NO other live session (one VM per process).
fn resolve_nth(trace: &Path, cache: &mut CheckpointCache, n: usize, from_k: u64, kind: HitKind,
               ordinal: u64, expect_pc: u64) -> Result<u64, String> {
    debug_assert!(ordinal >= 1, "ordinals are 1-based");
    let mut s = checkpointed_seek(trace, cache, n, from_k)?;
    let (mut k, mut seen) = (from_k, 0u64);
    let found = |s: &ReplaySession, k: u64| -> Result<u64, String> {
        if s.pc() == expect_pc { Ok(k) } else { Err(format!(
            "resolve hit #{ordinal} in window {n}: the scan saw pc {expect_pc:#x}, the resolver reached {:#x} at K={k}",
            s.pc())) }
    };
    match kind {
        HitKind::Break(addrs) => loop {
            if addrs.contains(&s.pc()) {
                seen += 1;
                if seen == ordinal { return found(&s, k); }
            }
            // M41 R7: name what was searched. The step is one instruction per call, so its own
            // "window N ends after 0 instruction(s)" read like a window length and was not one
            // (M41 t0 M2).
            s.step_insns(1).map_err(|e| if e.contains("ends after") {
                format!("resolve breakpoint hit #{ordinal} in window {n}: the window ended after {seen} hit(s) at K={k}")
            } else {
                format!("resolve breakpoint hit #{ordinal} in window {n}: {e}")
            })?;
            k += 1;
        },
        HitKind::Watch(ranges) => {
            s.arm_watchpoints(ranges);
            loop {
                match s.step_watched().map_err(|e| format!("resolve watch hit #{ordinal} in window {n}: {e}"))? {
                    Stepped::Retired => k += 1,
                    Stepped::Watch => {
                        seen += 1;
                        if seen == ordinal { return found(&s, k); }
                        s.clear_watchpoints(); // step over this earlier hit in place, then re-arm
                        s.step_insns(1).map_err(|e| format!("resolve watch hit #{ordinal} in window {n}: {e}"))?;
                        s.arm_watchpoints(ranges);
                        k += 1;
                    }
                    Stepped::AtTrap => return Err(format!(
                        "resolve watch hit #{ordinal} in window {n}: the window ended after {seen} hit(s) at K={k}")),
                }
            }
        }
    }
}

/// The armed watch range containing `far` (exact byte); else the range overlapping `far`'s aligned
/// doubleword (FAR may report the comparator base — spike F4b); else (M40) the first range
/// intersecting `far`'s naturally aligned 64-byte block. The Arm ARM bounds a watchpoint's FAR: for
/// `DC ZVA` it is any address in the zeroed block (64 bytes here), and for any other store it lies in
/// a naturally aligned block, no larger than the `DC ZVA` block, that contains a watched address the
/// store wrote. So a store that really wrote a watched range reports a FAR in that range's 64-byte
/// block (rung 8: FAR `…ac0` for a watched `…ac8`, t0 M5), and a FAR in any other block cannot have
/// come from one. With two armed ranges in one block this names the first, which may not be the one
/// written. Else `far` itself (honest fallback). Deterministic: `ws` is sorted, first match wins. The
/// third rule runs only where the first two missed, so no output they got right changes.
fn watched_of(ws: &[(u64, u64)], far: u64) -> u64 {
    let blk = far & !63;
    ws.iter().find(|&&(a, l)| far >= a && far < a + l)
        .or_else(|| ws.iter().find(|&&(a, l)| { let d = far & !7; d < a + l && a < d + 8 }))
        .or_else(|| ws.iter().find(|&&(a, l)| a < blk + 64 && blk < a + l))
        .map(|&(a, _)| a)
        .unwrap_or(far)
}

/// M41 §3b: where a hit sits within one coordinate (n, k), in the order the hardware produces them.
/// Hits are totally ordered by `(n, k, Phase)`; the debugger's cursor is such a triple.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Phase {
    /// A syscall's recorded write to a watched range. Only at k = 0: the event that ended window
    /// n − 1 wrote it before the instruction at (n, 0) runs.
    Sys,
    /// The instruction at (n, k) is about to execute and its address is a breakpoint. Also every
    /// ARRIVAL's phase (spec R4, gdb's rule): a breakpoint at the pc you stand on is reported in
    /// neither direction, and a watched store you stepped up to still fires going forward.
    Bp,
    /// That instruction's store to a watched range, stopped pre-retire. Also a TERMINAL's phase
    /// (R18, `park_at_terminal`): the end of the recording sits after every hit.
    Watch,
}

/// M43 §3b: what one motion did, alongside the lines it printed. The script CLI ignores it; the
/// gdb-remote server reports it.
#[derive(Debug)]
pub(crate) enum Halt {
    /// Parked on a breakpoint, (n, k, Bp). `pc()` is its address.
    Break,
    /// A store to a watched range, parked pre-retire, (n, k, Watch).
    Watch { watched: u64 },
    /// M43 §3d: `step_thread` stepped a store to a watched range; it retired, (n, k + 1, Bp).
    WatchStepped { watched: u64 },
    /// A syscall's recorded write to a watched range, (n, 0, Sys); `thread` wrote it.
    WatchSys { watched: u64, thread: u32 },
    /// The recording's end (exit, crash, fatal signal), parked at (T, K_f, Watch) (M41 R18).
    Terminal(ReplayReport),
    /// `reverse-continue` found nothing before the cursor, which did not move.
    NoEarlierHit,
    /// M54 §3i (C6): a motion reached an `Exec` landmark and stopped on it, parked at `(n, 0)`, the new
    /// image's entry, with `path` the one the program passed.
    Exec { path: Vec<u8> },
    /// A step completed.
    Stepped,
    /// `reverse-stepi` stopped at (1, 0).
    AtStart,
    /// The step did not move the thread, with the reason. Nothing moved for `stepi`'s window end or
    /// fault, or for `step_thread`'s thread that is not live (M43 §3d rule 1, as M44 B6(b) leaves
    /// it). For `step_thread`'s trap that returns to itself, the cursor crossed the event and only
    /// the pc stayed. For its run until the thread is current that reached the recording's end on
    /// another thread first (M44 Ruling T12-a), the cursor is parked at the terminal and the thread
    /// did not run again (a blocked step's thread ran only the trap it blocked in).
    Refused(String),
    /// M44 B3: `step_thread`'s thread exited in the step's crossing, parked at (n, 0, Sys), the
    /// crossing's own position (Ruling FW-b). Nothing runs it again, so the step ends at that
    /// boundary instead of at the end of the recording.
    ThreadExited { thread: u32 },
    /// M44 B6(a): a step of `thread` that blocked was ended by another thread's hit (`by`), parked
    /// at that hit exactly as `continue` parks it (`Break`, `Watch`, `WatchSys` above). The server
    /// re-parks and reports it as it does `continue`'s (Ruling T11-a).
    StepInterrupted { thread: u32, by: u32, hit: Hit },
}

/// M44 B6(a): the hits a step's run until its thread is current can end on, as `StepInterrupted`
/// carries them. Exactly `continue`'s three hit stops, so nothing else can be carried.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Hit {
    Break,
    Watch { watched: u64 },
    WatchSys { watched: u64 },
}

/// The scripted-debugger executor. Holds the cursor P = (`n`, `k`, `phase`) (M41 §3b) and at most ONE
/// live `ReplaySession` parked exactly at (`n`, `k`) (one VM per process → every command that MOVES
/// drops the old session before seeking a fresh one). `breakpoints` is kept sorted + deduped (≤ 6,
/// enforced by `break`) so the hardware-slot assignment and any iteration are deterministic.
pub(crate) struct Exec<'a> {
    trace: &'a Path,
    session: Option<ReplaySession>,
    n: usize,
    k: u64,
    breakpoints: Vec<u64>,
    /// Armed watch ranges (addr, len, optional thread scope), sorted by addr + deduped (≤ 4: one
    /// per DBGWVR slot). The thread scope (M15 Task 8) is a pure debugger-side filter — the
    /// hardware slot itself stays global — checked at each hit site via `watch_thread_matches`.
    watches: Vec<(u64, u64, Option<u32>)>,
    /// M41: the cursor's phase at (n, k) — see `Phase`. `continue` answers the first hit after
    /// (n, k, phase), `reverse-continue` the last one before it. Replaces M5's `last_watch_hit`,
    /// which was `phase == Watch` in disguise. Every position at one (n, k) is the same machine
    /// state (before the instruction at (n, k); a watch stop is pre-retire), so no session encodes
    /// the phase.
    phase: Phase,
    cache: CheckpointCache,
    /// M54 §3i: every image's symbols, one per epoch: epoch 0's built ONCE per session from the
    /// recording's opening snapshot (M19, below), and epoch k's from the k-th `Event::Exec`'s slices
    /// (`Symbols::from_exec`, C8). The epoch of a position is the last one whose `first_event` is
    /// before the position's landmark count (`epoch_of`), so a position at an exec stop is the new
    /// image's and the svc before it the old one's.
    ///
    /// M19: address → name, built ONCE per session from the recording's opening snapshot.
    ///
    /// Preserves this module's bit-reproducibility contract (see the file header): the names derive
    /// from *guest state* — the `__LINKEDIT` bytes the snapshot already carries — not from a host
    /// path, the host's own symbols, or anything timing- or iteration-order-dependent. An empty
    /// table (static guest, stripped binary) simply formats every address as bare hex.
    epochs: Vec<(Epoch, Symbols)>,
    /// M58 B2: readable names, one lazy cache per epoch (exec epochs have different images).
    readables: Vec<crate::demangle::Readable>,
    /// M58 B3: file and line, one lazy `SourceMap` per epoch, built on the first pc that needs one so a session
    /// that never asks for a line never runs `atos`. Host-derived and never written into a trace.
    sources: std::cell::RefCell<Vec<Option<crate::source::SourceMap>>>,
    /// M58 B3: epoch 0's candidate paths for its program, read from the opening start stack.
    start_cands: Vec<String>,
}

impl<'a> Exec<'a> {
    /// Open at the recording start: P = (1, 0), the first landmark's window, step 0.
    pub(crate) fn new(trace: &'a Path) -> Result<Self, String> {
        let mut cache = CheckpointCache::new(CHECKPOINT_BYTE_BUDGET, CHECKPOINT_COST_GATE_STEPS);
        // M50 R5: an incomplete recording (no exit, crash or fatal signal at its end: it stopped at a
        // wall, or its recorder was killed) is refused at the door, for `debug` and `gdbserver` alike,
        // rather than failing somewhere inside the session.
        if !cache.decoded(trace)?.is_complete() {
            return Err(format!("{} is an incomplete recording: it has no exit, crash or fatal signal at \
                its end, because it stopped at a wall or its recorder was killed. Debugging an incomplete \
                recording is not supported yet - see {}#incomplete-recordings",
                trace.display(), retrace_arch::WALLS_URL));
        }
        let session = checkpointed_seek(trace, &mut cache, 1, 0)?;
        // M19: build the symbol table from the OPENING snapshot, once. It is the image as loaded,
        // which is what every pc in the session refers to. Any failure here — unreadable trace, no
        // Snapshot, stripped binary — yields an empty table and bare-hex output rather than an
        // error: symbolication is presentation, and must never be able to fail a debug session.
        // M40: from the cache's one decode, not a second read of the file.
        let epochs = cache.decoded(trace).ok().map(|t| {
            let ev = t.events();
            exec_epochs(ev).into_iter().map(|e| {
                let syms = if e.first_event == 0 {
                    ev.iter().find_map(|x| match x {
                        retrace_trace::Event::Snapshot { mem, .. } => Some(Symbols::from_snapshot(mem)),
                        _ => None,
                    }).unwrap_or_default()
                } else { Symbols::from_exec(&e.image, &e.dylinker) };
                (e, syms)
            }).collect::<Vec<_>>()
        }).unwrap_or_default();
        let readables = epochs.iter().map(|(_, s)| crate::demangle::Readable::new(s)).collect();
        let start_cands = crate::image::start_candidates(&session);
        let sources = std::cell::RefCell::new((0..epochs.len().max(1)).map(|_| None).collect());
        Ok(Exec {
            trace, session: Some(session), n: 1, k: 0,
            breakpoints: Vec::new(), watches: Vec::new(), phase: Phase::Bp, cache, readables, epochs,
            sources, start_cands,
        })
    }

    /// M19: `"  in _child+0x30"` for an address that resolves, or the empty string for one that does
    /// not.
    ///
    /// **Appended at end of line, never inserted after the address.** Every pc in this file is
    /// already matched by existing assertions that read up to whatever follows it — `crashy_cli`
    /// greps `"guest crashed: pc={pc:#x} far=..."`, `debug_cli` greps `"hit 0x{pc:x} at ("` — so an
    /// insertion would break them for no gain. A suffix leaves every one of those substrings
    /// intact, which is why this returns an annotation rather than a replacement for the address.
    fn annot(&self, n: usize, addr: u64) -> String {
        match self.syms_at(n).resolve(addr) {
            Some((raw, 0)) => format!("  in {}", self.readable_at(n, raw)),
            Some((raw, off)) => format!("  in {}+{off:#x}", self.readable_at(n, raw)),
            None => String::new(),
        }
    }

    /// M54 §3i: the index of the epoch a position that has consumed `n` landmarks is in.
    pub(crate) fn epoch_of(&self, n: usize) -> usize {
        self.epochs.iter().rposition(|(e, _)| e.first_event < n).unwrap_or(0)
    }

    /// The epoch the cursor is in.
    pub(crate) fn epoch_index(&self) -> usize { self.epoch_of(self.n) }

    /// How many epochs the recording holds: one more than its execs.
    pub(crate) fn epoch_count(&self) -> usize { self.epochs.len().max(1) }

    /// The symbols of the epoch at landmark count `n`. Empty when the trace could not be decoded.
    fn syms_at(&self, n: usize) -> &Symbols {
        self.syms_at_inner(n)
    }

    /// M58 B2: the readable form of a raw symbol name in the epoch at `n` (the raw name when none).
    fn readable_at(&self, n: usize, raw: &str) -> String {
        match self.readables.get(self.epoch_of(n)) { Some(r) => r.name(raw), None => raw.to_string() }
    }

    /// M58 B2: `Symbols::format` with the readable name.
    fn format_at(&self, n: usize, addr: u64) -> String {
        match self.syms_at(n).resolve(addr) {
            Some((raw, 0)) => format!("{addr:#x} ({})", self.readable_at(n, raw)),
            Some((raw, off)) => format!("{addr:#x} ({}+{off:#x})", self.readable_at(n, raw)),
            None => format!("{addr:#x}"),
        }
    }

    fn syms_at_inner(&self, n: usize) -> &Symbols {
        static EMPTY: std::sync::OnceLock<Symbols> = std::sync::OnceLock::new();
        match self.epochs.get(self.epoch_of(n)) { Some((_, s)) => s, None => EMPTY.get_or_init(Symbols::default) }
    }

    /// M54 §3i: the path of the exec whose landmark has just been consumed at count `n`: the exec
    /// stop `(n, 0)`, the first position of the epoch it began. `None` for any other landmark.
    fn exec_at(&self, n: usize) -> Option<&[u8]> {
        self.epochs.iter().skip(1).find(|(e, _)| e.first_event + 1 == n).map(|(e, _)| e.path.as_slice())
    }

    /// M54 §3i, T6-e: whether the cursor stands on an exec stop, `(n, 0)` of an exec landmark, where
    /// any move backward would leave the new epoch for the old one.
    pub(crate) fn at_exec_stop(&self) -> bool { self.k == 0 && self.exec_at(self.n).is_some() }

    /// Whether the event that ends window `n` (the trap at its end) is an exec.
    fn exec_ahead(&self, n: usize) -> bool { self.exec_at(n + 1).is_some() }

    /// M54 §3i (C6): stop on the exec landmark just crossed, if landmark count `n` is one. Prints the
    /// line, parks the cursor at `(n, 0)` as an arrival (R4: a breakpoint at the new image's entry is
    /// not reported on top of it), and leaves the live session hit-clean.
    fn exec_stop<W: Write>(&mut self, n: usize, out: &mut W) -> Result<Option<Halt>, String> {
        let Some(path) = self.exec_at(n).map(<[u8]>::to_vec) else { return Ok(None) };
        line(out, format_args!("exec {} at ({n}, 0)", String::from_utf8_lossy(&path)))?;
        self.sess_mut().clear_breakpoints();
        self.sess_mut().clear_watchpoints();
        (self.n, self.k, self.phase) = (n, 0, Phase::Bp);
        Ok(Some(Halt::Exec { path }))
    }

    pub(crate) fn sess(&self) -> &ReplaySession { self.session.as_ref().expect("live session") }
    /// M43: the cursor `(n, k, phase)` (M41 §3b), for the gdb-remote server.
    pub(crate) fn cursor(&self) -> (usize, u64, Phase) { (self.n, self.k, self.phase) }
    fn sess_mut(&mut self) -> &mut ReplaySession { self.session.as_mut().expect("live session") }

    /// Drop the current session (freeing its VM) and seek a fresh one parked at (n, k), which is
    /// therefore breakpoint-clean. Updates the position coordinate, and resets the cursor's phase to
    /// `Bp`: an arrival (R4). A caller that parks on a hit or a terminal, or that did not move, sets
    /// it after.
    fn reseek(&mut self, n: usize, k: u64) -> Result<(), String> {
        self.session = None; // free the old VM BEFORE opening a new one
        self.session = Some(checkpointed_seek(self.trace, &mut self.cache, n, k)?);
        self.n = n;
        self.k = k;
        self.phase = Phase::Bp; // an arrival (R4); a hit's or a terminal's park sets its phase after
        Ok(())
    }

    /// M43 §3b: re-seek to a saved cursor, restoring its phase. `reseek` drops whatever session
    /// exists (armed, moved, or none, t0 R2) and seeks a fresh, breakpoint-clean one, so one call
    /// undoes every state an `Err` can leave. The gdb-remote server calls it after every failed
    /// motion. The script CLI does not, because an `Err` ends the script.
    pub(crate) fn recover(&mut self, at: (usize, u64, Phase)) -> Result<(), String> {
        self.reseek(at.0, at.1)?;
        self.phase = at.2;
        Ok(())
    }

    /// M43 §3c: set the cursor's phase without moving (a reverse store watch becomes `Bp`).
    pub(crate) fn set_phase(&mut self, phase: Phase) { self.phase = phase; }

    /// M43 §3c: park just before the event that ends window `n − 1`, at its trap, which is where a
    /// reverse syscall-watch stop belongs: before the write. `n ≥ 2` always (landmark 1's write
    /// lands at (2, 0)).
    pub(crate) fn park_before_event(&mut self, n: usize) -> Result<(), String> {
        debug_assert!(n >= 2, "a syscall write at ({n}, 0) has no event before it");
        let len = self.probe_window_len(n - 1)?;
        self.reseek(n - 1, len) // phase Bp: an arrival
    }

    /// M43 §3f: the armed watches, `(addr, len)`, so the server can tell a re-inserted `Z2` from a
    /// conflicting one.
    pub(crate) fn watches(&self) -> impl Iterator<Item = (u64, u64)> + '_ { self.watches.iter().map(|&(a, l, _)| (a, l)) }

    /// Window length of landmark `n`, memoized in the checkpoint cache (measured on a transient
    /// probe session at most once per landmark per debug session). Drops the live session first —
    /// even a memo hit re-establishes it cheaply via the position cache; the caller re-seeks via
    /// `reseek`.
    fn probe_window_len(&mut self, n: usize) -> Result<u64, String> {
        self.session = None; // free the live VM before any probe (one VM per process)
        self.cache.window_len(self.trace, n)
    }

    fn exec<W: Write>(&mut self, cmd: &Cmd, out: &mut W) -> Result<(), String> {
        match cmd {
            Cmd::Break(op)        => { let a = self.addr_of(op)?; self.cmd_break(a, out) }
            Cmd::Delete(op)       => { let a = self.addr_of(op)?; self.cmd_delete(a, out) }
            // M43 §3b: a motion's `Halt` is for the gdb-remote server. The script has printed it.
            Cmd::Continue         => self.cmd_continue(out).map(|_| ()),
            Cmd::ReverseContinue  => self.cmd_reverse_continue(out).map(|_| ()),
            Cmd::Stepi(count)     => self.cmd_stepi(*count, out).map(|_| ()),
            Cmd::ReverseStepi(c)  => self.cmd_reverse_stepi(*c, out).map(|_| ()),
            Cmd::Regs             => self.cmd_regs(out),
            Cmd::Examine(a, len)  => self.cmd_examine(*a, *len, out),
            Cmd::Where            => self.cmd_where(out),
            Cmd::Watch(a, l, t)   => self.cmd_watch(*a, *l, *t, out),
            Cmd::Unwatch(a)       => self.cmd_unwatch(*a, out),
            Cmd::Threads          => self.cmd_threads(out),
            Cmd::RegsOf(tid)      => self.cmd_regs_of(*tid, out),
            Cmd::Bt(tid)          => self.cmd_bt(*tid, out).map(|_| ()),
            Cmd::Goto(n, k)       => self.cmd_goto(*n, *k, out),
            Cmd::LastWrite(op, l) => self.cmd_last_write(op, *l, out).map(|_| ()),
            Cmd::Summary          => self.cmd_summary(out).map(|_| ()),
        }
    }

    /// M20: turn a `break`/`delete` operand into an address, resolving a symbol name against the
    /// session's table. Runs at **execution** because the table does not exist at parse time (S1).
    ///
    /// The three cases are the milestone's whole contract:
    ///
    /// - **no match** → error. Never a fallback to reinterpreting the token as hex: that would turn
    ///   a typo into a breakpoint at a wrong-but-valid address, which is precisely the silent
    ///   wrongness this design is arranged against.
    /// - **one match** → use it.
    /// - **many** → error, **listing the addresses**. Real input reaches this: dyld's arm64e slice
    ///   binds 14 names to more than one address and `___Block_byref_object_copy_` alone has 13 of
    ///   them (S4), so this is not a defensive branch, and a message that only
    ///   said "ambiguous" would leave the user with no way forward. The addresses are exactly what
    ///   M19 taught this debugger to print, so the error hands back something re-issuable as
    ///   `break 0x…`.
    ///
    /// This does not contradict M19's "symbolication may never fail a session": there, a missing
    /// *name* costs nothing because the debugger is only printing. Here a missing *address* means
    /// there is no command to carry out.
    fn addr_of(&self, op: &Operand) -> Result<u64, String> {
        match op {
            Operand::Addr(a) => Ok(*a),
            Operand::Sym(name) => {
                let hits = match self.readables.get(self.epoch_of(self.n)) {
                    Some(r) => r.lookup(self.syms_at(self.n), name),
                    None => self.syms_at(self.n).addrs_of(name),
                };
                match hits.len() {
                    0 => Err(format!("no symbol \"{name}\"")),
                    1 => Ok(hits[0]),
                    n => {
                        let list = hits.iter().map(|a| format!("{a:#x}"))
                            .collect::<Vec<_>>().join(", ");
                        Err(format!("\"{name}\" is ambiguous: {n} addresses ({list}) — \
                                     re-issue with the one you meant"))
                    }
                }
            }
        }
    }

    pub(crate) fn cmd_break<W: Write>(&mut self, addr: u64, out: &mut W) -> Result<(), String> {
        if let Err(i) = self.breakpoints.binary_search(&addr) {
            if self.breakpoints.len() >= 6 {
                return Err("cannot arm more than 6 breakpoints (hardware limit: DBGBVR0-5)".into());
            }
            self.breakpoints.insert(i, addr); // sorted + deduped (≤ 6: one per DBGBVR slot)
        }
        // M20 symbolicates this echo, deliberately extending M19's pc-bearing-lines-only rule by
        // one line: when the operand itself can be a name, confirming what it resolved to IS the
        // feedback, and it makes the ambiguity error above actionable. Safe by inspection — both
        // existing assertions on this line use `contains("breakpoint at 0x…")`, never `ends_with`,
        // which is the check M19's four broken assertions earned.
        line(out, format_args!("breakpoint at {}", self.format_at(self.n, addr)))
    }

    pub(crate) fn cmd_delete<W: Write>(&mut self, addr: u64, out: &mut W) -> Result<(), String> {
        if let Ok(i) = self.breakpoints.binary_search(&addr) {
            self.breakpoints.remove(i);
        }
        line(out, format_args!("deleted {addr:#x}"))
    }

    /// Task 8 fix round 1: re-`watch`ing an address that is ALREADY armed is a fail-loud usage
    /// error, not a silent no-op. Before this fix the echo unconditionally printed the
    /// just-requested `len`/`thread` while the STORED entry (what `arm_watchpoints` and
    /// `watch_thread_matches` actually consult) was left untouched on a duplicate — a scope
    /// change is exactly what this task exists to make real, so a watch that CLAIMS to be scoped
    /// while the filter keeps letting every thread through is the one failure mode this file
    /// cannot tolerate silently. Consistent with this file's other watch-arming failures (the
    /// 4-slot cap just below, the len/alignment checks in `parse_one`): explicit `Err`, not a
    /// partial or implicit mutation of already-armed state. `unwatch` first to change a watch.
    pub(crate) fn cmd_watch<W: Write>(&mut self, addr: u64, len: u64, thread: Option<u32>, out: &mut W) -> Result<(), String> {
        match self.watches.binary_search_by_key(&addr, |&(a, _, _)| a) {
            Err(i) => {
                if self.watches.len() >= 4 {
                    return Err("cannot arm more than 4 watchpoints (hardware limit: DBGWVR0-3)".into());
                }
                self.watches.insert(i, (addr, len, thread));
            }
            Ok(_) => return Err(format!(
                "{addr:#x} is already watched; `unwatch {addr:#x}` before re-arming it with a \
                 different len or thread scope")),
        }
        match thread {
            Some(t) => line(out, format_args!("watch at {addr:#x} len {len} thread {t}")),
            None    => line(out, format_args!("watch at {addr:#x} len {len}")),
        }
    }

    pub(crate) fn cmd_unwatch<W: Write>(&mut self, addr: u64, out: &mut W) -> Result<(), String> {
        if let Ok(i) = self.watches.binary_search_by_key(&addr, |&(a, _, _)| a) {
            self.watches.remove(i);
        }
        line(out, format_args!("unwatched {addr:#x}"))
    }

    /// Whether a hardware/syscall watch hit at `addr` by `thread` should be reported to the user:
    /// true when that watch has no scope, or the scope equals `thread`. The `_ => true` arm is a
    /// deliberate fail-OPEN, not merely a defensive default: `watched_of`'s own `far`-fallback
    /// (`.unwrap_or(far)`, used when `far` lands outside every armed range's exact bytes, its aligned
    /// doubleword, AND its 64-byte block) CAN hand this function an address genuinely absent from
    /// `self.watches` — a hit whose owning watch is unknown. Suppressing an unattributable hit would
    /// be a worse failure than showing an unscoped one: this function chooses to report it rather than
    /// risk silently hiding a real write.
    fn watch_thread_matches(&self, addr: u64, thread: u32) -> bool {
        match self.watches.iter().find(|&&(a, _, _)| a == addr) {
            Some(&(_, _, Some(scope))) => scope == thread,
            _ => true,
        }
    }

    fn cmd_regs<W: Write>(&mut self, out: &mut W) -> Result<(), String> {
        let dump = self.sess().dbg_regs();
        line(out, format_args!("{dump}"))
    }

    /// M15: one line per `ThreadSummary`, `*` marking the thread currently on the vCPU. Exited
    /// threads stay in the table (see `thread_summaries`'s doc) and are listed too — the debugger's
    /// user wants to see them, not have them silently drop off.
    fn cmd_threads<W: Write>(&mut self, out: &mut W) -> Result<(), String> {
        for t in self.sess().thread_summaries() {
            let marker = if t.is_current { "*" } else { " " };
            line(out, format_args!("{marker} thread {}: {:?}", t.tid, t.state))?;
        }
        Ok(())
    }

    /// M15: a specific thread's register dump, including a BLOCKED (non-current) one. `dbg_regs_of`
    /// returns `None` for an out-of-range id — that is a bad script input, not an internal bug, so it
    /// becomes an `Err` here (a usage error the CLI reports and exits 5 on, same as `break`'s 6-slot
    /// limit), never a panic on an unwrap.
    fn cmd_regs_of<W: Write>(&mut self, tid: u32, out: &mut W) -> Result<(), String> {
        // `tid` is u32 because that is what the CLI parses; the session's hooks take `usize`,
        // matching `Box_`. One cast, at the boundary where the u32 actually comes from.
        match self.sess().dbg_regs_of(tid as usize) {
            Some(dump) => line(out, format_args!("{dump}")),
            None => Err(format!("no such thread: {tid}")),
        }
    }

    /// M58 B4: `bt [tid]`. A frame-pointer walk of `tid` (default: the current thread) from its own registers:
    /// `ThreadCtx` is the live vCPU for the current thread and the saved context for any other.
    ///
    /// Whether `lr` is frame 1 is `unwind::walk`'s rule: `at_entry` (pc at offset 0 of its symbol, or at offset 4
    /// right after a `pacibsp`/`paciasp`), or `lr` differs from the record's return address at `fp` and does not
    /// point back into pc's own function. That covers a frameless leaf, a pc between `sub sp` and `stp`, and a
    /// syscall stub's `svc`. A leaf whose caller's own `lr`-equal record cannot be told apart is the residual limit.
    ///
    /// Frames from index 1 are return addresses: printed as such, but symbol and line are looked up at `pc - 4`, the
    /// call. One `atos` call covers every frame. Returns the JSON fields `frames` and, when there is one, `source_note`.
    fn cmd_bt<W: Write>(&mut self, tid: Option<u32>, out: &mut W) -> Result<Vec<(&'static str, J)>, String> {
        let tid = tid.unwrap_or_else(|| self.sess().current_thread());
        let ctx = self.sess().thread_ctx(tid as usize).ok_or_else(|| format!("no such thread: {tid}"))?;
        let (pc, fp, lr) = (ctx.regs.pc, ctx.regs.x[29], ctx.regs.x[30]);
        let n = self.n;
        let at_entry = match self.syms_at(n).resolve(pc) {
            Some((_, 0)) => true,
            Some((_, 4)) => self.sess().read_mem(pc - 4, 4)
                .is_some_and(|b| matches!(u32::from_le_bytes([b[0], b[1], b[2], b[3]]), 0xd503_237f | 0xd503_233f)),
            _ => false,
        };
        let frames = {
            let sess = self.sess();
            let read = |a: u64| -> Option<(u64, u64)> {
                let b = sess.read_mem(a, 16)?;
                Some((u64::from_le_bytes(b[..8].try_into().ok()?), u64::from_le_bytes(b[8..].try_into().ok()?)))
            };
            // Same function as pc: the same resolved symbol, or, when either is unresolved (shared-cache stubs),
            // an address below pc within a small window.
            let syms = self.syms_at(n);
            let here = syms.resolve(pc).map(|(r, _)| r);
            let same_fn = |a: u64| match (here, syms.resolve(a)) {
                (Some(h), Some((r, _))) => h == r,
                _ => a < pc && pc - a < 0x400,
            };
            crate::unwind::walk(pc, fp, lr, at_entry, same_fn, read, crate::unwind::MAX_FRAMES)
        };
        // Where each frame is looked up: its pc for frame 0, the call (pc - 4) for a return address.
        let probes: Vec<u64> = frames.iter().enumerate().map(|(i, f)| if i == 0 { f.pc } else { f.pc.saturating_sub(4) }).collect();
        let (lines, note) = self.source_lines(&probes);
        let mut jf = Vec::new();
        for (i, f) in frames.iter().enumerate() {
            let sym = self.syms_at(n).resolve(probes[i]).map(|(raw, off)| (self.readable_at(n, raw), off + (f.pc - probes[i])));
            let loc = lines.get(i).cloned().flatten();
            let mut text = format!("#{i} {:#x}", f.pc);
            if let Some((name, off)) = &sym { text += &format!(" in {name}+{off:#x}"); }
            if let Some((file, l)) = &loc { text += &format!(" at {file}:{l}"); }
            line(out, format_args!("{text}"))?;
            jf.push(J::Obj(vec![
                ("index", J::UInt(i as u64)),
                ("pc", hex(f.pc)),
                ("function", sym.as_ref().map_or(J::Null, |(n, _)| J::Str(n.clone()))),
                ("offset", sym.as_ref().map_or(J::Null, |(_, o)| J::UInt(*o))),
                ("file", loc.as_ref().map_or(J::Null, |(f, _)| J::Str(f.clone()))),
                ("line", loc.as_ref().map_or(J::Null, |(_, l)| J::UInt(u64::from(*l)))),
            ]));
        }
        let mut v = vec![("frames", J::Arr(jf))];
        if let Some(nt) = note {
            line(out, format_args!("{nt}"))?;
            v.push(("source_note", J::Str(nt)));
        }
        Ok(v)
    }

    /// M58 B5: move to (n, k) with phase `Bp` and print `where`'s line. A position the recording does not reach is
    /// an error naming the last one; the cursor stays where it was.
    fn cmd_goto<W: Write>(&mut self, n: usize, k: u64, out: &mut W) -> Result<(), String> {
        let at = (self.n, self.k, self.phase);
        if let Err(e) = self.reseek(n, k) {
            self.recover(at)?;
            let last = self.last_position()?;
            return Err(format!("goto ({n}, {k}): {e}; the recording's last position is ({}, {})", last.0, last.1));
        }
        self.cmd_where(out)
    }

    /// The recording's last position, the terminal's park. The cursor is restored.
    fn last_position(&mut self) -> Result<(usize, u64), String> {
        let at = (self.n, self.k, self.phase);
        let (bps, ws) = (std::mem::take(&mut self.breakpoints), std::mem::take(&mut self.watches));
        let r = self.cmd_continue(&mut std::io::sink());
        let pos = (self.n, self.k);
        self.breakpoints = bps;
        self.watches = ws;
        self.recover(at)?;
        match r? {
            Halt::Terminal(_) => Ok(pos),
            h => Err(format!("goto: no end of the recording was reached ({h:?})")),
        }
    }

    /// M58 B5: who wrote `len` bytes at `op` last, before the cursor. It arms a temporary unscoped watch (with the
    /// user's other watches and breakpoints off, so only this write can stop it), runs `reverse-continue`, and
    /// reads its `Halt`. Whatever happens the user's breakpoints and watches come back.
    fn cmd_last_write<W: Write>(&mut self, op: &Operand, len: u64, out: &mut W) -> Result<Vec<(&'static str, J)>, String> {
        let addr = self.addr_of(op)?;
        check_watch_align(addr, len)?;
        if self.watches.len() >= 4 {
            return Err("last-write needs a free watchpoint slot; 4 are armed (hardware limit: DBGWVR0-3)".into());
        }
        if self.watches.iter().any(|&(a, _, _)| a == addr) {
            return Err(format!("{addr:#x} is already watched; `unwatch {addr:#x}` before `last-write`"));
        }
        let at = (self.n, self.k, self.phase);
        let (bps, ws) = (std::mem::take(&mut self.breakpoints), std::mem::take(&mut self.watches));
        self.watches = vec![(addr, len, None)];
        let halt = self.cmd_reverse_continue(&mut std::io::sink());
        // The report steps the writer once, so it too runs with the user's breakpoints and watches still off.
        self.watches.clear();
        let rep = self.last_write_report(halt, addr, len, at, out);
        self.breakpoints = bps;
        self.watches = ws;
        rep
    }

    fn last_write_report<W: Write>(&mut self, halt: Result<Halt, String>, addr: u64, len: u64, at: (usize, u64, Phase), out: &mut W)
        -> Result<Vec<(&'static str, J)>, String> {
        let mut f: Vec<(&'static str, J)> = Vec::new();
        let hex_of = |b: Option<Vec<u8>>| b.map_or(J::Null, |b| J::Str(b.iter().map(|x| format!("{x:02x}")).collect()));
        match halt? {
            Halt::NoEarlierHit => {
                line(out, format_args!("no earlier write to {addr:#x}: it holds what it held when the program started"))?;
                f.push(("found", J::Bool(false)));
                f.push(("reason", J::Str("start".into())));
            }
            Halt::Exec { .. } => {
                // The scan met an exec first: the image that held this address is gone, and no write is later.
                let n = self.n;
                self.recover(at)?;
                line(out, format_args!("{}", no_write_since_exec(addr, n)))?;
                f.push(("found", J::Bool(false)));
                f.push(("reason", J::Str("exec".into())));
            }
            Halt::Watch { .. } => {
                let (n, k) = (self.n, self.k);
                let (pc, thread) = (self.sess().pc(), self.sess().current_thread());
                let before = self.sess().read_mem(addr, len as usize);
                // One step of the writing thread retires the store; a window end leaves `value_after` null.
                let after = match self.cmd_stepi(1, &mut std::io::sink())? {
                    Halt::Stepped => self.sess().read_mem(addr, len as usize),
                    _ => None,
                };
                self.reseek(n, k)?;
                self.phase = Phase::Watch;
                let sym = self.syms_at(n).resolve(pc).map(|(raw, off)| (self.readable_at(n, raw), off));
                let (lines, _) = self.source_lines(&[pc]);
                let loc = lines.into_iter().next().flatten();
                let mut text = format!("last write to {addr:#x} ({len} bytes): store at {pc:#x}");
                if let Some((name, off)) = &sym { text += &format!(" in {name}+{off:#x}"); }
                if let Some((file, l)) = &loc { text += &format!(" at {file}:{l}"); }
                text += &format!(" on thread {thread}, ({n}, {k})");
                line(out, format_args!("{text}"))?;
                f.push(("found", J::Bool(true)));
                f.push(("kind", J::Str("store".into())));
                f.push(("value_before", hex_of(before)));
                f.push(("value_after", hex_of(after)));
                f.extend(self.cmd_bt(None, out)?);
            }
            Halt::WatchSys { thread, .. } => {
                let n = self.n;
                // The write belongs to the event that ended window n - 1: park before it to read its number.
                self.park_before_event(n)?;
                let num = self.sess().peek_syscall().map(|(num, _)| num)
                    .ok_or_else(|| format!("last-write: no syscall event ends window {}", n - 1))?;
                self.reseek(n, 0)?;
                self.phase = Phase::Sys;
                let name = retrace_arch::syscall_name(num).map_or_else(|| format!("syscall{num}"), str::to_string);
                let after = self.sess().read_mem(addr, len as usize);
                line(out, format_args!("last write to {addr:#x}: syscall {name} ({num}) on thread {thread}, ({n}, 0)"))?;
                f.push(("found", J::Bool(true)));
                f.push(("kind", J::Str("syscall".into())));
                f.push(("thread", J::UInt(u64::from(thread))));
                f.push(("syscall", J::Obj(vec![("number", J::UInt(num)), ("name", J::Str(name))])));
                f.push(("value_before", J::Null));
                f.push(("value_after", hex_of(after)));
            }
            h => return Err(format!("last-write: unexpected stop {h:?}")),
        }
        Ok(f)
    }

    fn cmd_where<W: Write>(&mut self, out: &mut W) -> Result<(), String> {
        let pc = self.sess().pc();
        let a = self.annot(self.n, pc);
        line(out, format_args!("at ({}, {}) pc={pc:#x} thread={}{a}",
            self.n, self.k, self.sess().current_thread()))
    }

    fn cmd_examine<W: Write>(&mut self, addr: u64, len: usize, out: &mut W) -> Result<(), String> {
        match self.sess().read_mem(addr, len) {
            Some(bytes) => {
                let hex = bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
                line(out, format_args!("{addr:#x}: {hex}"))
            }
            None => line(out, format_args!("unmapped")),
        }
    }

    /// Step forward `count` instructions in place. On the window boundary the primitive errs
    /// (naming the window length) and spends the session — print the length clause and re-seek to
    /// the unchanged pre-command coordinate. NOTE: `M` in the error line is the number of
    /// instructions REMAINING from the current step K to the window end (i.e. window_len − K), which
    /// equals the full window length only when stepped from K = 0.
    pub(crate) fn cmd_stepi<W: Write>(&mut self, count: u64, out: &mut W) -> Result<Halt, String> {
        if count > 0 { if let Some(h) = self.step_at_pending_terminal(out)? { return Ok(h); } }
        let res = self.sess_mut().step_insns(count);
        match res {
            // An arrival (R4) only if it moved: `stepi 0` stands still, so the cursor stays (§3b).
            Ok(()) => { self.k += count; if count > 0 { self.phase = Phase::Bp; } Ok(Halt::Stepped) }
            Err(msg) => {
                // M54 §3i (T6-d): standing on the svc of an exec, the step is the exec: it crosses the
                // landmark and stops there, as `continue` does.
                if count > 0 && self.exec_ahead(self.n) {
                    let (n0, k0, p0) = (self.n, self.k, self.phase);
                    if self.probe_window_len(n0)? == k0 {
                        self.reseek(n0, k0)?;
                        let adv = self.sess_mut().advance().map_err(|d|
                            format!("stepi diverged at landmark {} pc {:#x}: {}", d.landmark, d.pc, d.detail))?;
                        match adv {
                            Advance::Exited(report) => return self.park_at_terminal(report, out),
                            Advance::Event => {
                                let n = self.sess().landmark();
                                if let Some(h) = self.exec_stop(n, out)? { return Ok(h); }
                                return Err("stepi: the exec trap's landmark is not an exec stop".into());
                            }
                            _ => return Err("stepi: a hardware stop during a one-event crossing".into()),
                        }
                    }
                    self.reseek(n0, k0)?;
                    self.phase = p0;
                }
                let head = msg.split("; cannot step").next().unwrap_or(&msg);
                line(out, format_args!("error: {head}"))?;
                // The position did not change, so neither does the cursor.
                let (n0, k0, p0) = (self.n, self.k, self.phase);
                self.reseek(n0, k0)?;
                self.phase = p0;
                Ok(Halt::Refused(head.to_string()))
            }
        }
    }

    /// Fix round 2 of final fix 2 (N1): at a position where a C5 terminal is pending (an exec that released a fatal
    /// signal, parked at its exec stop `(n, 0)`), the process is already dead natively, so a forward step runs
    /// nothing: it reports the terminal as `continue` does and the cursor stays at `(n, 0)`. `None` anywhere else.
    fn step_at_pending_terminal<W: Write>(&mut self, out: &mut W) -> Result<Option<Halt>, String> {
        if self.sess().terminal_pending().is_none() { return Ok(None); }
        match self.sess_mut().advance().map_err(|d| format!("step diverged at landmark {} pc {:#x}: {}", d.landmark, d.pc, d.detail))? {
            Advance::Exited(report) => self.park_at_terminal(report, out).map(Some),
            _ => Err("step: a pending terminal did not end the run".into()),
        }
    }

    /// Step backward `count` instructions by coordinate arithmetic, crossing a landmark boundary via
    /// one probe of the previous window's length, then one final seek to the resolved coordinate. At
    /// (1, 0) it cannot go earlier: print `at start of recording` and stop.
    pub(crate) fn cmd_reverse_stepi<W: Write>(&mut self, count: u64, out: &mut W) -> Result<Halt, String> {
        let (mut n, mut k) = (self.n, self.k);
        let mut at_start = false;
        for _ in 0..count {
            if k > 0 {
                k -= 1;
            } else if n > 1 {
                n -= 1;
                k = self.probe_window_len(n)?;
            } else {
                at_start = true; // (1, 0): the start of the recording
                break;
            }
        }
        if at_start {
            line(out, format_args!("at start of recording"))?;
        }
        // An arrival (R4) only if it moved. `reverse-stepi 0`, or one at (1, 0), stands still, so
        // the cursor keeps its phase (§3b).
        let (moved, p0) = ((n, k) != (self.n, self.k), self.phase);
        self.reseek(n, k)?;
        if !moved { self.phase = p0; }
        Ok(if at_start { Halt::AtStart } else { Halt::Stepped })
    }

    /// Report a terminal `Advance::Exited` and park the session on it. Both of `cmd_continue`'s
    /// `Exited` arms (the scan, and the finish's one-event crossing of a trap or a fault, R10) route
    /// here so the two stay identical.
    ///
    /// Every terminal (an `exit`, a crash, a fatal signal) parks at `(T, K_f)`: `T` is the terminal
    /// event's own window, which `landmark()` still names because `advance()` reports `Exited`
    /// without consuming a landmark, and `K_f` is that window's full length, the count of its
    /// instructions that DID retire. Stepping that far leaves the guest immediately before the one
    /// instruction that never retires, the exit `svc` or the faulting instruction, so `pc()` IS
    /// that instruction. The cursor's phase there is `Watch`, the last phase at a coordinate, so
    /// the terminal sits AFTER every hit in the recording, a breakpoint on that last instruction
    /// included: a following `reverse-continue` finds the last of them (which is what makes "run
    /// backward from the crash to the corrupting store" work), and `continue` just reports the end
    /// again. Until M41's final review (R18) an exit parked at `(E, 0)`, before its own window,
    /// so a hit in the exit window was lost backward and re-reported by every `continue` after
    /// the exit; and every terminal took phase `Bp`, so a breakpoint on the terminal instruction
    /// itself was lost backward.
    fn park_at_terminal<W: Write>(&mut self, report: ReplayReport, out: &mut W) -> Result<Halt, String> {
        match &report.outcome {
            Outcome::Exit { code } => line(out, format_args!("exited (code {code})"))?,
            Outcome::Crash { pc, esr, far, .. } => {
                let a = self.annot(self.sess().landmark(), *pc);
                line(out, format_args!("guest crashed: pc={pc:#x} far={far:#x} esr={esr:#x}{a}"))?
            }
            // M11: terminal like a crash, and it inherits the same seek machinery — but it is NOT
            // presented as one. Calling a SIGABRT a fault is the same lie that got Event::Crash
            // reuse rejected in the spec, and the debug output would carry it forever.
            Outcome::Signal { sig } => line(out, format_args!("guest terminated by signal {sig}"))?,
            Outcome::Refused(_) | Outcome::Incomplete(_) => unreachable!("M50 R5: a debugger opens only a complete recording, and replay never refuses one"),
        }
        let t = self.sess().landmark();
        let kf = self.probe_window_len(t)?; // drops the live session (one VM per process)
        self.reseek(t, kf)?;
        self.phase = Phase::Watch; // after every hit, the terminal instruction's breakpoint too
        Ok(Halt::Terminal(report))
    }

    /// Run forward to the first hit after the cursor (M41 §3c), or to the guest's end.
    ///
    /// **Finish** the current coordinate first:
    /// - a breakpoint that a syscall hit left ahead of the cursor at (n, 0) is reported;
    /// - if the cursor stands on a breakpoint or on a watched store, the instruction is executed,
    ///   with the watches armed unless the cursor is ON its store. So a store under a breakpoint
    ///   still reports (t0 M3), and one already reported is stepped over;
    /// - a trap or a fault there is crossed with `advance()`, with the watches armed for that one
    ///   event (R10).
    ///
    /// Then **scan** at native speed with hardware breakpoints (one DBGBVR slot each; ≤ 6) and
    /// watchpoints armed. A mid-window hit resolves to an exact (N, K) from `kctx`, and the landmark
    /// check catches a breakpoint exactly on a boundary. With nothing armed, runs to the end.
    pub(crate) fn cmd_continue<W: Write>(&mut self, out: &mut W) -> Result<Halt, String> { self.continue_until(None, out) }

    /// `cmd_continue`'s run, with one extra stop (M43 §3d): with `until: Some(t)`, arriving at a
    /// boundary where thread `t` is current ends the run as `Halt::Stepped`, before anything at that
    /// coordinate. `step_thread` uses it when the stepped thread blocked, and when it is not running
    /// (M44 B6(b)). With `None` it is `continue` exactly.
    ///
    /// With `Some(t)` the user's hits are armed as for `continue` (M44 B6(a)), and a hit by another
    /// thread during one such step ends it: `run_until_thread` reports it on `t`. M43 armed
    /// nothing here (spec R7's fallback), because lldb-2100 re-stepped its thread forever when the
    /// hit was answered on the other thread (Task 4's measurement: 307,016 × `vCont;s:1` in 60 s).
    pub(crate) fn continue_until<W: Write>(&mut self, until: Option<u32>, out: &mut W) -> Result<Halt, String> {
        // M44 B6(a): an until-run arms the user's hits too; `run_until_thread` turns a hit into
        // `StepInterrupted`. (M43's R7 fallback armed nothing, because the hit was reported on the
        // other thread and lldb looped on that; t0 L7's form names the stepped thread instead.)
        let (bps, ws): (Vec<u64>, Vec<(u64, u64)>) =
            (self.breakpoints.clone(), self.watches.iter().map(|&(a, l, _)| (a, l)).collect());
        let diverged = |d: retrace_core::Divergence|
            format!("continue diverged at landmark {} pc {:#x}: {}", d.landmark, d.pc, d.detail);
        'finish: loop {
            // ---- Finish (n, k): its hits still ahead of the cursor ----
            loop {
                // M43 §3d's arrival invariant: end when `t` is current, before anything at that
                // coordinate (M41 R4). Load-bearing since M44 B6(b): a step of a thread that is not
                // running enters the finish wherever the cursor stands. If the running thread stands
                // on a user breakpoint at a blocking trap (lldb lifts a breakpoint only for a thread
                // it resumes), the finish crosses that trap to (n + 1, 0, Sys), where `t` may arrive.
                // This check ends the run there; the scan would start past the arrival and run `t`'s
                // window (`gdbserver_e2e`'s `…past_the_running_threads_breakpointed_trap` row). A
                // blocked step enters at (n, 0, Sys) with another thread current, and crosses to an
                // arrival here only after a scoped-out watch (`continue 'finish` below), which the
                // gdbserver cannot set.
                if let Some(t) = until {
                    if self.k == 0 && self.phase == Phase::Sys && self.sess().current_thread() == t {
                        self.phase = Phase::Bp;
                        return Ok(Halt::Stepped);
                    }
                }
                let pc = self.sess().pc();
                let on_bp = bps.contains(&pc);
                if self.phase == Phase::Sys && on_bp {
                    // A breakpoint behind a syscall hit at (n, 0) is still ahead (t0 M5).
                    let a = self.annot(self.n, pc);
                    line(out, format_args!("hit {pc:#x} at ({}, 0){a}", self.n))?;
                    self.phase = Phase::Bp;
                    return Ok(Halt::Break);
                }
                if !on_bp && self.phase != Phase::Watch {
                    break; // nothing here is behind the cursor: the scan reports whatever is next
                }
                // Execute the instruction at (n, k). Its store is still ahead of the cursor unless
                // the cursor is ON it, so arm the watches exactly then.
                let arm = self.phase != Phase::Watch;
                if arm { self.sess_mut().arm_watchpoints(&ws); }
                let stepped = self.sess_mut().step_armed();
                if arm { self.sess_mut().clear_watchpoints(); } // the kept-session invariant
                match stepped? {
                    Armed::Retired => { self.k += 1; break; } // (n, k + 1): nothing there examined yet
                    Armed::Watch => {
                        let (n, k) = (self.n, self.k);
                        let watched = watched_of(&ws, self.sess().far());
                        let thread = self.sess().current_thread();
                        self.phase = Phase::Watch; // parked pre-retire on the store
                        if self.watch_thread_matches(watched, thread) {
                            let a = self.annot(n, pc);
                            line(out, format_args!("hit watch {watched:#x} (write at {pc:#x}) at ({n}, {k}){a}"))?;
                            return Ok(Halt::Watch { watched });
                        }
                        // Scoped out (M15 Task 8): passed, not reported. The next turn steps over it.
                    }
                    Armed::AtTrap | Armed::Fault => {
                        // The instruction ends the window (a trap) or faults (R10): nothing retired.
                        // `advance()` consumes the trap, or delivers or ends the fault, from a fresh
                        // session parked before it — the pre-M41 crossing's own move — with the
                        // watches armed for that one event, so a syscall write to a watched range is
                        // reported (M5 final-review M-1), and breakpoints off, so the parked
                        // position is not re-reported. No instruction retires during it.
                        let (n, k) = (self.n, self.k);
                        self.reseek(n, k)?;
                        self.sess_mut().arm_watchpoints(&ws);
                        let adv = self.sess_mut().advance().map_err(diverged)?;
                        match adv {
                            Advance::Exited(report) => return self.park_at_terminal(report, out),
                            Advance::WatchSyscall { watched, thread } if self.watch_thread_matches(watched, thread) => {
                                self.sess_mut().clear_watchpoints();
                                let n = self.sess().landmark();
                                line(out, format_args!("hit watch {watched:#x} (syscall write) at ({n}, 0)"))?;
                                (self.n, self.k, self.phase) = (n, 0, Phase::Sys);
                                return Ok(Halt::WatchSys { watched, thread });
                            }
                            Advance::Event | Advance::WatchSyscall { .. } => {
                                // A plain event, or a syscall write scoped out by thread: at
                                // (n + 1, 0) only the Sys phase is behind the cursor.
                                self.sess_mut().clear_watchpoints();
                                let n = self.sess().landmark();
                                (self.n, self.k, self.phase) = (n, 0, Phase::Sys);
                                // M54 §3i: the trap crossed was an exec, so the stop is at its landmark.
                                if let Some(h) = self.exec_stop(n, out)? { return Ok(h); }
                            }
                            Advance::Break | Advance::Watch { .. } => return Err(
                                "continue: a hardware stop during a one-event crossing (breakpoints are off \
                                 and the guest is parked on the trap or fault)".into()),
                        }
                    }
                    Armed::Break => return Err(
                        "continue: a breakpoint stop with no breakpoint armed (the kept-session invariant)".into()),
                }
            }
            // ---- Scan from (n, k): the finish left nothing here behind the cursor ----
            let (start_n, start_k) = (self.n, self.k);
            self.sess_mut().arm_breakpoints(&bps);
            self.sess_mut().arm_watchpoints(&ws);
            loop {
                match self.sess_mut().advance().map_err(diverged)? {
                    Advance::Break => {
                        let n = self.sess().landmark();
                        let p_hit = self.sess().pc();
                        let a = self.annot(n, p_hit);
                        line(out, format_args!("hit {p_hit:#x} at ({n}, +?){a}"))?;
                        // M40's R11, fixed: resolve FROM kctx, not kctx + 1. In the start window
                        // kctx is the scan's start, where a breakpoint is still ahead (the finish
                        // stepped off any it had passed); in a later window kctx is 0, and a hit at
                        // (n, 0) is found rather than skipped.
                        let kctx = if n == start_n { start_k } else { 0 };
                        self.session = None; // free the VM before the resolution seek
                        let k = resolve_nth(self.trace, &mut self.cache, n, kctx, HitKind::Break(&[p_hit]), 1, p_hit)?;
                        line(out, format_args!("resolved ({n}, {k})"))?;
                        self.reseek(n, k)?; // phase Bp
                        return Ok(Halt::Break);
                    }
                    Advance::Event => {
                        // M54 §3i (C6): an exec landmark ends the run, before anything at its position.
                        let n = self.sess().landmark();
                        if let Some(h) = self.exec_stop(n, out)? { return Ok(h); }
                        if let Some(t) = until {
                            if self.sess().current_thread() == t {
                                let n = self.sess().landmark();
                                self.sess_mut().clear_breakpoints(); // keep this session, hit-clean
                                self.sess_mut().clear_watchpoints();
                                (self.n, self.k, self.phase) = (n, 0, Phase::Bp);
                                return Ok(Halt::Stepped);
                            }
                        }
                        let pc = self.sess().pc(); // the INCOMING thread's after a block (§3a)
                        if bps.contains(&pc) {
                            let n = self.sess().landmark();
                            let a = self.annot(n, pc);
                            line(out, format_args!("hit {pc:#x} at ({n}, 0){a}"))?;
                            self.sess_mut().clear_breakpoints(); // keep this session, hit-clean
                            self.sess_mut().clear_watchpoints();
                            (self.n, self.k, self.phase) = (n, 0, Phase::Bp);
                            return Ok(Halt::Break);
                        }
                        // no boundary match; keep scanning (hardware breakpoints stay armed)
                    }
                    Advance::Exited(report) => return self.park_at_terminal(report, out),
                    // Both watch arms name their `thread` field (not `..`): M15 Task 5 plumbs the
                    // writing thread through `Advance`, and naming it keeps every site that drops
                    // it greppable.
                    Advance::Watch { thread } => {
                        let n = self.sess().landmark();
                        let p_hit = self.sess().pc();
                        let watched = watched_of(&ws, self.sess().far());
                        let matched = self.watch_thread_matches(watched, thread);
                        if matched {
                            let a = self.annot(n, p_hit);
                            line(out, format_args!("hit watch {watched:#x} (write at {p_hit:#x}) at ({n}, +?){a}"))?;
                        }
                        // The FIRST watch stop from kctx is this hit, found by the hardware BY
                        // ADDRESS (M40). This resolution runs whether or not the hit is scoped out:
                        // the vCPU is physically parked pre-retire at the store either way.
                        let kctx = if n == start_n { start_k } else { 0 };
                        self.session = None; // free the VM before the resolution seek
                        let k = resolve_nth(self.trace, &mut self.cache, n, kctx, HitKind::Watch(&ws), 1, p_hit)?;
                        if matched {
                            line(out, format_args!("resolved ({n}, {k})"))?;
                        }
                        self.reseek(n, k)?;
                        self.phase = Phase::Watch;
                        if matched {
                            return Ok(Halt::Watch { watched });
                        }
                        // Scoped out (M15 Task 8): the finish steps over it. R8: a loop where M15
                        // recursed into this function, which could overflow the stack on a hot
                        // scoped-out write loop.
                        continue 'finish;
                    }
                    Advance::WatchSyscall { watched, thread } => {
                        if self.watch_thread_matches(watched, thread) {
                            // Reported even when this event is also `t`'s arrival: the write sits at
                            // (n, 0, Sys), which orders before the arrival's (n, 0, Bp).
                            let n = self.sess().landmark();
                            line(out, format_args!("hit watch {watched:#x} (syscall write) at ({n}, 0)"))?;
                            self.sess_mut().clear_breakpoints(); // keep this session, hit-clean
                            self.sess_mut().clear_watchpoints();
                            (self.n, self.k, self.phase) = (n, 0, Phase::Sys);
                            return Ok(Halt::WatchSys { watched, thread });
                        }
                        // Scoped out: the writing event is already consumed, so keep scanning. That
                        // skips the `Event` arm's arrival check, so an until-run would scan on past
                        // `t`'s resume point if this event were it. Unreachable today: only a
                        // thread-scoped watch is scoped out, and the gdbserver cannot scope one.
                        assert!(until.is_none(),
                            "an until-run met a thread-scoped watch: scanning on past its scoped-out syscall \
                             write would overshoot the stepped thread's resume point");
                    }
                }
            }
        }
    }

    /// M43 §3d: one instruction of thread `t` (retrace's id), as lldb's `s` needs it. A `t` that is
    /// live but not running is first run to until it is scheduled (M44 B6(b)). It crosses a
    /// trap, and a blocking syscall until `t` runs again, unless another thread's hit ends it first
    /// (`StepInterrupted`, M44 B6(a)), or the recording ends first on another thread (`Refused`,
    /// Ruling T12-a). A trap that is `t`'s own exit stops at the exit's boundary as `ThreadExited`
    /// (M44 B3), because nothing runs `t` again. A trap that returns to itself is
    /// never `Stepped`, because lldb re-steps a `trace` whose pc did not move forever (t0 L4b, L7).
    /// An instruction that branches to itself (`b .`) does retire with its pc unchanged and is
    /// `Stepped`: that is one real instruction, and lldb loops on it on a live target too.
    pub(crate) fn step_thread<W: Write>(&mut self, t: u32, out: &mut W) -> Result<Halt, String> {
        if let Some(h) = self.step_at_pending_terminal(out)? { return Ok(h); }
        let cur = self.sess().current_thread();
        if t != cur {
            // M44 B6(b): a live thread that is not running is run to until it is — the blocked
            // step's tail — then stepped one instruction. One that has exited, or never existed,
            // is still refused in place (§3d rule 1).
            let live = self.sess().thread_summaries().iter()
                .any(|s| s.tid == t && !matches!(s.state, ThreadState::Exited(_)));
            if !live {
                return Ok(Halt::Refused(format!("cannot step thread {}: it is not a live thread", t + 1)));
            }
            return match self.run_until_thread(t, false, out)? {
                Halt::Stepped => self.step_thread(t, out),
                other => Ok(other),
            };
        }
        let ws: Vec<(u64, u64)> = self.watches.iter().map(|&(a, l, _)| (a, l)).collect();
        let pc0 = self.sess().pc();
        self.sess_mut().arm_watchpoints(&ws);
        let stepped = self.sess_mut().step_armed();
        self.sess_mut().clear_watchpoints(); // the kept-session invariant
        match stepped? {
            Armed::Retired => { self.k += 1; self.phase = Phase::Bp; Ok(Halt::Stepped) }
            Armed::Watch => {
                let watched = watched_of(&ws, self.sess().far());
                self.sess_mut().step_insns(1)?; // retire it with nothing armed
                self.k += 1;
                self.phase = Phase::Bp;
                Ok(Halt::WatchStepped { watched })
            }
            Armed::AtTrap | Armed::Fault => {
                // cmd_continue's own crossing (R10): a fresh session parked before the trap, the
                // watches armed for this one event.
                let (n, k) = (self.n, self.k);
                self.reseek(n, k)?;
                self.sess_mut().arm_watchpoints(&ws);
                let adv = self.sess_mut().advance()
                    .map_err(|d| format!("step diverged at landmark {} pc {:#x}: {}", d.landmark, d.pc, d.detail));
                self.sess_mut().clear_watchpoints();
                match adv? {
                    Advance::Exited(report) => self.park_at_terminal(report, out),
                    Advance::WatchSyscall { watched, thread } => {
                        let n = self.sess().landmark();
                        (self.n, self.k, self.phase) = (n, 0, Phase::Sys);
                        Ok(Halt::WatchSys { watched, thread })
                    }
                    Advance::Event => {
                        let n = self.sess().landmark();
                        // M54 §3i (T6-d): a step across the exec svc is the exec stop.
                        if let Some(h) = self.exec_stop(n, out)? { return Ok(h); }
                        if self.sess().current_thread() == t {
                            (self.n, self.k, self.phase) = (n, 0, Phase::Bp);
                            if self.sess().pc() == pc0 {
                                // The cursor HAS crossed the event, to (n, 0). Only the pc stayed.
                                return Ok(Halt::Refused(format!(
                                    "the step crossed a trap that returns to itself: the replay moved \
                                     on to landmark {n}, but thread {}'s pc {pc0:#x} did not move", t + 1)));
                            }
                            return Ok(Halt::Stepped);
                        }
                        // M44 B3: or `t` exited, crossing its own exit. Nothing will run it again, so
                        // the until-run below would reach the end of the recording; stop here. Phase
                        // Sys, as the blocked arm below parks (M44 final review, Ruling FW-b): this is
                        // the crossing's own position, not an arrival of the thread now running, so a
                        // breakpoint at that thread's pc is still ahead and the next `c` reports it.
                        let exited = self.sess().thread_summaries().iter()
                            .any(|s| s.tid == t && matches!(s.state, ThreadState::Exited(_)));
                        if exited {
                            (self.n, self.k, self.phase) = (n, 0, Phase::Sys);
                            return Ok(Halt::ThreadExited { thread: t });
                        }
                        // `t` blocked. Run until it is current again, until another thread's hit
                        // (M44 B6(a)), or until the recording's end (Ruling T12-a, a refusal on
                        // `t`). Phase Sys: the crossing's own position, as `continue`'s
                        // finish leaves it, so a breakpoint at the incoming thread's pc is ahead.
                        (self.n, self.k, self.phase) = (n, 0, Phase::Sys);
                        self.run_until_thread(t, true, out)
                    }
                    Advance::Break | Advance::Watch { .. } => Err(
                        "step: a hardware stop during a one-event crossing (breakpoints are off and the \
                         guest is parked on the trap or fault)".into()),
                }
            }
            Armed::Break => Err("step: a breakpoint stop with no breakpoint armed".into()),
        }
    }

    /// Run until thread `t` is current (M43 §3d's until-run), with the user's hits armed: `again`
    /// after `t` blocked in its own step, or for the first time from the cursor in a step of a `t`
    /// that is not running (M44 B6(b)). A hit on the way ends the run as `StepInterrupted` on `t`
    /// (M44 B6(a)), parked where `continue` parks that hit; the server re-parks it as it does
    /// `continue`'s (Ruling T11-a).
    ///
    /// The recording's end on another thread, before `t` runs, is `t`'s refusal, naming that
    /// terminal, with the cursor parked at it (Ruling T12-a). Answered as the other thread's
    /// terminal, the stop named a thread lldb-2100 had suspended for the step, so lldb ignored it
    /// and re-stepped `t` without end (the review's probe on crashthread: 128 × `vCont;s:1` in
    /// 30 s). Each re-step reached the same terminal again.
    fn run_until_thread<W: Write>(&mut self, t: u32, again: bool, out: &mut W) -> Result<Halt, String> {
        // A breakpoint's or a store's thread is the current one. A syscall write's is the thread
        // that issued it, which is the `thread` that `Advance::WatchSyscall` carries, not whichever
        // thread is current after it. That is right whether or not the syscall blocked: one that
        // blocked has already switched away from its thread, and one that did not has not. A
        // terminal's is the current one, as the server reports it.
        Ok(match self.continue_until(Some(t), out)? {
            Halt::Break => {
                let by = self.sess().current_thread();
                Halt::StepInterrupted { thread: t, by, hit: Hit::Break }
            }
            Halt::Watch { watched } => {
                let by = self.sess().current_thread();
                Halt::StepInterrupted { thread: t, by, hit: Hit::Watch { watched } }
            }
            Halt::WatchSys { watched, thread: by } => Halt::StepInterrupted { thread: t, by, hit: Hit::WatchSys { watched } },
            Halt::Terminal(r) if self.sess().current_thread() != t => {
                let by = self.sess().current_thread() + 1;
                let what = match r.outcome {
                    Outcome::Exit { code } => format!("the guest exited (code {code}) on thread {by}"),
                    Outcome::Crash { pc, esr, far, .. } => format!("thread {by} crashed: pc={pc:#x} far={far:#x} esr={esr:#x}"),
                    Outcome::Signal { sig } => format!("the guest was terminated by signal {sig} on thread {by}"),
                    Outcome::Refused(_) | Outcome::Incomplete(_) => unreachable!("M50 R5: a debugger opens only a complete recording, and replay never refuses one"),
                };
                Halt::Refused(format!("the recording ended ({what}) before thread {} ran{}",
                    t + 1, if again { " again" } else { "" }))
            }
            other => other,
        })
    }

    /// Run backward to the last hit — breakpoint, hardware watch, or syscall watch — before the
    /// cursor P = (n, k, phase) in M41's hit order (spec §3b, §3d; M40 §3b for the one pass).
    /// Replay only runs forward, so this makes ONE
    /// forward pass from landmark 1 and remembers the last qualifying hit:
    /// - phase 1 runs every window before P's at native speed, stepping over each hit in place;
    /// - phase 2 single-steps P's own window up to P, so exactly the hits before P count.
    ///
    /// Then it resolves only that one hit (`resolve_nth`, by ordinal) and parks there: at most three
    /// seeks, however many hits the pass went through. The pre-M40 loop re-seeked and resolved at
    /// every hit it passed, by pc, so a store that ran on other addresses first cost a full
    /// iteration per run (t0 M2/M6: 183 runs in one window of rung 8, ~20.8 s CPU each).
    ///
    /// A scoped-out watch hit still occupies an ordinal, because the hardware fired for it. The
    /// thread FILTER applies only where `last` is decided (M15 Task 8; spec R4). A breakpoint on a
    /// watched store yields both hits, breakpoint first (R3): its step-off keeps the watches armed.
    pub(crate) fn cmd_reverse_continue<W: Write>(&mut self, out: &mut W) -> Result<Halt, String> {
        enum RHit { Exec, Bp { pc: u64, ord: u64 }, Watch { watched: u64, pc: u64, ord: u64 }, WatchSys { watched: u64, thread: u32 } }
        let (pn, pk, pphase) = (self.n, self.k, self.phase);
        let bps = self.breakpoints.clone();
        let ws: Vec<(u64, u64)> = self.watches.iter().map(|&(a, l, _)| (a, l)).collect();
        self.session = None; // the scan and the resolution use their own transient sessions
        let mut last: Option<(usize, RHit)> = None;
        {
            let mut s = checkpointed_seek(self.trace, &mut self.cache, 1, 0)?;
            s.arm_breakpoints(&bps);
            s.arm_watchpoints(&ws);
            let mut win = s.landmark();
            let (mut bp_ord, mut w_ord) = (0u64, 0u64); // hits so far in window `win`
            let mut ended = false;
            let mut bps_off_for_crossing = false;
            // Phase 1: every window before P's, at native speed.
            while s.landmark() < pn {
                let adv = s.advance().map_err(|d| format!("reverse-continue diverged: {}", d.detail))?;
                if bps_off_for_crossing { s.arm_breakpoints(&bps); bps_off_for_crossing = false; }
                let n = s.landmark();
                if n != win { win = n; bp_ord = 0; w_ord = 0; }
                match adv {
                    // M54 §3i (C6): an exec landmark before P is a stop, at its arrival (n, 0, Bp).
                    Advance::Event => {
                        if self.exec_at(n).is_some() && (n, 0u64, Phase::Bp) < (pn, pk, pphase) {
                            last = Some((n, RHit::Exec));
                        }
                    }
                    // Exited covers BOTH terminals (exit and crash): either way the scan is over.
                    Advance::Exited(_) => { ended = true; break; }
                    Advance::Break => {
                        let pc = s.pc();
                        bp_ord += 1;
                        last = Some((n, RHit::Bp { pc, ord: bp_ord })); // breakpoints are never scoped
                        s.clear_breakpoints(); // step off it with the watches still armed (R3)
                        match s.step_watched()? {
                            Stepped::Retired => s.arm_breakpoints(&bps),
                            Stepped::Watch => {
                                w_ord += 1;
                                let (watched, thread) = (watched_of(&ws, s.far()), s.current_thread());
                                if self.watch_thread_matches(watched, thread) {
                                    last = Some((n, RHit::Watch { watched, pc, ord: w_ord }));
                                }
                                s.clear_watchpoints();
                                s.step_insns(1)?;
                                s.arm_watchpoints(&ws);
                                s.arm_breakpoints(&bps);
                            }
                            // The breakpoint is ON the window-ending trap, so nothing retires
                            // before it: step() left the guest parked at EL1 on the vector slot
                            // head (the trap itself, unconsumed), not re-seeked to EL0 on the svc
                            // the way `continue` is. The next advance() reaches that same
                            // Stop::Syscall, consumes it, and lands at (n+1, 0) — breakpoints stay
                            // off for exactly that one call (re-armed right after, above) so a
                            // breakpoint address coinciding with (n+1, 0) cannot re-fire against
                            // an instruction that never retires.
                            Stepped::AtTrap => bps_off_for_crossing = true,
                        }
                    }
                    Advance::Watch { thread } => {
                        w_ord += 1;
                        let (watched, pc) = (watched_of(&ws, s.far()), s.pc());
                        if self.watch_thread_matches(watched, thread) {
                            last = Some((n, RHit::Watch { watched, pc, ord: w_ord }));
                        }
                        // Step over the watched store in place: retire it with nothing armed, re-arm.
                        s.clear_breakpoints();
                        s.clear_watchpoints();
                        s.step_insns(1)?;
                        s.arm_breakpoints(&bps);
                        s.arm_watchpoints(&ws);
                    }
                    Advance::WatchSyscall { watched, thread } => {
                        // M41 §3d: a syscall write sits at (n, 0, Sys). It is before P unless P is
                        // that very hit (the cursor at (pn, 0, Sys)) — M40's stuck-loop guard (its
                        // Ruling 2) is that one case of this comparison. An ARRIVAL at (n, 0) has
                        // phase Bp, so the write just behind it counts (t0 M6, M7; spec R5).
                        if (n, 0u64, Phase::Sys) < (pn, pk, pphase) && self.watch_thread_matches(watched, thread) {
                            last = Some((n, RHit::WatchSys { watched, thread }));
                        }
                    }
                }
            }
            // Phase 2: P's own window, instruction by instruction, so exactly the hits before P count.
            if !ended {
                if s.landmark() != pn {
                    return Err(format!("reverse-continue: the scan overshot landmark {pn} (at {})", s.landmark()));
                }
                if win != pn { bp_ord = 0; w_ord = 0; }
                s.clear_breakpoints(); // compared by pc below
                let mut k = 0u64;
                while k < pk {
                    // Read BEFORE the step: the breakpoint check compares the pc about to execute.
                    // At (pn, 0) after a blocking event that is the INCOMING thread's (M41 §3a).
                    let pc = s.pc();
                    if bps.contains(&pc) {
                        bp_ord += 1;
                        last = Some((pn, RHit::Bp { pc, ord: bp_ord }));
                    }
                    match s.step_watched()? {
                        Stepped::Retired => k += 1,
                        Stepped::Watch => {
                            w_ord += 1;
                            // The store's pc is read AFTER the step: the guest is parked pre-retire
                            // on it. Since M41 §3a no thread switch is pending at a position the
                            // debugger observes, so this equals the pc read before the step; it is
                            // read after anyway, where the stop is (M40 final review).
                            let (watched, thread, pc) = (watched_of(&ws, s.far()), s.current_thread(), s.pc());
                            if self.watch_thread_matches(watched, thread) {
                                last = Some((pn, RHit::Watch { watched, pc, ord: w_ord }));
                            }
                            s.clear_watchpoints();
                            s.step_insns(1)?;
                            s.arm_watchpoints(&ws);
                            k += 1;
                        }
                        Stepped::AtTrap => return Err(format!(
                            "reverse-continue: window {pn} ended after {k} instruction(s), before P's {pk}")),
                    }
                }
                // M41 §3d: at K = pk itself, a breakpoint comes before a cursor that is ON the
                // store's watch (t0 M4). The pc is read before any step, as the loop above does.
                if pphase == Phase::Watch {
                    let pc = s.pc();
                    if bps.contains(&pc) {
                        bp_ord += 1;
                        last = Some((pn, RHit::Bp { pc, ord: bp_ord }));
                    }
                }
            }
        } // the scan session drops here: one VM per process
        // Defence (M40 final review): the scan only records hits strictly before P, so a resolved
        // coordinate at or after P means scan and resolver disagree. Fail loud rather than reseek
        // FORWARD under a command whose whole contract is "backward".
        let before_p = |n: usize, k: u64, ph: Phase| -> Result<(), String> {
            if (n, k, ph) < (pn, pk, pphase) { Ok(()) } else {
                Err(format!("reverse-continue: resolved ({n}, {k}, {ph:?}) is not before P ({pn}, {pk}, {pphase:?})"))
            }
        };
        // M54 §3i: the exec stop's report, shared by the exec hit itself and by a breakpoint on the new
        // image's entry, which is the same position (an arrival at (n, 0): R4 reports the exec alone).
        let exec_hit = |me: &mut Self, n: usize, out: &mut W| -> Result<Halt, String> {
            before_p(n, 0, Phase::Bp)?;
            let path = me.exec_at(n).expect("an exec landmark").to_vec();
            line(out, format_args!("exec {} at ({n}, 0)", String::from_utf8_lossy(&path)))?;
            me.reseek(n, 0)?; // phase Bp
            Ok(Halt::Exec { path })
        };
        match last {
            Some((n, RHit::Exec)) => exec_hit(self, n, out),
            Some((n, RHit::Bp { pc, ord })) => {
                let k = resolve_nth(self.trace, &mut self.cache, n, 0, HitKind::Break(&bps), ord, pc)?;
                before_p(n, k, Phase::Bp)?;
                if k == 0 && self.exec_at(n).is_some() { return exec_hit(self, n, out); }
                let a = self.annot(n, pc);
                line(out, format_args!("hit {pc:#x} at ({n}, {k}){a}"))?;
                self.reseek(n, k)?; // phase Bp
                Ok(Halt::Break)
            }
            Some((n, RHit::Watch { watched, pc, ord })) => {
                let k = resolve_nth(self.trace, &mut self.cache, n, 0, HitKind::Watch(&ws), ord, pc)?;
                before_p(n, k, Phase::Watch)?;
                let a = self.annot(n, pc);
                line(out, format_args!("hit watch {watched:#x} (write at {pc:#x}) at ({n}, {k}){a}"))?;
                self.reseek(n, k)?;
                self.phase = Phase::Watch;
                Ok(Halt::Watch { watched })
            }
            Some((n, RHit::WatchSys { watched, thread })) => {
                before_p(n, 0, Phase::Sys)?;
                line(out, format_args!("hit watch {watched:#x} (syscall write) at ({n}, 0)"))?;
                self.reseek(n, 0)?;
                self.phase = Phase::Sys;
                Ok(Halt::WatchSys { watched, thread })
            }
            None => {
                line(out, format_args!("no earlier hit"))?;
                // The cursor stays where it was (Review Focus 5).
                self.reseek(pn, pk)?;
                self.phase = pphase;
                Ok(Halt::NoEarlierHit)
            }
        }
    }
}

/// M58 B6: a hit of `summary`'s scans.
struct SumHit { pos: (usize, u64), thread: u32, x1: u64, x2: u64, text: Vec<String>, frames: J }

const SUMMARY_PANIC_CAP: usize = 16;
const SUMMARY_TAIL_LINES: usize = 20;

impl Exec<'_> {
    /// The executable's addresses whose readable name (hash dropped) `pred` accepts, ascending. A raw name must
    /// contain `needle` first, so no name is demangled that cannot match.
    fn exe_addrs(&self, needle: &str, pred: impl Fn(&str) -> bool) -> Vec<u64> {
        let mut v: Vec<u64> = match self.syms_at(1).exe() {
            Some(t) => t.iter().filter(|(_, raw)| raw.contains(needle))
                .filter(|(_, raw)| pred(&self.readable_at(1, raw))).map(|(a, _)| a).collect(),
            None => Vec::new(),
        };
        v.sort_unstable();
        v.dedup();
        v
    }

    /// From the recording's start, `continue` with only `addrs` armed until the terminal. `cap` bounds the hits kept:
    /// one more than `cap` ends the scan early (`truncated`), and the run goes on to the terminal with nothing armed.
    /// With `frames`, each hit carries its backtrace. The terminal's report comes back with the hits.
    fn scan_hits(&mut self, addrs: &[u64], cap: usize, frames: bool) -> Result<(Vec<SumHit>, bool, ReplayReport), String> {
        self.reseek(1, 0)?;
        self.breakpoints = addrs.to_vec();
        let mut hits = Vec::new();
        let mut truncated = false;
        let mut sink = std::io::sink();
        for _ in 0..(cap + 1) * 4 + 64 {
            match self.cmd_continue(&mut sink)? {
                Halt::Break => {
                    if hits.len() == cap {
                        truncated = true;
                        self.breakpoints.clear();
                        continue;
                    }
                    let thread = self.sess().current_thread();
                    let (x1, x2) = self.sess().thread_ctx(thread as usize).map_or((0, 0), |c| (c.regs.x[1], c.regs.x[2]));
                    let (text, fr) = if frames {
                        let mut buf = Vec::new();
                        let f = self.cmd_bt(None, &mut buf)?;
                        let text = String::from_utf8_lossy(&buf).lines().filter(|l| l.starts_with('#')).map(str::to_string).collect();
                        (text, f.into_iter().find(|(k, _)| *k == "frames").map_or(J::Arr(Vec::new()), |(_, v)| v))
                    } else { (Vec::new(), J::Null) };
                    hits.push(SumHit { pos: (self.n, self.k), thread, x1, x2, text, frames: fr });
                }
                Halt::Exec { .. } => {}
                Halt::Terminal(r) => return Ok((hits, truncated, r)),
                h => return Err(format!("summary: unexpected stop {h:?}")),
            }
        }
        Err("summary: the scan did not reach the end of the recording".into())
    }

    /// M58 B6: how the run ended and every Rust panic, from the recording's start. The user's breakpoints and
    /// watches are set aside for it and come back; the cursor is left at the recording's end.
    pub(crate) fn cmd_summary<W: Write>(&mut self, out: &mut W) -> Result<Vec<(&'static str, J)>, String> {
        let (bps, ws) = (std::mem::take(&mut self.breakpoints), std::mem::take(&mut self.watches));
        let r = self.summary_inner(out);
        self.breakpoints = bps;
        self.watches = ws;
        r
    }

    fn summary_inner<W: Write>(&mut self, out: &mut W) -> Result<Vec<(&'static str, J)>, String> {
        let mut programs: Vec<String> = Vec::new();
        for (i, (e, _)) in self.epochs.iter().enumerate() {
            programs.push(if i == 0 { self.start_cands.first().cloned().unwrap_or_default() } else { String::from_utf8_lossy(&e.path).into_owned() });
        }
        if programs.is_empty() { programs.push(self.start_cands.first().cloned().unwrap_or_default()); }

        let mut notes: Vec<String> = Vec::new();
        let mut panic_at = self.exe_addrs("rust_panic", |r| r == "rust_panic" || r.ends_with("::rust_panic"));
        if panic_at.len() > 6 {
            notes.push(format!("{} rust_panic symbols; only the first 6 by address are armed", panic_at.len()));
            panic_at.truncate(6);
        }
        let (mut panics, truncated, mut report) = self.scan_hits(&panic_at, SUMMARY_PANIC_CAP, true)?;
        let mut asserts: Vec<Option<(u64, u64)>> = vec![None; panics.len()];
        let mut assert_at = if panics.is_empty() { Vec::new() } else {
            self.exe_addrs("assert_failed", |r| r == "core::panicking::assert_failed")
        };
        if assert_at.len() > 6 {
            notes.push(format!("{} assert_failed symbols; only the first 6 by address are armed, so an assert_eq! on another type has no operands", assert_at.len()));
            assert_at.truncate(6);
        }
        if !assert_at.is_empty() {
            let (ahits, _, r) = self.scan_hits(&assert_at, 256, false)?;
            report = r;
            let mut used = vec![false; ahits.len()];
            for (pi, p) in panics.iter().enumerate() {
                if !p.text.iter().any(|l| l.contains(" in core::panicking::assert_failed")) { continue; }
                // The latest unused assert_failed entry on this thread before the panic.
                if let Some(ai) = (0..ahits.len()).rev().find(|&i| !used[i] && ahits[i].thread == p.thread && ahits[i].pos <= p.pos) {
                    used[ai] = true;
                    asserts[pi] = Some((ahits[ai].x1, ahits[ai].x2));
                }
            }
        }
        panics.truncate(SUMMARY_PANIC_CAP);

        // The cursor is at the terminal.
        let mut bt = Vec::new();
        let frames = self.cmd_bt(None, &mut bt)?.into_iter().find(|(k, _)| *k == "frames").map_or(J::Arr(Vec::new()), |(_, v)| v);
        // `bt`'s frame lines at the end, as a panic's are printed (a note line is left out, as it is there).
        let end_text: Vec<String> = String::from_utf8_lossy(&bt).lines().filter(|l| l.starts_with('#')).map(str::to_string).collect();
        let (end_pos, end_thread) = ((self.n, self.k), self.sess().current_thread());
        let (kind, ended, extra): (&str, String, Vec<(&'static str, J)>) = match &report.outcome {
            Outcome::Exit { code } => ("exit", format!("exited with {code}"), vec![("code", J::UInt(*code))]),
            Outcome::Crash { sig, far, .. } => {
                let name = retrace_arch::signal_name(*sig);
                ("crash", format!("killed by {name}, fault at {far:#x}"), vec![("signal", J::Str(name)), ("fault_address", hex(*far))])
            }
            Outcome::Signal { sig } => {
                let name = retrace_arch::signal_name(*sig);
                ("signal", format!("killed by {name}"), vec![("signal", J::Str(name))])
            }
            o => return Err(format!("summary: the recording ends in {o:?}")),
        };
        let mut ending = vec![("kind", J::Str(kind.into()))];
        ending.extend(extra);
        ending.extend(self.state_fields().into_iter().filter(|(k, _)| *k != "source_note"));
        ending.push(("frames", frames));

        let tail_of = |b: &[u8]| -> Vec<String> {
            let s = String::from_utf8_lossy(b).into_owned();
            let all: Vec<&str> = s.lines().collect();
            all[all.len().saturating_sub(SUMMARY_TAIL_LINES)..].iter().map(|l| l.to_string()).collect()
        };
        let (tail, out_tail) = (tail_of(&report.stderr), tail_of(&report.stdout));
        let threads: Vec<(u32, String, bool)> = self.sess().thread_summaries().into_iter().map(|t| (t.tid, format!("{:?}", t.state), t.is_current)).collect();

        let shown = if programs.iter().all(String::is_empty) { "(unknown)".to_string() } else { programs.join(" -> ") };
        line(out, format_args!("program: {shown}"))?;
        line(out, format_args!("ended: {ended}"))?;
        // Where a crash or a signal stopped the program: the thread, the position and its backtrace. An exit's
        // backtrace is the runtime's exit path, so it stays in JSON only.
        if kind != "exit" {
            line(out, format_args!("{kind} on thread {end_thread} at ({}, {})", end_pos.0, end_pos.1))?;
            for l in &end_text { line(out, format_args!("  {l}"))?; }
        }
        for (p, a) in panics.iter().zip(&asserts) {
            line(out, format_args!("panic on thread {} at ({}, {})", p.thread, p.pos.0, p.pos.1))?;
            for l in &p.text { line(out, format_args!("  {l}"))?; }
            if let Some((l, r)) = a { line(out, format_args!("  assert_eq! left operand at {l:#x}, right at {r:#x}"))?; }
        }
        if truncated { line(out, format_args!("(more than {SUMMARY_PANIC_CAP} panics; the rest are not listed)"))?; }
        for n in &notes { line(out, format_args!("note: {n}"))?; }
        // A libtest run prints a failing test's panic message on stdout (it captures the test's own stderr).
        if !out_tail.is_empty() {
            line(out, format_args!("stdout (last {SUMMARY_TAIL_LINES} lines):"))?;
            for l in &out_tail { line(out, format_args!("  {l}"))?; }
        }
        if !tail.is_empty() {
            line(out, format_args!("stderr (last {SUMMARY_TAIL_LINES} lines):"))?;
            for l in &tail { line(out, format_args!("  {l}"))?; }
        }
        line(out, format_args!("threads:"))?;
        for (t, s, cur) in &threads { line(out, format_args!("  {}thread {t}: {s}", if *cur { "* " } else { "  " }))?; }

        let jpanics = panics.iter().zip(&asserts).map(|(p, a)| {
            let mut o = vec![
                ("position", J::Arr(vec![J::UInt(p.pos.0 as u64), J::UInt(p.pos.1)])),
                ("thread", J::UInt(u64::from(p.thread))),
                ("frames", p.frames.clone()),
            ];
            if let Some((l, r)) = a { o.push(("assert", J::Obj(vec![("left", hex(*l)), ("right", hex(*r))]))); }
            J::Obj(o)
        }).collect();
        let mut v = vec![
            ("programs", J::Arr(programs.into_iter().map(J::Str).collect())),
            ("ending", J::Obj(ending)),
            ("panics", J::Arr(jpanics)),
            ("threads", J::Arr(threads.into_iter().map(|(t, s, cur)| J::Obj(vec![
                ("thread", J::UInt(u64::from(t))), ("state", J::Str(s)), ("current", J::Bool(cur))])).collect())),
            ("stderr_tail", J::Str(tail.join("\n"))),
            ("stdout_tail", J::Str(out_tail.join("\n"))),
            ("truncated", J::Bool(truncated)),
        ];
        if !notes.is_empty() { v.push(("notes", J::Arr(notes.into_iter().map(J::Str).collect()))); }
        Ok(v)
    }
}

/// Output mode of a script run (M58 B1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode { Text, Json }

/// Execute a `;`-separated debugger script against `trace`, writing the transcript to `out`. Parses
/// the whole script up front (a syntax error aborts before any output/VM work → the CLI exits 5),
/// then echoes and runs each command in order.
#[allow(dead_code)] // kept as the text-mode entry (M58 B1)
pub fn run_script(trace: &Path, script: &str, out: &mut impl Write) -> Result<(), String> {
    run_script_mode(trace, script, Mode::Text, out)
}

fn emit(out: &mut impl Write, j: J) -> Result<(), String> {
    let mut s = j.render();
    s.push('\n');
    out.write_all(s.as_bytes()).map_err(|e| format!("write error: {e}"))
}

/// `run_script` with a mode. In JSON mode each command is one line: its captured text lines in
/// `output`, then `state_fields`, then the command's extra fields. A failure prints its own
/// `ok:false` line before the error is returned.
pub fn run_script_mode(trace: &Path, script: &str, mode: Mode, out: &mut impl Write) -> Result<(), String> {
    let cmds = match parse_script(script) {
        Ok(c) => c,
        Err(e) => {
            if mode == Mode::Json {
                emit(out, J::Obj(vec![("command", J::Null), ("ok", J::Bool(false)), ("error", J::Str(e.clone()))]))?;
            }
            return Err(e);
        }
    };
    let segs: Vec<&str> = segments(script).collect(); // echo the exact source segment text
    let mut ex = match Exec::new(trace) {
        Ok(ex) => ex,
        Err(e) => {
            if mode == Mode::Json {
                emit(out, J::Obj(vec![("command", J::Null), ("ok", J::Bool(false)), ("error", J::Str(e.clone()))]))?;
            }
            return Err(e);
        }
    };
    for (seg, cmd) in segs.iter().zip(&cmds) {
        match mode {
            Mode::Text => {
                line(out, format_args!("> {seg}"))?;
                ex.exec(cmd, out)?;
            }
            Mode::Json => {
                let mut buf = Vec::new();
                match ex.exec_structured(cmd, &mut buf) {
                    Ok(extra) => {
                        let text = String::from_utf8_lossy(&buf);
                        let lines = text.lines().map(|l| J::Str(l.to_string())).collect();
                        let mut o = vec![("command", J::Str(seg.to_string())), ("ok", J::Bool(true)), ("output", J::Arr(lines))];
                        o.extend(ex.state_fields());
                        for (k, v) in extra { // an extra field of a state field's name replaces it
                            match o.iter_mut().find(|(ok, _)| *ok == k) { Some(slot) => slot.1 = v, None => o.push((k, v)) }
                        }
                        emit(out, J::Obj(o))?;
                    }
                    Err(e) => {
                        emit(out, J::Obj(vec![("command", J::Str(seg.to_string())), ("ok", J::Bool(false)), ("error", J::Str(e.clone()))]))?;
                        return Err(e);
                    }
                }
            }
        }
    }
    Ok(())
}

impl Exec<'_> {
    /// Today's `exec` into `out`, plus the extra fields a command contributes (none yet; B2-B6 add them).
    pub fn exec_structured(&mut self, cmd: &Cmd, out: &mut Vec<u8>) -> Result<Vec<(&'static str, J)>, String> {
        if let Cmd::Bt(tid) = cmd { return self.cmd_bt(*tid, out); }
        if let Cmd::LastWrite(op, len) = cmd { return self.cmd_last_write(op, *len, out); }
        if let Cmd::Summary = cmd { return self.cmd_summary(out); }
        self.exec(cmd, out)?;
        Ok(Vec::new())
    }

    /// The cursor as JSON: `position`, `thread`, `pc`, `function`. The one place these are built.
    pub fn state_fields(&self) -> Vec<(&'static str, J)> {
        let pc = self.sess().pc();
        // As a `bt` frame has them: the readable name alone, and the offset into it as its own field.
        let (function, offset) = match self.syms_at(self.n).resolve(pc) {
            Some((raw, off)) => (J::Str(self.readable_at(self.n, raw)), J::UInt(off)),
            None => (J::Null, J::Null),
        };
        let (lines, note) = self.source_lines(&[pc]);
        let (file, line) = match lines.into_iter().next().flatten() {
            Some((f, l)) => (J::Str(f), J::UInt(u64::from(l))),
            None => (J::Null, J::Null),
        };
        let mut v = vec![
            ("position", J::Arr(vec![J::UInt(self.n as u64), J::UInt(self.k)])),
            ("thread", J::UInt(u64::from(self.sess().current_thread()))),
            ("pc", hex(pc)),
            ("function", function),
            ("offset", offset),
            ("file", file),
            ("line", line),
        ];
        if let Some(n) = note { v.push(("source_note", J::Str(n))); }
        v
    }

    /// M58 B3: `(file, line)` per pc in the cursor's epoch, plus the note explaining an absence. The epoch's
    /// `SourceMap` is built on the first call that reaches it, from the image `resolve_image` trusts (the
    /// recorded `LC_UUID`; candidates are the recorded path, then the trace's directory). At most one note
    /// comes back per call, and only when a pc inside the executable got no line.
    pub(crate) fn source_lines(&self, pcs: &[u64]) -> (Vec<Option<(String, u32)>>, Option<String>) {
        let e = self.epoch_index();
        let mut maps = self.sources.borrow_mut();
        let Some(slot) = maps.get_mut(e) else { return (vec![None; pcs.len()], None) };
        if slot.is_none() {
            let cands: Vec<String> = if e == 0 { self.start_cands.clone() }
                else { self.epochs.get(e).map(|(ep, _)| String::from_utf8_lossy(&ep.path).into_owned()).into_iter().collect() };
            let trace_dir = match self.trace.parent() { Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(), _ => std::path::PathBuf::from(".") };
            let image = crate::image::resolve_for_source(&cands, &trace_dir, crate::image::recorded_uuid(self.sess()));
            let text_end = self.syms_at(self.n).exe().map_or(0, |t| t.text_end());
            *slot = Some(crate::source::SourceMap::new(image, text_end));
        }
        slot.as_ref().map_or((vec![None; pcs.len()], None), |m| m.lines(pcs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn parses_commands() {
        let cs = parse_script("break 0x1804af834; continue; reverse-stepi 2; x 0x1000 16; where").unwrap();
        assert_eq!(cs, vec![Cmd::Break(Operand::Addr(0x1804af834)), Cmd::Continue, Cmd::ReverseStepi(2),
                            Cmd::Examine(0x1000, 16), Cmd::Where]);
    }
    #[test] fn stepi_defaults_to_one() {
        assert_eq!(parse_script("stepi").unwrap(), vec![Cmd::Stepi(1)]);
        assert_eq!(parse_script("reverse-stepi").unwrap(), vec![Cmd::ReverseStepi(1)]);
    }
    #[test] fn rejects_unknown_verbs() {
        // An unknown VERB stays a parse error under M20 (S3). Only the *operand* rule changed, and
        // these two cases used to sit in one test where they read as the same thing; they are not.
        assert!(parse_script("frobnicate").is_err());
        assert!(parse_script("threads x").is_err(), "`threads` takes no arguments");
    }

    #[test]
    fn exec_text_names_the_exec() {
        assert_eq!(no_write_since_exec(0x1000, 7), "no write to 0x1000 since the exec at (7, 0)");
    }

    #[test]
    fn goto_parses_three_spellings() {
        for t in ["goto 3 5", "goto (3, 5)", "goto 3:5", "goto (3,5)"] {
            assert_eq!(parse_script(t).unwrap(), vec![Cmd::Goto(3, 5)], "{t}");
        }
        for t in ["goto", "goto 3", "goto 3 x", "goto a:b", "goto 1 2 3", "goto 0 1", "goto -1 2"] {
            assert!(parse_script(t).is_err(), "{t}");
        }
    }

    #[test]
    fn last_write_parses_len_and_alignment() {
        assert_eq!(parse_script("last-write 0x1000").unwrap(), vec![Cmd::LastWrite(Operand::Addr(0x1000), 8)]);
        assert_eq!(parse_script("last-write 0x1004 4").unwrap(), vec![Cmd::LastWrite(Operand::Addr(0x1004), 4)]);
        assert_eq!(parse_script("last-write rust_panic 1").unwrap(), vec![Cmd::LastWrite(Operand::Sym("rust_panic".into()), 1)]);
        assert!(parse_script("last-write 0x1000 3").is_err(), "bad len");
        assert!(parse_script("last-write 0x1004").is_err(), "8-byte misaligned");
        assert!(parse_script("last-write").is_err() && parse_script("last-write 0x1000 8 8").is_err());
    }

    // -------------------------------------------------------------------------------------------
    // M20: operand classification. The rule, in order (S2/S4):
    //   1. `0x`/`0X` prefix        => address, always
    //   2. parses completely as hex => address   (this is what keeps existing scripts working)
    //   3. otherwise                => symbol name
    // -------------------------------------------------------------------------------------------

    #[test] fn a_prefixed_or_bare_hex_operand_is_still_an_address() {
        // Rule 1 and rule 2. This is the backward-compatibility guarantee: every debug script that
        // worked before M20 parses to exactly the same Cmd afterwards.
        assert_eq!(parse_script("break 0x1000").unwrap(), vec![Cmd::Break(Operand::Addr(0x1000))]);
        assert_eq!(parse_script("break 1000").unwrap(), vec![Cmd::Break(Operand::Addr(0x1000))]);
        assert_eq!(parse_script("delete 0x1000").unwrap(), vec![Cmd::Delete(Operand::Addr(0x1000))]);
    }

    #[test] fn a_non_hex_operand_is_now_a_symbol_name() {
        // REPLACES the old `assert!(parse_script("break zzz").is_err())` (S3). That assertion was
        // the single one whose meaning M20 changes, so it is replaced by the new rule rather than
        // deleted — deleting it would have shipped the rule untested.
        assert_eq!(parse_script("break zzz").unwrap(), vec![Cmd::Break(Operand::Sym("zzz".into()))]);
        assert_eq!(parse_script("break _main").unwrap(), vec![Cmd::Break(Operand::Sym("_main".into()))]);
        assert_eq!(parse_script("delete _child").unwrap(),
                   vec![Cmd::Delete(Operand::Sym("_child".into()))]);
        // A mangled Rust name is just a name; no demangler is involved (M3).
        assert_eq!(parse_script("break _ZN4core3ptrE").unwrap(),
                   vec![Cmd::Break(Operand::Sym("_ZN4core3ptrE".into()))]);
    }

    #[test] fn an_all_hex_token_is_an_address_even_when_it_looks_like_a_name() {
        // The documented collision: `deadbeef` is valid hex AND a valid identifier, and rule 2 wins
        // so that existing scripts keep working. A symbol genuinely named `deadbeef` is unreachable;
        // Mach-O's leading underscore means the realistic form `_deadbeef` is not.
        assert_eq!(parse_script("break deadbeef").unwrap(),
                   vec![Cmd::Break(Operand::Addr(0xdeadbeef))]);
        assert_eq!(parse_script("break _deadbeef").unwrap(),
                   vec![Cmd::Break(Operand::Sym("_deadbeef".into()))]);
    }
    #[test] fn empty_segments_are_skipped() {
        assert_eq!(parse_script("regs;; where ;").unwrap(), vec![Cmd::Regs, Cmd::Where]);
    }
    #[test] fn parses_watch_and_unwatch() {
        assert_eq!(parse_script("watch 0x1000").unwrap(), vec![Cmd::Watch(0x1000, 8, None)]);
        assert_eq!(parse_script("watch 0x1004 4; unwatch 0x1004").unwrap(),
                   vec![Cmd::Watch(0x1004, 4, None), Cmd::Unwatch(0x1004)]);
    }
    #[test] fn rejects_bad_watch_len_and_alignment() {
        assert!(parse_script("watch 0x1000 3").unwrap_err().contains("must be 1, 2, 4, or 8"));
        assert!(parse_script("watch 0x1001 8").unwrap_err().contains("8-byte aligned"));
        assert!(parse_script("watch").is_err());
        assert!(parse_script("watch 0x1000 8 extra").is_err());
    }
    #[test] fn parses_watch_thread_scope() {
        // `thread <n>` composes with an omitted OR present `len`.
        assert_eq!(parse_script("watch 0x1000 thread 1").unwrap(), vec![Cmd::Watch(0x1000, 8, Some(1))]);
        assert_eq!(parse_script("watch 0x1000 4 thread 2").unwrap(), vec![Cmd::Watch(0x1000, 4, Some(2))]);
        assert!(parse_script("watch 0x1000 thread").unwrap_err().contains("requires a thread id"));
        assert!(parse_script("watch 0x1000 thread abc").unwrap_err().contains("bad thread id"));
        assert!(parse_script("watch 0x1000 thread 1 extra").is_err());
    }
    #[test] fn parses_threads_and_regs_of() {
        assert_eq!(parse_script("threads").unwrap(), vec![Cmd::Threads]);
        assert_eq!(parse_script("regs").unwrap(), vec![Cmd::Regs]);
        assert_eq!(parse_script("regs 3").unwrap(), vec![Cmd::RegsOf(3)]);
        assert_eq!(parse_script("bt; bt 2").unwrap(), vec![Cmd::Bt(None), Cmd::Bt(Some(2))]);
        assert!(parse_script("bt x").is_err() && parse_script("bt 1 2").is_err());
        assert!(parse_script("regs 1 2").is_err(), "`regs` takes at most one operand");
        assert!(parse_script("regs abc").is_err(), "a thread id must parse as u32");
        assert!(parse_script("threads x").is_err(), "`threads` takes no arguments");
    }

    #[test] fn watched_of_names_the_range_a_wider_store_covers() {
        let ws = [(0xa01722ac8u64, 8u64)];
        assert_eq!(watched_of(&ws, 0xa01722ac8), 0xa01722ac8, "exact");
        assert_eq!(watched_of(&ws, 0xa01722acc), 0xa01722ac8, "inside the range");
        // t0 M5/M7: rung 8's writers report FAR 0xa01722ac0, 8 below the watched qword. The Arm ARM
        // puts a watchpoint's FAR in the same naturally aligned block (at most the 64-byte DC ZVA
        // block) as a watched address the store wrote, so these FARs can come from a covering store:
        assert_eq!(watched_of(&ws, 0xa01722ac0), 0xa01722ac8, "16-byte stp, 8 below");
        assert_eq!(watched_of(&ws, 0xa01722ae0), 0xa01722ac8, "DC ZVA, 24 above in the same 64-byte block");
        assert_eq!(watched_of(&ws, 0xa01722aff), 0xa01722ac8, "DC ZVA, the block's last byte");
        // ...and these cannot: each lies in the 64-byte block BELOW the range's. A 32-byte `stp q`
        // based at 0xa01722ab0 does cover the range, but it reports a FAR inside the range's block,
        // never its own base. Both keep the honest fallback.
        assert_eq!(watched_of(&ws, 0xa01722ab0), 0xa01722ab0, "24 below, previous block: fallback");
        assert_eq!(watched_of(&ws, 0xa01722a90), 0xa01722a90, "56 below, previous block: fallback");
        assert_eq!(watched_of(&ws, 0xa01722b40), 0xa01722b40, "out of reach: the honest fallback, unchanged");
    }

    // -------------------------------------------------------------------------------------------
    // M40: VM-driving tests on the repo-owned WATCHSWEEP fixture. They live here, not in an e2e
    // file, because what they assert (the checkpoint cache's seek count and the trace decode count)
    // is internal to the debugger and invisible through the CLI.
    // -------------------------------------------------------------------------------------------

    fn record_watchsweep(tag: &str) -> std::path::PathBuf {
        let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::WATCHSWEEP).unwrap());
        let trace = std::env::temp_dir()
            .join(format!("retrace-m40-{tag}-{}.bin", std::process::id()));
        retrace_core::record(&loaded, &trace).expect("record watchsweep");
        trace
    }

    /// `&buf[40]`, from the recorded write(1, target, 8).
    fn watchsweep_target(trace: &Path) -> u64 {
        let mut s = retrace_core::ReplaySession::open(trace).unwrap();
        loop {
            if let Some((4, args)) = s.peek_syscall() {
                if args[0] == 1 { return args[1]; }
            }
            s.advance().unwrap();
        }
    }

    fn run_cmds(ex: &mut Exec, script: &str, sink: &mut Vec<u8>) {
        for cmd in parse_script(script).unwrap() { ex.exec(&cmd, sink).unwrap(); }
    }

    #[test] fn reverse_continue_makes_at_most_three_seeks_whatever_the_hits() {
        let trace = record_watchsweep("seeks");
        let t = watchsweep_target(&trace);
        let mut ex = Exec::new(&trace).unwrap();
        let mut sink = Vec::new();
        run_cmds(&mut ex, &format!("continue; watch 0x{t:x}"), &mut sink);
        let before = ex.cache.seeks();
        ex.exec(&Cmd::ReverseContinue, &mut sink).unwrap();
        let seeks = ex.cache.seeks() - before;
        let text = String::from_utf8_lossy(&sink).into_owned();
        assert!(text.contains(&format!("hit watch 0x{t:x} (write at ")), "{text}");
        // Spec §3b: the scan, the resolution and the park. Pre-M40 this paid two seeks per run of
        // the sweeping store between each resume point and each real hit.
        assert!(seeks <= 3, "one reverse-continue made {seeks} seeks:\n{text}");
    }

    #[test] fn a_debug_session_decodes_its_trace_once() {
        let trace = record_watchsweep("decodes");
        let t = watchsweep_target(&trace); // decodes on its own; the baseline is taken after it
        let d0 = retrace_trace::decode_count();
        let mut ex = Exec::new(&trace).unwrap();
        let mut sink = Vec::new();
        run_cmds(&mut ex, &format!("continue; watch 0x{t:x}; reverse-continue; stepi; reverse-stepi"), &mut sink);
        let decodes = retrace_trace::decode_count() - d0;
        // Spec §3e: every session the debugger opens, and M19's symbol table, share one decode.
        assert_eq!(decodes, 1, "a debug session decoded its trace {decodes} times:\n{}",
                   String::from_utf8_lossy(&sink));
    }

    #[test] fn hits_order_syscall_write_then_breakpoint_then_watch() {
        // M41 §3b: the hardware's own order at one coordinate, and the tuple order the cursor
        // compares with.
        assert!(Phase::Sys < Phase::Bp && Phase::Bp < Phase::Watch);
        assert!((4usize, 0u64, Phase::Watch) < (4, 1, Phase::Sys), "K dominates the phase");
        assert!((3usize, 9u64, Phase::Watch) < (4, 0, Phase::Sys), "N dominates K");
    }

    #[test] fn a_breakpoint_you_stepped_onto_is_reported_in_neither_direction() {
        // M41 R4 (gdb's rule): arriving by `stepi` puts the cursor AT the breakpoint phase, so
        // `continue` goes to the next pass and `reverse-continue` finds nothing earlier.
        let trace = record_watchsweep("arrival");
        let mut ex = Exec::new(&trace).unwrap();
        let mut sink = Vec::new();
        run_cmds(&mut ex, "stepi 8", &mut sink);
        let store = ex.sess().pc(); // the sweeping store's first pass
        run_cmds(&mut ex, &format!("break 0x{store:x}"), &mut sink);
        assert_eq!((ex.n, ex.k, ex.phase), (1, 8, Phase::Bp));
        run_cmds(&mut ex, "continue", &mut sink);
        let text = String::from_utf8_lossy(&sink).into_owned();
        // The loop body is five instructions, so the next pass is K = 13.
        assert!(text.contains("resolved (1, 13)"), "continue skips the breakpoint it stands on:\n{text}");

        drop(ex); // one VM per process: free the first session's VM before opening the second
        let mut ex = Exec::new(&trace).unwrap();
        let mut sink = Vec::new();
        run_cmds(&mut ex, &format!("stepi 8; break 0x{store:x}; reverse-continue"), &mut sink);
        let text = String::from_utf8_lossy(&sink).into_owned();
        assert!(text.contains("no earlier hit"), "…and so does reverse-continue:\n{text}");
        assert_eq!((ex.n, ex.k, ex.phase), (1, 8, Phase::Bp), "the cursor is where it was");

        // Both arrivals above start from `Exec::new`'s Bp, so they cannot tell an arrival from a
        // cursor left alone (review I1). From a WATCH park they can: the step must reset it.
        drop(ex);
        let buf0 = watchsweep_target(&trace) - 320; // buf[0]: written once, by the store at K = 8
        for (step, k) in [("stepi", 9u64), ("reverse-stepi", 7)] {
            let mut ex = Exec::new(&trace).unwrap();
            let mut sink = Vec::new();
            run_cmds(&mut ex, &format!("watch 0x{buf0:x}; continue"), &mut sink);
            assert_eq!((ex.n, ex.k, ex.phase), (1, 8, Phase::Watch), "parked on the store:\n{}",
                       String::from_utf8_lossy(&sink));
            run_cmds(&mut ex, step, &mut sink);
            assert_eq!((ex.n, ex.k, ex.phase), (1, k, Phase::Bp), "`{step}` off a watch park is an arrival");
        }
    }

    #[test] fn a_zero_count_step_is_not_an_arrival() {
        // Review I2: `stepi 0` and `reverse-stepi 0` leave (n, k) where it was, so they are not
        // arrivals (spec §3b: "arriving anywhere ELSE"). Resetting the phase would make the next
        // `continue` report the store the cursor is already on a second time.
        let trace = record_watchsweep("zerostep");
        let buf0 = watchsweep_target(&trace) - 320; // buf[0]: written once, by the store at K = 8
        for step in ["stepi 0", "reverse-stepi 0"] {
            let mut ex = Exec::new(&trace).unwrap();
            let mut sink = Vec::new();
            run_cmds(&mut ex, &format!("watch 0x{buf0:x}; continue; {step}; continue"), &mut sink);
            let text = String::from_utf8_lossy(&sink).into_owned();
            assert_eq!(text.matches("hit watch").count(), 1, "`{step}` re-armed the store:\n{text}");
            assert!(text.trim_end().ends_with("exited (code 0)"), "{text}");
        }
    }

    #[test] fn each_motion_returns_what_it_printed() {
        // M43 §3b: the Halt is the printed line's structured twin.
        let trace = record_watchsweep("halts");
        let t = watchsweep_target(&trace);
        let mut ex = Exec::new(&trace).unwrap();
        let mut sink = Vec::new();
        assert!(matches!(ex.cmd_reverse_stepi(1, &mut sink).unwrap(), Halt::AtStart));
        assert!(matches!(ex.cmd_stepi(3, &mut sink).unwrap(), Halt::Stepped));
        assert!(matches!(ex.cmd_reverse_continue(&mut sink).unwrap(), Halt::NoEarlierHit));
        ex.cmd_watch(t, 8, None, &mut sink).unwrap();
        assert!(matches!(ex.cmd_continue(&mut sink).unwrap(), Halt::Watch { watched } if watched == t));
        assert!(matches!(ex.cmd_continue(&mut sink).unwrap(), Halt::Watch { watched } if watched == t));
        assert!(matches!(ex.cmd_continue(&mut sink).unwrap(),
            Halt::Terminal(ref r) if r.outcome == retrace_core::Outcome::Exit { code: 0 }));
        assert!(matches!(ex.cmd_reverse_continue(&mut sink).unwrap(), Halt::Watch { .. }));
        let text = String::from_utf8_lossy(&sink).into_owned();
        assert_eq!(text.matches("hit watch").count(), 3, "{text}");
        assert!(text.contains("no earlier hit") && text.contains("at start of recording"), "{text}");
    }

    #[test] fn recover_leaves_a_usable_session_at_the_saved_cursor() {
        // M43 §3b, M41's owed item: a scan that diverges with breakpoints armed leaves the session
        // armed AND moved (t0 R2, debug.rs's scan `advance()?`). `recover` undoes both, because it
        // replaces the session with a fresh `reseek`, which is breakpoint-clean by construction.
        // What this pins is the observable half: the position is back, and the next motion works.
        let good = record_watchsweep("recover");
        let mut ev = retrace_trace::Reader::open(&good).unwrap();
        let i = ev.iter().rposition(|e| matches!(e, retrace_trace::Event::Syscall { num: 4, .. })).unwrap();
        if let retrace_trace::Event::Syscall { args, .. } = &mut ev[i] { args[2] += 1; }
        let bad = good.with_extension("tampered.bin");
        let mut w = retrace_trace::Writer::create(&bad).unwrap();
        for e in &ev { w.append(e).unwrap(); }
        drop(w);
        // In window 2 (exit's `svc`), which the scan never reaches: window 1's write diverges first.
        // Taken before `Exec::new`, whose session is this process's one VM from then on.
        let exit_svc = retrace_core::seek(&good, 2, 0).unwrap().pc() + 8;
        let mut ex = Exec::new(&bad).unwrap();
        let mut sink = Vec::new();
        let pc0 = ex.sess().pc();
        ex.cmd_break(exit_svc, &mut sink).unwrap();
        let at = ex.cursor();
        let err = ex.cmd_continue(&mut sink).unwrap_err();
        assert!(err.contains("diverged"), "{err}");
        assert_ne!(ex.sess().pc(), pc0, "the failed scan left its session moved");
        ex.recover(at).unwrap();
        // Cannot fail: a failed scan never assigns the cursor (t3 control C3b). The pc below carries it.
        assert_eq!(ex.cursor(), at);
        assert_eq!(ex.sess().pc(), pc0, "the session is back at the saved cursor");
        assert!(matches!(ex.cmd_stepi(1, &mut sink).unwrap(), Halt::Stepped));
        assert_eq!(ex.cursor(), (1, 1, Phase::Bp));
    }

    #[test] fn step_thread_steps_a_watched_store_and_reports_it_retired() {
        // M43 §3d: stepping onto a watched store reports it post-retire (WatchStepped), cursor at
        // (n, k + 1, Bp).
        let trace = record_watchsweep("stepthread");
        let buf0 = watchsweep_target(&trace) - 320;
        let mut ex = Exec::new(&trace).unwrap();
        let mut sink = Vec::new();
        run_cmds(&mut ex, &format!("watch 0x{buf0:x}; stepi 8"), &mut sink);
        let h = ex.step_thread(0, &mut sink).unwrap();
        assert!(matches!(h, Halt::WatchStepped { watched } if watched == buf0), "{h:?}");
        assert_eq!(ex.cursor(), (1, 9, Phase::Bp));
    }
}
