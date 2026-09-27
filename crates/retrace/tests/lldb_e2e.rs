//! M43: real lldb against `retrace gdbserver` (spec
//! `docs/superpowers/specs/2026-09-25-retrace-m43-lldb-design.md` §4). The original design's exit
//! criterion, "reverse-step through a real crash in LLDB", on the repo-owned `crashy` fixture, and
//! on CPython when Homebrew's is installed.
//!
//! lldb is not a repo artifact, so each test skips with a loud `SKIPPED` line (`util::announce`) when
//! `/usr/bin/lldb --version` does not run. A silent skip reads as a green it did not earn. The lldb
//! invocation is `lldb -x -b -s <file> </dev/null`, never `-o`, which silently stops after a crash
//! or boundary stop and exits 0 (t0 L10). The script's last command prints `END`, and every test
//! asserts it.
mod util;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

const GARBAGE_VA: u64 = 0x4000_DEAD_0000; // mirrors c/crashy.c
/// Seconds one lldb session may run before it is killed, so that a loop fails rather than stalls
/// (Ruling T5-d). The slowest session measured about 2 s (CPython's).
const BOUND: u64 = 120;
/// Spec §4's lldb, the one the server's behaviour was measured against (t0 L1–L10). Never the one
/// on `PATH`: a Homebrew LLVM `lldb` earlier there would put this gate on a different lldb.
const LLDB: &str = "/usr/bin/lldb";

/// Whether `LLDB --version` runs. Its first line is announced once, so a gate log shows which lldb ran.
fn lldb_runs() -> bool {
    static V: OnceLock<Option<String>> = OnceLock::new();
    V.get_or_init(|| {
        let v = Command::new(LLDB).arg("--version").output().ok().filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or("").trim().to_string());
        match &v {
            Some(line) => util::announce(&format!("lldb_e2e runs {LLDB}: {line}")),
            None => util::announce(&format!("lldb_e2e: `{LLDB} --version` did not run")),
        }
        v
    }).is_some()
}

fn retrace_py() -> String { concat!(env!("CARGO_MANIFEST_DIR"), "/lldb/retrace.py").to_string() }

/// Start `retrace gdbserver <trace>`, run lldb with `cmds` (a `gdb-remote` line is prepended and an
/// `END` sentinel appended), return (lldb's exit code or None if killed at the bound, stdout, stderr).
/// The server and lldb are each held by a `KillOnDrop`, so every path, a panic included, kills and
/// reaps both.
fn session(trace: &Path, cmds: &[String]) -> (Option<i32>, String, String) {
    let (srv, port, srv_err) = util::rsp::spawn_server(trace, &[]);
    let srv = util::rsp::KillOnDrop(srv);
    let base = std::env::temp_dir().join(format!("retrace-lldb-{}-{port}", std::process::id()));
    let (cmd_p, out_p, err_p) = (base.with_extension("cmds"), base.with_extension("out"), base.with_extension("err"));
    let mut script = vec![format!("gdb-remote 127.0.0.1:{port}"), format!("command script import {}", retrace_py())];
    script.extend(cmds.iter().cloned());
    script.push(r#"script print("END")"#.into());
    std::fs::write(&cmd_p, script.join("\n") + "\n").unwrap();
    let mut lldb = util::rsp::KillOnDrop(Command::new(LLDB).args(["-x", "-b", "-s", cmd_p.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&out_p).unwrap()).stderr(std::fs::File::create(&err_p).unwrap())
        .spawn().expect("spawn lldb"));
    let mut code = None;
    for _ in 0..BOUND * 20 {
        if let Some(st) = lldb.0.try_wait().unwrap() { code = Some(st.code().unwrap_or(-1)); break; }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    drop(lldb); // killed here if the bound fired
    drop(srv);
    let read = |p: &PathBuf| { let s = std::fs::read_to_string(p).unwrap_or_default(); let _ = std::fs::remove_file(p); s };
    let _ = std::fs::remove_file(&cmd_p);
    let srv_log = read(&srv_err);
    (code, read(&out_p), read(&err_p) + "\n--- gdbserver stderr\n" + &srv_log)
}

/// The corrupting store, by an oracle the server cannot influence: step window T (the crash's)
/// with a fresh session until g.ptr changes. Returns the pc before the store, the store's, and the
/// one after it. The first and last are read, not computed: the loop may branch.
fn crashy_store(trace: &Path, ptr: u64) -> (u64, u64, u64) {
    let events = retrace_trace::Reader::open(trace).unwrap();
    let t = events.iter().position(|e| matches!(e, retrace_trace::Event::Crash { .. })).expect("a crash");
    let mut s = retrace_core::seek(trace, t, 0).unwrap();
    let before = s.read_mem(ptr, 8).unwrap();
    let mut prev = None;
    loop {
        let pc = s.pc();
        s.step_insns(1).expect("the store comes before the fault");
        if s.read_mem(ptr, 8).unwrap() != before {
            return (prev.expect("the store is not window T's first instruction"), pc, s.pc());
        }
        prev = Some(pc);
    }
}

/// Every value lldb printed after `key` (`old value:` / `new value:`), in order. lldb prints an
/// integer in decimal, or in hex with `0x`. Both are accepted, so the assertions do not depend on
/// the format.
fn values(out: &str, key: &str) -> Vec<u64> {
    out.match_indices(key).filter_map(|(i, _)| {
        let t = out[i + key.len()..].split_whitespace().next()?;
        match t.strip_prefix("0x") { Some(h) => u64::from_str_radix(h, 16).ok(), None => t.parse().ok() }
    }).collect()
}

fn crashy_trace() -> PathBuf {
    let (rec, t) = util::record_dynamic(retrace_guest::CRASHY);
    assert_eq!(rec.code, 139, "record crashy: {}", rec.stderr);
    t
}

fn crashy_script(ptr: u64) -> Vec<String> {
    vec!["process continue".into(), "bt 1".into(),
         format!("watchpoint set expression -w write -s 8 -- {ptr:#x}"),
         "process continue -R".into(), "register read pc".into(),
         "rsi".into(), "register read pc".into(),
         "process continue -F".into(), "register read pc".into()]
}

#[test]
fn lldb_reverse_debugs_crashy_from_the_crash_to_its_corrupting_store() {
    if !lldb_runs() {
        util::announce("SKIPPED lldb_reverse_debugs_crashy…: `/usr/bin/lldb --version` did not run. This gate did NOT run.");
        return;
    }
    let trace = crashy_trace();
    let (_st, ptr) = util::discover_crashy_addrs(&trace);
    let (prev, store, next) = crashy_store(&trace, ptr);
    let (code, out, err) = session(&trace, &crashy_script(ptr));
    let t = format!("exit {code:?}\n--- stdout\n{out}\n--- stderr\n{err}");
    assert!(out.lines().any(|l| l.trim() == "END"), "the batch ran to its end: {t}");
    assert_eq!(code, Some(0), "{t}");
    assert!(out.contains("stop reason = start of recording"), "connect: {t}");
    assert!(out.contains(&format!("stop reason = EXC_BAD_ACCESS (code=1, address={GARBAGE_VA:#x})")), "{t}");
    assert!(out.contains("crashy`main"), "frame #0 is symbolicated from the recording's exe: {t}");
    assert!(out.contains("stop reason = watchpoint 1"), "{t}");
    let pcs: Vec<u64> = out.lines().filter_map(|l| l.trim().strip_prefix("pc = "))
        .map(|v| u64::from_str_radix(v.split_whitespace().next().unwrap().trim_start_matches("0x"), 16).unwrap())
        .collect();
    assert_eq!(pcs, vec![store, prev, next],
        "c -R stops before the store, rsi one instruction earlier, c -F after the store: {t}");
    // Backward, lldb's old value is the one it last saw (the garbage, at the crash), and the new one
    // is memory before the store (&g.buf[0]). Forward again, the store writes the garbage back.
    // `watchpoint set` prints the value it read as a lone `new value:`, which is no stop's report,
    // so the stops' values are read from `c -R` on.
    let stops = &out[out.find("(lldb) process continue -R").unwrap_or_else(|| panic!("c -R ran: {t}"))..];
    assert_eq!(values(stops, "old value:"), vec![GARBAGE_VA, ptr - 32], "{t}");
    assert_eq!(values(stops, "new value:"), vec![ptr - 32, GARBAGE_VA], "{t}");
    assert!(out.contains("stop reason = trace"), "rsi: {t}");
}

#[test]
fn an_lldb_session_is_deterministic() {
    if !lldb_runs() {
        util::announce("SKIPPED an_lldb_session_is_deterministic: `/usr/bin/lldb --version` did not run. This gate did NOT run.");
        return;
    }
    let trace = crashy_trace();
    let (_st, ptr) = util::discover_crashy_addrs(&trace);
    // The port is in the `gdb-remote` line and in the script's path, which lldb echoes
    // (`command source -s 0 '…/retrace-lldb-<pid>-<port>.cmds'`). Both are normalised (t0 L10).
    let norm = |s: &str| s.lines().map(|l| if l.contains("gdb-remote 127.0.0.1:") { "gdb-remote <port>" }
            else if l.contains("retrace-lldb-") { "<script path>" } else { l })
        .collect::<Vec<_>>().join("\n");
    // Each session must itself have run to its end: two that fail the same way (a refused
    // connection, the same stall killed at the bound) would compare equal.
    let run = || {
        let (code, out, err) = session(&trace, &crashy_script(ptr));
        let t = format!("exit {code:?}\n--- stdout\n{out}\n--- stderr\n{err}");
        assert!(out.lines().any(|l| l.trim() == "END"), "the batch ran to its end: {t}");
        assert_eq!(code, Some(0), "{t}");
        out
    };
    let (a, b) = (run(), run());
    assert!(a.contains("stop reason = watchpoint 1"), "the transcript reached the watch: {a}");
    assert_eq!(norm(&a), norm(&b), "two sessions, one transcript (t0 L10)");
}

#[test]
fn lldb_reverse_debugs_cpython_from_the_crash_to_the_store_of_the_pointer() {
    const REAL: &str = "/opt/homebrew/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python";
    const TARGET: u64 = 0x4000_DEAD_0000;
    if !lldb_runs() || !Path::new(REAL).exists() {
        util::announce(&format!("SKIPPED lldb_reverse_debugs_cpython…: needs {LLDB} and {REAL}. This gate did NOT run."));
        return;
    }
    let (rec, trace) = util::record_dynamic_args(REAL, &[retrace_guest::CRASH_PY]);
    let stdout = String::from_utf8_lossy(&rec.stdout).into_owned();
    let start = stdout.find("CRASHPY cell=0x").expect("the marker line") + "CRASHPY cell=0x".len();
    let cell = u64::from_str_radix(&stdout[start..].chars().take_while(|c| c.is_ascii_hexdigit()).collect::<String>(), 16).unwrap();
    // Forward again with `-F`, not `thread step-inst`: after `-R` lldb's direction is reverse, and
    // what a step does then is not something t0 measured. `-F` re-reports the same store, retired
    // (§3c: the reverse stop is an arrival before it).
    let cmds = vec!["process continue".into(),
        format!("watchpoint set expression -w write -s 8 -- {cell:#x}"),
        "process continue -R".into(),
        format!("memory read -s8 -fx -c1 {cell:#x}"),
        "process continue -F".into(),
        format!("memory read -s8 -fx -c1 {cell:#x}")];
    let (code, out, err) = session(&trace, &cmds);
    let t = format!("exit {code:?}\n--- stdout\n{out}\n--- stderr\n{err}");
    assert!(out.lines().any(|l| l.trim() == "END"), "{t}");
    assert_eq!(code, Some(0), "{t}");
    assert!(out.contains(&format!("address={TARGET:#x})")), "the crash is the deref: {t}");
    assert_eq!(out.matches("stop reason = watchpoint 1").count(), 2, "backward, then forward: {t}");
    // By effect (cpython_crash_e2e's proof): before the store the cell is not TARGET; after the
    // forward watch stop it is. `memory read -fx` prints "0x<addr>: 0x<value>".
    let reads: Vec<&str> = out.lines().filter(|l| l.starts_with(&format!("{cell:#x}:"))).collect();
    assert_eq!(reads.len(), 2, "{t}");
    assert!(!reads[0].contains(&format!("{TARGET:#018x}")) && reads[1].contains(&format!("{TARGET:#018x}")), "{t}");
}

/// `thread list`'s rows, as (selected, tid, pc, stop reason). lldb prints
/// `* thread #1: tid = 0x0001, 0x00000001804afaf8, stop reason = instruction step into`.
fn thread_rows(out: &str) -> Vec<(bool, u64, u64, &str)> {
    let hex = |s: &str| u64::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok();
    out.lines().filter_map(|l| {
        let (head, rest) = l.split_once(": tid = ")?;
        let mut f = rest.split(", ");
        let (tid, pc) = (hex(f.next()?)?, hex(f.next()?)?);
        Some((head.trim_start().starts_with('*'), tid, pc, rest.split_once("stop reason = ")?.1))
    }).collect()
}

/// Every `process plugin packet monitor where` reply, past its `at (`: `n, k) phase=… pc=… thread=…`.
/// lldb prints each on a line of its own.
fn wheres(out: &str) -> Vec<&str> { out.lines().filter_map(|l| l.strip_prefix("at (")).collect() }

/// lldb's commands from the connect to the stop at landmark `n`'s svc, with the `m` earlier stops
/// there ignored (`continue_to_window` counts them), and that breakpoint then deleted.
fn to_svc(svc: u64, m: usize) -> Vec<String> {
    vec![format!("breakpoint set -a {svc:#x} -i {m}"), "process continue".into(), "breakpoint delete 1".into()]
}

/// `session`, with lldb's gdb-remote packet log enabled after the connect. Also returns every
/// packet lldb sent, as the log prints it past `send packet: $` (checksum included).
fn packet_logged_session(trace: &Path, cmds: &[String]) -> (Option<i32>, String, String, Vec<String>) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let pkt = std::env::temp_dir().join(format!("retrace-lldb-{}-{n}.packets", std::process::id()));
    let mut all = vec![format!("log enable -f {} gdb-remote packets", pkt.display())];
    all.extend(cmds.iter().cloned());
    let (code, out, err) = session(trace, &all);
    let log = std::fs::read_to_string(&pkt).unwrap_or_default();
    let _ = std::fs::remove_file(&pkt);
    let sends = log.lines().filter_map(|l| l.split_once("send packet: $").map(|(_, p)| p.to_string())).collect();
    (code, out, err, sends)
}

/// A `vCont`'s actions, without the checksum. None for any other packet.
fn vcont_actions(p: &str) -> Option<Vec<&str>> {
    p.strip_prefix("vCont;").map(|b| b.split('#').next().unwrap_or("").split(';').collect())
}
/// A step, as the server reads one: a `vCont` with a step action ANYWHERE (`vCont;c:1;s:2` is a
/// step, gdbserver.rs `handle`'s `vCont` arm).
fn is_step(p: &str) -> bool { vcont_actions(p).is_some_and(|a| a.iter().any(|x| x.starts_with(['s', 'S']))) }
/// A resume: `c`, or a `vCont` of continue actions alone.
fn is_resume(p: &str) -> bool {
    p.starts_with("c#") || vcont_actions(p).is_some_and(|a| a.iter().all(|x| x.starts_with(['c', 'C'])))
}

#[test]
fn lldb_steps_a_blocked_thread_to_where_it_resumes_and_refuses_one_that_is_not_running() {
    // Spec R7's fallback (Ruling T4-a) and §3d rule 1 on the stepped thread (Ruling T4-b), in lldb
    // itself. Each looped lldb-2100 before its fix (Task 4: 307,016 × `vCont;s:1` and 80,103 ×
    // `vCont;s:2` in 60 s). Measured against this row: rule 1 named on the running thread loops
    // session B until the bound kills it, without `END`. The fallback undone no longer loops,
    // because rule 1 now ends lldb's re-step: session A stops refused on the stepped thread, with
    // the other thread at its breakpoint, which the `thread list` assertion catches.
    if !lldb_runs() {
        util::announce("SKIPPED lldb_steps_a_blocked_thread…: `/usr/bin/lldb --version` did not run. This gate did NOT run.");
        return;
    }
    let (tr, n, t) = util::rsp::threadrust_block();
    let svc = util::rsp::trap_pc(tr, n);
    let b = retrace_core::seek(tr, n + 1, 0).unwrap().pc(); // where the other thread runs first
    // lldb's ignore count: the stops at svc before window n, counted over the wire. `n` varies
    // between recordings, so it is never a constant.
    let m = {
        let mut c = util::rsp::Rsp::spawn(tr, &[]);
        assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
        util::rsp::continue_to_window(&mut c, n).0
    };
    let (me, other) = (u64::from(t) + 1, if t == 0 { 2 } else { 1 }); // RSP tids
    // Session B's pc oracle: `dbg_regs_of` at the refusal's own position, window n's trap, since a
    // refusal moves nothing. Not `b`: that is (n + 1, 0), a later position, which agrees only if
    // the switch resumes the thread at exactly its saved pc.
    let other_pc = {
        let len = retrace_core::seek(tr, n, 0).unwrap().window_len_here().unwrap();
        let s = retrace_core::seek(tr, n, len).unwrap();
        util::rsp::dbg_field(&s.dbg_regs_of(other as usize - 1).unwrap(), "pc")
    };
    let where_ = || "process plugin packet monitor where".to_string();

    // Session A: the step blocks and ends on the stepped thread at svc + 4, not at the other
    // thread's breakpoint on the way.
    let mut a = to_svc(svc, m);
    a.extend([where_(), format!("breakpoint set -a {b:#x}"), "thread step-inst".into(), "thread list".into(),
              where_()]);
    let (code, out, err) = session(tr, &a);
    let ta = format!("exit {code:?}\n--- stdout\n{out}\n--- stderr\n{err}");
    assert!(out.lines().any(|l| l.trim() == "END"), "the batch ran to its end: {ta}");
    assert_eq!(code, Some(0), "{ta}");
    let rows = thread_rows(&out);
    assert!(rows.contains(&(true, me, svc + 4, "instruction step into")), "{rows:?}: {ta}");
    // Before the step, `where` pins where lldb stopped: in window n, the wait that blocked, so `m`
    // is asserted rather than inferred. After it, a later landmark shows that other threads ran
    // during the step. Together: the step crossed the block.
    let w = wheres(&out);
    assert_eq!(w.len(), 2, "{ta}");
    assert!(w[0].starts_with(&format!("{n}, ")), "lldb stopped in window n = {n}: {ta}");
    let landed: usize = w[1].split(',').next().unwrap().parse().unwrap();
    assert!(landed > n + 1, "other threads ran during the step (n = {n}): {ta}");

    // Session B: a step on the thread that is not running is refused, named on that thread.
    // lldb numbers threads in the order it first sees them, and thread 1 is alone at the start of
    // recording, so the other thread's lldb index is its RSP tid; the `*` row's tid confirms it.
    let mut bb = to_svc(svc, m);
    bb.extend([format!("thread select {other}"), "thread step-inst".into(), "thread list".into()]);
    let (code, out, err) = session(tr, &bb);
    let tb = format!("exit {code:?}\n--- stdout\n{out}\n--- stderr\n{err}");
    assert!(out.lines().any(|l| l.trim() == "END"), "the batch ran to its end: {tb}");
    assert_eq!(code, Some(0), "{tb}");
    let refused = thread_rows(&out).into_iter().find(|r| r.0).unwrap_or_else(|| panic!("a selected thread: {tb}"));
    assert_eq!(refused.1, other, "{tb}");
    assert!(refused.3.starts_with(&format!("cannot step thread {other}: ")), "{refused:?}: {tb}");
    assert_eq!(refused.2, other_pc, "the refused thread's own saved pc, not the running one's: {tb}");
}

#[test]
fn lldb_steps_a_thread_across_its_own_exit_without_looping() {
    // M44 B3's loop check in lldb itself (spec §3c): M43 measured lldb re-stepping forever when a
    // step was answered on another thread, and B3's stop is named on the running thread. The
    // session is bounded (BOUND), so a loop fails here as a killed session with no END.
    //
    // lldb-2100 does not loop, and it does not display B3's own stop either. The server answers the
    // step with the exception stop on the running thread, tid 1 (`gdbserver_e2e`'s
    // `a_step_across_the_stepped_threads_own_exit_stops_there` pins that). lldb suspended tid 1 for
    // the `vCont;s:2`, so it ignores the stop. Its step log (`log enable lldb step thread`,
    // `docs/sweep-evidence/2026-09-27-m44-t0/t8/t8-lldb-steplog.log`, line 46) reads
    // `Thread::ShouldStop for tid = 0x0001 0x0001, should_stop = 0 (ignore since thread was
    // suspended)`. The stepped thread is gone, so nothing re-steps: lldb sends one `c`. With no
    // breakpoint ahead, that runs to the end of the recording. This is lldb's behaviour, not the
    // server's, pinned so that a change in it is seen; the display is routed (M44 Ruling T8-a). The
    // `c` is a real continue from the exit's boundary, so lldb no longer skips the breakpoints
    // after the exit (the next row).
    if !lldb_runs() {
        util::announce("SKIPPED lldb_steps_a_thread_across_its_own_exit…: `/usr/bin/lldb --version` did not run. This gate did NOT run.");
        return;
    }
    let (tr, x, child) = util::rsp::threadrust_child_exit();
    let ev = retrace_trace::Reader::open(tr).unwrap();
    let (exit_code, exit_thread) = ev.iter().find_map(|e| match e {
        retrace_trace::Event::Exit { code, thread } => Some((*code, *thread)),
        _ => None,
    }).expect("threadrust exits");
    let svc = util::rsp::trap_pc(tr, x);
    let m = {
        let mut c = util::rsp::Rsp::spawn(tr, &[]);
        assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
        util::rsp::continue_to_window(&mut c, x).0
    };
    let mut cmds = to_svc(svc, m);
    cmds.extend([format!("thread select {}", child + 1), "thread step-inst".into(), "thread list".into()]);
    let (code, out, err, sends) = packet_logged_session(tr, &cmds);
    let t = format!("exit {code:?}\n--- stdout\n{out}\n--- stderr\n{err}");
    assert!(out.lines().any(|l| l.trim() == "END"), "the batch ran to its end — no re-step loop: {t}");
    assert_eq!(code, Some(0), "{t}");
    // The step is sent once and never re-sent, and lldb's one resume after it is a continue.
    assert!(!sends.is_empty(), "lldb's packet log is empty: {t}");
    let steps: Vec<usize> = (0..sends.len()).filter(|&i| is_step(&sends[i])).collect();
    assert_eq!(steps.len(), 1, "one step, never re-sent: {sends:?}: {t}");
    let resumes = sends[steps[0] + 1..].iter().filter(|p| is_resume(p)).count();
    assert_eq!(resumes, 1, "lldb resumed once after the ignored stop: {sends:?}: {t}");
    // lldb's stop is the end of the recording, on the thread that exits the process. Measured:
    // `* thread #1: tid = 0x0001, 0x00000001804b5580, stop reason = exited (code 0)`.
    let sel = thread_rows(&out).into_iter().find(|r| r.0).unwrap_or_else(|| panic!("a selected thread: {t}"));
    assert_eq!((sel.1, sel.3), (u64::from(exit_thread) + 1, format!("exited (code {exit_code})").as_str()), "{t}");
}

#[test]
fn lldb_stops_at_a_breakpoint_past_a_step_across_the_threads_own_exit() {
    // M44 B3's effect in lldb (Ruling T8-b). lldb ignores B3's own stop and resumes, as the row
    // above pins. That resume is now a real continue from the exit's boundary, with lldb's
    // breakpoints armed. Before M44, the step's run until the stepped thread was current again
    // armed nothing and consumed the rest of the recording. A breakpoint that main reaches after
    // the child's exit was skipped, and lldb stopped at the end.
    if !lldb_runs() {
        util::announce("SKIPPED lldb_stops_at_a_breakpoint_past_a_step_across…: `/usr/bin/lldb --version` did not run. This gate did NOT run.");
        return;
    }
    let (tr, x, child) = util::rsp::threadrust_child_exit();
    let ev = retrace_trace::Reader::open(tr).unwrap();
    let main = match ev[x + 1] {
        retrace_trace::Event::Syscall { thread, .. } => thread,
        _ => panic!("landmark {} is not a syscall", x + 1),
    };
    assert_ne!(main, child, "window {} runs after the child's exit, on another thread", x + 1);
    // The svc that ends window x + 1. Main runs it after the child has exited, and no earlier
    // instruction of that window is a svc, so it is the breakpoint's first hit after the exit.
    let b = util::rsp::trap_pc(tr, x + 1);
    let svc = util::rsp::trap_pc(tr, x);
    let m = {
        let mut c = util::rsp::Rsp::spawn(tr, &[]);
        assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
        util::rsp::continue_to_window(&mut c, x).0
    };
    let mut cmds = to_svc(svc, m);
    cmds.extend([format!("breakpoint set -a {b:#x}"), format!("thread select {}", child + 1),
                 "thread step-inst".into(), "thread list".into(), "process plugin packet monitor where".into()]);
    let (code, out, err) = session(tr, &cmds);
    let t = format!("exit {code:?}\n--- stdout\n{out}\n--- stderr\n{err}");
    assert!(out.lines().any(|l| l.trim() == "END"), "the batch ran to its end: {t}");
    assert_eq!(code, Some(0), "{t}");
    let sel = thread_rows(&out).into_iter().find(|r| r.0).unwrap_or_else(|| panic!("a selected thread: {t}"));
    assert_eq!(sel, (true, u64::from(main) + 1, b, "breakpoint 2.1"), "main, at the breakpoint past the exit: {t}");
    let w = wheres(&out);
    assert_eq!(w.len(), 1, "{t}");
    assert!(w[0].starts_with(&format!("{}, ", x + 1)), "in window {}, not at the end of the recording: {t}", x + 1);
}

#[test]
fn lldb_reverse_steps_back_onto_another_threads_trap() {
    // Spec §3e (Ruling T5-b): a reverse step can cross a landmark backward onto another thread's
    // trap, and the reply names the thread current there. lldb has no step plan for a `bc`, so it
    // displays that stop instead of re-stepping. After the blocked step, window L begins on the
    // stepped thread, and window L - 1 ended at the other thread's svc.
    if !lldb_runs() {
        util::announce("SKIPPED lldb_reverse_steps_back…: `/usr/bin/lldb --version` did not run. This gate did NOT run.");
        return;
    }
    let (tr, n, t) = util::rsp::threadrust_block();
    let svc = util::rsp::trap_pc(tr, n);
    // The oracle, over the wire before lldb runs: the same blocked step, and the landmark L it ends at.
    let (m, l) = {
        let mut c = util::rsp::Rsp::spawn(tr, &[]);
        assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
        let (m, _) = util::rsp::continue_to_window(&mut c, n);
        assert_eq!(c.send(&format!("z0,{svc:x},4")), "OK");
        let s = c.send(&format!("vCont;s:{:x}", t + 1));
        assert!(s.contains("reason:trace;"), "{s}");
        let w = c.where_();
        (m, w["at (".len()..].split(',').next().unwrap().parse::<usize>().unwrap())
    };
    let ev = retrace_trace::Reader::open(tr).unwrap();
    let back = match &ev[l - 1] {
        retrace_trace::Event::Syscall { thread, .. } => *thread,
        _ => panic!("landmark {} is not a syscall", l - 1),
    };
    assert_ne!(back, t, "window {} ends on another thread, so the step back crosses threads", l - 1);
    let (tid, pc) = (u64::from(back) + 1, util::rsp::trap_pc(tr, l - 1));
    let len = retrace_core::seek(tr, l - 1, 0).unwrap().window_len_here().unwrap();
    let mut cmds = to_svc(svc, m);
    cmds.extend(["thread step-inst".into(), "rsi".into(), "thread list".into(),
                 "process plugin packet monitor where".into()]);
    let (code, out, err) = session(tr, &cmds);
    let tt = format!("exit {code:?}\n--- stdout\n{out}\n--- stderr\n{err}");
    assert!(out.lines().any(|l| l.trim() == "END"), "the batch ran to its end: {tt}");
    assert_eq!(code, Some(0), "{tt}");
    // By tid, never lldb's index: lldb renumbers a thread it saw exit (Task 5's probe listed tid 2
    // as `#3` here).
    let row = thread_rows(&out).into_iter().find(|r| r.1 == tid)
        .unwrap_or_else(|| panic!("tid {tid} is listed with a stop reason: {tt}"));
    assert_eq!(row, (true, tid, pc, "trace"), "{tt}");
    let w = wheres(&out);
    assert_eq!(w.len(), 1, "{tt}");
    assert!(w[0].starts_with(&format!("{}, {len}) ", l - 1)), "at window L - 1's trap: {tt}");
}
