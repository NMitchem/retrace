//! `retrace debug <trace> --script '…'` — a scripted, deterministic reverse-debugger over a
//! recorded trace. Commands are `;`-separated; each echoes itself (`> <command>`) then its output.
//! Every printed byte derives from guest state, the script, or fixed strings — no host pointers, no
//! timing, no map-iteration order — so a transcript is bit-reproducible (the Task 6 e2e contract).

use std::io::Write;
use std::path::Path;
use retrace_core::{checkpointed_seek, Advance, CheckpointCache, Outcome, ReplayReport, ReplaySession};
use retrace_core::symbols::Symbols;

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
    Break(u64),
    Delete(u64),
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
}

/// The non-empty, trimmed command segments of a script, in order. `break;;continue;` → the two
/// commands (empty segments skipped). Shared by `parse_script` and the executor so the echoed
/// text is exactly the source segment.
fn segments(script: &str) -> impl Iterator<Item = &str> {
    script.split(';').map(str::trim).filter(|s| !s.is_empty())
}

/// A guest address: hex, `0x`-prefix optional (the brief writes `<hex-addr>`). `zzz` → Err.
fn parse_addr(tok: &str) -> Result<u64, String> {
    let hex = tok.strip_prefix("0x").or_else(|| tok.strip_prefix("0X")).unwrap_or(tok);
    u64::from_str_radix(hex, 16).map_err(|_| format!("bad hex address: {tok}"))
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
        "break"           => { one_operand(verb, &ops)?; Ok(Cmd::Break(parse_addr(ops[0])?)) }
        "delete"          => { one_operand(verb, &ops)?; Ok(Cmd::Delete(parse_addr(ops[0])?)) }
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
        "x"               => {
            if ops.len() != 2 { return Err(format!("`x` takes <addr> <len>; got {} operand(s)", ops.len())); }
            Ok(Cmd::Examine(parse_addr(ops[0])?, parse_len(ops[1])?))
        }
        other             => Err(format!("unknown command: {other}")),
    }
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

/// Replay window `n` from step `from_k` and return the first K (>= `from_k`) at which the guest's
/// live PC equals `pc`. Turns a mid-window hardware-breakpoint hit (which knows only the pc + its
/// landmark) into an exact (N, K) coordinate. Deterministic; runs on its own transient session, so
/// the caller must hold NO other live session (one VM per process).
fn resolve_hit_k(trace: &Path, cache: &mut CheckpointCache, n: usize, pc: u64, from_k: u64) -> Result<u64, String> {
    let mut s = checkpointed_seek(trace, cache, n, from_k)?;
    let mut k = from_k;
    loop {
        if s.pc() == pc { return Ok(k); }
        s.step_insns(1).map_err(|e| format!("resolve K in window {n}: {e}"))?;
        k += 1;
    }
}

/// The armed watch range containing `far` (exact byte), else the range overlapping `far`'s aligned
/// doubleword (FAR may report the comparator base — spike F4b), else `far` itself (honest fallback,
/// never a wrong range). Deterministic: `ws` is sorted, first match wins.
fn watched_of(ws: &[(u64, u64)], far: u64) -> u64 {
    ws.iter().find(|&&(a, l)| far >= a && far < a + l)
        .or_else(|| ws.iter().find(|&&(a, l)| { let d = far & !7; d < a + l && a < d + 8 }))
        .map(|&(a, _)| a)
        .unwrap_or(far)
}

/// The scripted-debugger executor. Holds the position coordinate P = (`n`, `k`) and at most ONE live
/// `ReplaySession` parked exactly at P (one VM per process → every command that MOVES drops the old
/// session before seeking a fresh one). `breakpoints` is kept sorted + deduped (≤ 6, enforced by
/// `break`) so the hardware-slot assignment and any iteration are deterministic.
struct Exec<'a> {
    trace: &'a Path,
    session: Option<ReplaySession>,
    n: usize,
    k: u64,
    breakpoints: Vec<u64>,
    /// Armed watch ranges (addr, len, optional thread scope), sorted by addr + deduped (≤ 4: one
    /// per DBGWVR slot). The thread scope (M15 Task 8) is a pure debugger-side filter — the
    /// hardware slot itself stays global — checked at each hit site via `watch_thread_matches`.
    watches: Vec<(u64, u64, Option<u32>)>,
    /// The (n, k) at which the session is currently parked pre-retire on a hardware watch hit, be
    /// it one reported to the user or one discarded by a thread scope — either way the store has
    /// not retired, so `cmd_continue`'s progress rule must pre-step off it before resuming.
    last_watch_hit: Option<(usize, u64)>,
    cache: CheckpointCache,
    /// M19: address → name, built ONCE per session from the recording's opening snapshot.
    ///
    /// Preserves this module's bit-reproducibility contract (see the file header): the names derive
    /// from *guest state* — the `__LINKEDIT` bytes the snapshot already carries — not from a host
    /// path, the host's own symbols, or anything timing- or iteration-order-dependent. An empty
    /// table (static guest, stripped binary) simply formats every address as bare hex.
    syms: Symbols,
}

impl<'a> Exec<'a> {
    /// Open at the recording start: P = (1, 0), the first landmark's window, step 0.
    fn new(trace: &'a Path) -> Result<Self, String> {
        let mut cache = CheckpointCache::new(CHECKPOINT_BYTE_BUDGET, CHECKPOINT_COST_GATE_STEPS);
        let session = checkpointed_seek(trace, &mut cache, 1, 0)?;
        // M19: build the symbol table from the OPENING snapshot, once. It is the image as loaded,
        // which is what every pc in the session refers to. Any failure here — unreadable trace, no
        // Snapshot, stripped binary — yields an empty table and bare-hex output rather than an
        // error: symbolication is presentation, and must never be able to fail a debug session.
        let syms = retrace_trace::Reader::open(trace).ok()
            .and_then(|events| events.iter().find_map(|e| match e {
                retrace_trace::Event::Snapshot { mem, .. } => Some(Symbols::from_snapshot(mem)),
                _ => None,
            }))
            .unwrap_or_default();
        Ok(Exec {
            trace, session: Some(session), n: 1, k: 0,
            breakpoints: Vec::new(), watches: Vec::new(), last_watch_hit: None, cache, syms,
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
    fn annot(&self, addr: u64) -> String {
        match self.syms.resolve(addr) {
            Some((n, 0)) => format!("  in {n}"),
            Some((n, off)) => format!("  in {n}+{off:#x}"),
            None => String::new(),
        }
    }

    fn sess(&self) -> &ReplaySession { self.session.as_ref().expect("live session") }
    fn sess_mut(&mut self) -> &mut ReplaySession { self.session.as_mut().expect("live session") }

    /// Drop the current session (freeing its VM) and seek a fresh one parked at (n, k), which is
    /// therefore breakpoint-clean. Updates the position coordinate.
    fn reseek(&mut self, n: usize, k: u64) -> Result<(), String> {
        self.session = None; // free the old VM BEFORE opening a new one
        self.session = Some(checkpointed_seek(self.trace, &mut self.cache, n, k)?);
        self.n = n;
        self.k = k;
        Ok(())
    }

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
            Cmd::Break(a)         => self.cmd_break(*a, out),
            Cmd::Delete(a)        => self.cmd_delete(*a, out),
            Cmd::Continue         => self.cmd_continue(out),
            Cmd::ReverseContinue  => self.cmd_reverse_continue(out),
            Cmd::Stepi(count)     => self.cmd_stepi(*count, out),
            Cmd::ReverseStepi(c)  => self.cmd_reverse_stepi(*c, out),
            Cmd::Regs             => self.cmd_regs(out),
            Cmd::Examine(a, len)  => self.cmd_examine(*a, *len, out),
            Cmd::Where            => self.cmd_where(out),
            Cmd::Watch(a, l, t)   => self.cmd_watch(*a, *l, *t, out),
            Cmd::Unwatch(a)       => self.cmd_unwatch(*a, out),
            Cmd::Threads          => self.cmd_threads(out),
            Cmd::RegsOf(tid)      => self.cmd_regs_of(*tid, out),
        }
    }

    fn cmd_break<W: Write>(&mut self, addr: u64, out: &mut W) -> Result<(), String> {
        if let Err(i) = self.breakpoints.binary_search(&addr) {
            if self.breakpoints.len() >= 6 {
                return Err("cannot arm more than 6 breakpoints (hardware limit: DBGBVR0-5)".into());
            }
            self.breakpoints.insert(i, addr); // sorted + deduped (≤ 6: one per DBGBVR slot)
        }
        line(out, format_args!("breakpoint at {addr:#x}"))
    }

    fn cmd_delete<W: Write>(&mut self, addr: u64, out: &mut W) -> Result<(), String> {
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
    fn cmd_watch<W: Write>(&mut self, addr: u64, len: u64, thread: Option<u32>, out: &mut W) -> Result<(), String> {
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

    fn cmd_unwatch<W: Write>(&mut self, addr: u64, out: &mut W) -> Result<(), String> {
        if let Ok(i) = self.watches.binary_search_by_key(&addr, |&(a, _, _)| a) {
            self.watches.remove(i);
        }
        line(out, format_args!("unwatched {addr:#x}"))
    }

    /// Whether a hardware/syscall watch hit at `addr` by `thread` should be reported to the user:
    /// true when that watch has no scope, or the scope equals `thread`. The `_ => true` arm is a
    /// deliberate fail-OPEN, not merely a defensive default: `watched_of`'s own `far`-fallback
    /// (`.unwrap_or(far)`, used when `far` lands outside every armed range's exact bytes AND its
    /// aligned doubleword) CAN hand this function an address genuinely absent from `self.watches`
    /// — a hit whose owning watch is unknown. Suppressing an unattributable hit would be a worse
    /// failure than showing an unscoped one: this function chooses to report it rather than risk
    /// silently hiding a real write.
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

    fn cmd_where<W: Write>(&mut self, out: &mut W) -> Result<(), String> {
        let pc = self.sess().pc();
        let a = self.annot(pc);
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
    fn cmd_stepi<W: Write>(&mut self, count: u64, out: &mut W) -> Result<(), String> {
        let res = self.sess_mut().step_insns(count);
        match res {
            Ok(()) => { self.k += count; Ok(()) }
            Err(msg) => {
                let head = msg.split("; cannot step").next().unwrap_or(&msg);
                line(out, format_args!("error: {head}"))?;
                let (n0, k0) = (self.n, self.k);
                self.reseek(n0, k0)
            }
        }
    }

    /// Step backward `count` instructions by coordinate arithmetic, crossing a landmark boundary via
    /// one probe of the previous window's length, then one final seek to the resolved coordinate. At
    /// (1, 0) it cannot go earlier: print `at start of recording` and stop.
    fn cmd_reverse_stepi<W: Write>(&mut self, count: u64, out: &mut W) -> Result<(), String> {
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
        self.reseek(n, k)
    }

    /// Report a terminal `Advance::Exited` and park the session on it. Both of `cmd_continue`'s
    /// `Exited` arms (main scan and pre-step boundary cross) route here so the two stay identical.
    ///
    /// An `exit` parks at the final landmark's window start `(E, 0)` — the exit syscall is consumed,
    /// so there is nothing further to reach. A CRASH instead parks AT the fault: `(C, K_f)`, where
    /// `K_f` is the crash window's full length (the count of instructions that DID retire before the
    /// fault). Stepping that far leaves the guest immediately before the never-retiring faulting
    /// instruction, so `pc()` IS the crash pc and a following `reverse-continue` orders after every
    /// write in the recording — which is what makes "run backward from the crash to the corrupting
    /// store" work.
    fn park_at_terminal<W: Write>(&mut self, report: ReplayReport, out: &mut W) -> Result<(), String> {
        match report.outcome {
            Outcome::Exit { code } => {
                line(out, format_args!("exited (code {code})"))?;
                let e = self.sess().landmark();
                self.reseek(e, 0)
            }
            Outcome::Crash { pc, esr, far } => {
                let a = self.annot(pc);
                line(out, format_args!("guest crashed: pc={pc:#x} far={far:#x} esr={esr:#x}{a}"))?;
                let c = self.sess().landmark();
                let kf = self.probe_window_len(c)?; // drops the live session (one VM per process)
                self.reseek(c, kf)
            }
            // M11: terminal like a crash, and it inherits the same seek machinery — but it is NOT
            // presented as one. Calling a SIGABRT a fault is the same lie that got Event::Crash
            // reuse rejected in the spec, and the debug output would carry it forever.
            Outcome::Signal { sig } => {
                line(out, format_args!("guest terminated by signal {sig}"))?;
                let c = self.sess().landmark();
                let kf = self.probe_window_len(c)?; // drops the live session (one VM per process)
                self.reseek(c, kf)
            }
        }
    }

    /// Run forward until a breakpoint is reached or the guest exits. Hardware breakpoints (one
    /// DBGBVR slot each; ≤ 6, enforced by `break`) catch mid-window hits (`Advance::Break`),
    /// resolved to an exact (N, K); the landmark-granular check catches a breakpoint that lands
    /// exactly on a landmark boundary. With no breakpoints set, runs to exit.
    fn cmd_continue<W: Write>(&mut self, out: &mut W) -> Result<(), String> {
        // Pre-step rule: if we are parked exactly ON a breakpoint, advance one instruction BEFORE
        // arming BVRs, so the scan below does not immediately re-report the current position as a hit
        // (which would otherwise re-fire at 0 progress and error, exit 5). Fixes back-to-back
        // `continue` on a once-executed breakpoint and the boundary-bp K=0 edge. Re-seek to (N, K+1);
        // if that walks off the window end, cross into the next window at (N+1, 0) via one advance();
        // if THAT advance exits, print the exit and the command is over. The boundary check at the
        // new position is left to the scan loop below (do NOT report a hit at the pre-step position).
        // A boundary-cross advances with the WATCHES armed, so a syscall write to a watched range
        // in the crossed event is reported, not skipped.
        if self.last_watch_hit == Some((self.n, self.k)) || self.breakpoints.contains(&self.sess().pc()) {
            let (n, k) = (self.n, self.k);
            match self.reseek(n, k + 1) {
                Ok(()) => {}
                Err(e) if e.contains("ends after") => {
                    self.reseek(n, k)?; // window end: re-establish (N, K), then advance to (N+1, 0)
                    // Arm the watches for this one-event crossing: the consumed boundary event may
                    // itself be a syscall write to a watched range — without arming, that writer
                    // would be silently skipped (M5 final-review M-1). Breakpoints stay unarmed
                    // (the pre-step must not re-report the parked position); no instruction
                    // retires during the crossing (the guest is parked ON the trap), so only
                    // Event, WatchSyscall, or Exited can come back — never Watch or Break.
                    let ws: Vec<(u64, u64)> = self.watches.iter().map(|&(a, l, _)| (a, l)).collect();
                    self.sess_mut().arm_watchpoints(&ws);
                    match self.sess_mut().advance().map_err(|d|
                        format!("continue diverged at landmark {} pc {:#x}: {}", d.landmark, d.pc, d.detail))?
                    {
                        Advance::Exited(report) => return self.park_at_terminal(report, out),
                        // Task 8: a scoped-out writer falls through to the `_` arm below — it is
                        // treated exactly like a plain Event (boundary crossed, keep scanning).
                        Advance::WatchSyscall { watched, thread } if self.watch_thread_matches(watched, thread) => {
                            let n = self.sess().landmark();
                            line(out, format_args!("hit watch {watched:#x} (syscall write) at ({n}, 0)"))?;
                            // Only watches were armed here — clear them; the session is kept.
                            self.sess_mut().clear_watchpoints();
                            self.n = n;
                            self.k = 0;
                            return Ok(());
                        }
                        _ => {
                            // Plain Event, or a WatchSyscall scoped out by thread: disarm before
                            // the main scan re-arms (a second arm_watchpoints without a clear
                            // would duplicate watch_ranges).
                            self.sess_mut().clear_watchpoints();
                            self.n = self.sess().landmark();
                            self.k = 0;
                        }
                    }
                }
                Err(e) => return Err(e),
            }
        }
        let (start_n, start_k) = (self.n, self.k);
        let bps = self.breakpoints.clone();
        self.sess_mut().arm_breakpoints(&bps);
        let ws: Vec<(u64, u64)> = self.watches.iter().map(|&(a, l, _)| (a, l)).collect();
        self.sess_mut().arm_watchpoints(&ws);
        loop {
            let adv = self.sess_mut().advance()
                .map_err(|d| format!("continue diverged at landmark {} pc {:#x}: {}", d.landmark, d.pc, d.detail))?;
            match adv {
                Advance::Break => {
                    let n = self.sess().landmark();
                    let p_hit = self.sess().pc();
                    let a = self.annot(p_hit);
                    line(out, format_args!("hit {p_hit:#x} at ({n}, +?){a}"))?;
                    // K_cur = the pre-continue step only if the hit is in that same window; else 0
                    // (we entered window n via a landmark). Resolve the FIRST occurrence past it.
                    let kctx = if n == start_n { start_k } else { 0 };
                    self.session = None; // free the VM before the resolution seek
                    let k = resolve_hit_k(self.trace, &mut self.cache, n, p_hit, kctx + 1)?;
                    line(out, format_args!("resolved ({n}, {k})"))?;
                    return self.reseek(n, k);
                }
                Advance::Event => {
                    let pc = self.sess().pc();
                    if bps.contains(&pc) {
                        let n = self.sess().landmark();
                        let a = self.annot(pc);
                        line(out, format_args!("hit {pc:#x} at ({n}, 0){a}"))?;
                        self.sess_mut().clear_breakpoints(); // keep this session, breakpoint-clean
                        self.sess_mut().clear_watchpoints(); // invariant: never leave WPs armed on a kept session (stepi runs on it)
                        self.n = n;
                        self.k = 0;
                        return Ok(());
                    }
                    // no boundary match; keep scanning (hardware breakpoints stay armed)
                }
                Advance::Exited(report) => return self.park_at_terminal(report, out),
                // Both watch arms below name their `thread` field (not `..`): M15 Task 5 plumbs
                // the writing thread through `Advance`, Task 7 is what teaches these lines to
                // print it, and Task 8 is what filters on it. Naming the field keeps every site
                // that drops it greppable; `..` is exactly how Task 4's oracle check went missing
                // from two arms.
                Advance::Watch { thread } => {
                    let n = self.sess().landmark();
                    let p_hit = self.sess().pc();
                    let watched = watched_of(&ws, self.sess().far());
                    let matched = self.watch_thread_matches(watched, thread);
                    if matched {
                        let a = self.annot(p_hit);
                        line(out, format_args!("hit watch {watched:#x} (write at {p_hit:#x}) at ({n}, +?){a}"))?;
                    }
                    // Resolve from kctx, NOT kctx+1: unlike a breakpoint (whose parked-on case the
                    // pre-step already moved off), a watched store CAN legitimately fire at the
                    // exact parked coordinate (the user stepi'd up to it), and the store pc repeats
                    // in loops — searching from kctx+1 would misresolve to the NEXT iteration. This
                    // resolution runs whether or not the hit is scoped out: the vCPU is physically
                    // parked pre-retire at the store either way.
                    let kctx = if n == start_n { start_k } else { 0 };
                    self.session = None; // free the VM before the resolution seek
                    let k = resolve_hit_k(self.trace, &mut self.cache, n, p_hit, kctx)?;
                    if matched {
                        line(out, format_args!("resolved ({n}, {k})"))?;
                    }
                    self.last_watch_hit = Some((n, k));
                    self.reseek(n, k)?;
                    if matched {
                        return Ok(());
                    }
                    // Task 8: scoped to a different thread — not a hit for this filter. Re-enter
                    // this function rather than duplicating its own pre-step rule: `last_watch_hit
                    // == (n, k)` now holds, so the top-of-function pre-step steps past the
                    // un-retired store and the scan resumes from there. Recursion depth is bounded
                    // by the number of scoped-out hits within this one `continue` (not by
                    // instruction count), and the cost of each is NOT merely slowness: a discarded
                    // hit pays a full `resolve_hit_k` seek AND one stack frame, so a guest with a
                    // hot write loop on the scoped-out thread OVERFLOWS THE STACK and crashes the
                    // debugger rather than degrading gracefully. Unexercised today (no guest writes
                    // a watched address in a loop from a thread that isn't the watched one), and
                    // the fix is mechanical when one does: everything a `loop` would need is
                    // already on `self` — `self.n`, `self.k` and `last_watch_hit`, all set by the
                    // `reseek` above.
                    return self.cmd_continue(out);
                }
                Advance::WatchSyscall { watched, thread } => {
                    if self.watch_thread_matches(watched, thread) {
                        let n = self.sess().landmark();
                        line(out, format_args!("hit watch {watched:#x} (syscall write) at ({n}, 0)"))?;
                        self.sess_mut().clear_breakpoints();  // keep this session, hit-clean
                        self.sess_mut().clear_watchpoints();
                        self.n = n;
                        self.k = 0;
                        return Ok(());
                    }
                    // Task 8: scoped out. The writing event is already consumed (no pre-retire
                    // issue for a syscall write), so just keep scanning — breakpoints/watches
                    // stay armed and the loop's next `advance()` moves past it on its own.
                }
            }
        }
    }

    /// Run backward to the latest hit — breakpoint, hardware watch, or syscall watch — strictly
    /// before the current position P. Scans forward from the start (the only direction replay
    /// runs), recording each hit's coordinate and stepping the cursor past it, until a hit at/after
    /// P or exit. A hardware watch hit resolves K from the hit pc (searching from the scan cursor,
    /// NOT cursor+1 — a store can fire at the cursor's own coordinate); a syscall hit's coordinate
    /// is the post-event boundary (n, 0) and its cursor resumes AT (n, 0): the writing event is
    /// already consumed by the (unarmed) seek, so it cannot re-fire, but a first-instruction store
    /// in window n can still be caught.
    fn cmd_reverse_continue<W: Write>(&mut self, out: &mut W) -> Result<(), String> {
        // Watch/WatchSys carry the writing `thread` too (M15 Task 8): the scan below must still
        // walk THROUGH a scoped-out hit (it is a real, earlier event that may hide an earlier
        // matching one behind it), so the thread rides along on every candidate and the FILTER is
        // applied only once, where `last` gets decided — never at the point of discovery.
        enum RHit { Bp(u64), Watch { watched: u64, pc: u64, thread: u32 }, WatchSys { watched: u64, thread: u32 } }
        let (pn, pk) = (self.n, self.k);
        let bps = self.breakpoints.clone();
        let ws: Vec<(u64, u64)> = self.watches.iter().map(|&(a, l, _)| (a, l)).collect();
        self.session = None; // the scan uses its own transient sessions
        let mut last: Option<(usize, u64, RHit)> = None; // (n, k, kind) of the latest hit < P
        let (mut cur_n, mut cur_k) = (1usize, 0u64);     // scan cursor
        loop {
            let mut s = checkpointed_seek(self.trace, &mut self.cache, cur_n, cur_k)?;
            s.arm_breakpoints(&bps);
            s.arm_watchpoints(&ws);
            let hit = loop {
                match s.advance().map_err(|d| format!("reverse-continue diverged: {}", d.detail))? {
                    Advance::Break => break Some((s.landmark(), RHit::Bp(s.pc()))),
                    Advance::Watch { thread } => {
                        let watched = watched_of(&ws, s.far());
                        break Some((s.landmark(), RHit::Watch { watched, pc: s.pc(), thread }));
                    }
                    Advance::WatchSyscall { watched, thread } =>
                        break Some((s.landmark(), RHit::WatchSys { watched, thread })),
                    Advance::Event => continue,
                    // Exited covers BOTH terminals (exit and crash): either way the scan is over.
                    Advance::Exited(_) => break None,
                }
            };
            drop(s); // free the VM before resolving K
            let (n, rh) = match hit { Some(h) => h, None => break };
            let (k, resume) = match &rh {
                RHit::Bp(pc) | RHit::Watch { pc, .. } => {
                    let from_k = if n == cur_n { cur_k } else { 0 };
                    let k = resolve_hit_k(self.trace, &mut self.cache, n, *pc, from_k)?;
                    (k, (n, k + 1)) // resume strictly past a resolved instruction hit
                }
                RHit::WatchSys { .. } => (0u64, (n, 0u64)),
            };
            if (n, k) < (pn, pk) {
                // Task 8: a scoped-out watch hit is still a real event — the cursor advances past
                // it exactly as if it counted, so scanning continues into whatever comes after —
                // but it does NOT become a candidate for `last`. Breakpoints are never scoped.
                let matches = match &rh {
                    RHit::Bp(_) => true,
                    RHit::Watch { watched, thread, .. } => self.watch_thread_matches(*watched, *thread),
                    RHit::WatchSys { watched, thread } => self.watch_thread_matches(*watched, *thread),
                };
                if matches {
                    last = Some((n, k, rh));
                }
                (cur_n, cur_k) = resume;
            } else {
                break; // reached P; earlier hits are already recorded
            }
        }
        match last {
            Some((n, k, RHit::Bp(pc))) => {
                let a = self.annot(pc);
                line(out, format_args!("hit {pc:#x} at ({n}, {k}){a}"))?;
                self.reseek(n, k)
            }
            Some((n, k, RHit::Watch { watched, pc, .. })) => {
                let a = self.annot(pc);
                line(out, format_args!("hit watch {watched:#x} (write at {pc:#x}) at ({n}, {k}){a}"))?;
                self.last_watch_hit = Some((n, k));
                self.reseek(n, k)
            }
            Some((n, _, RHit::WatchSys { watched, .. })) => {
                line(out, format_args!("hit watch {watched:#x} (syscall write) at ({n}, 0)"))?;
                self.reseek(n, 0)
            }
            None => { line(out, format_args!("no earlier hit"))?; self.reseek(pn, pk) }
        }
    }
}

/// Execute a `;`-separated debugger script against `trace`, writing the transcript to `out`. Parses
/// the whole script up front (a syntax error aborts before any output/VM work → the CLI exits 5),
/// then echoes and runs each command in order.
pub fn run_script(trace: &Path, script: &str, out: &mut impl Write) -> Result<(), String> {
    let cmds = parse_script(script)?;
    let segs: Vec<&str> = segments(script).collect(); // echo the exact source segment text
    let mut ex = Exec::new(trace)?;
    for (seg, cmd) in segs.iter().zip(&cmds) {
        line(out, format_args!("> {seg}"))?;
        ex.exec(cmd, out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn parses_commands() {
        let cs = parse_script("break 0x1804af834; continue; reverse-stepi 2; x 0x1000 16; where").unwrap();
        assert_eq!(cs, vec![Cmd::Break(0x1804af834), Cmd::Continue, Cmd::ReverseStepi(2),
                            Cmd::Examine(0x1000, 16), Cmd::Where]);
    }
    #[test] fn stepi_defaults_to_one() {
        assert_eq!(parse_script("stepi").unwrap(), vec![Cmd::Stepi(1)]);
        assert_eq!(parse_script("reverse-stepi").unwrap(), vec![Cmd::ReverseStepi(1)]);
    }
    #[test] fn rejects_unknown_and_bad_hex() {
        assert!(parse_script("frobnicate").is_err());
        assert!(parse_script("break zzz").is_err());
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
        assert!(parse_script("regs 1 2").is_err(), "`regs` takes at most one operand");
        assert!(parse_script("regs abc").is_err(), "a thread id must parse as u32");
        assert!(parse_script("threads x").is_err(), "`threads` takes no arguments");
    }
}
