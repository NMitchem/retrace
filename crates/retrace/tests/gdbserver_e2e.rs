//! M43: the gdb-remote server, over the wire, without lldb (spec
//! `docs/superpowers/specs/2026-09-25-retrace-m43-lldb-design.md` §4). Repo-owned: it guards the
//! protocol on any machine. `lldb_e2e` guards lldb itself. Every expected value comes from a source
//! the server cannot influence: the recording, the fixture binary, or a fresh `ReplaySession` in
//! this process. The server is its own process, so the two VMs never meet.
mod util;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use util::rsp::{self as r, Rsp};

fn watchsweep() -> &'static Path {
    static C: OnceLock<PathBuf> = OnceLock::new();
    C.get_or_init(|| {
        let (rec, t) = util::record(retrace_guest::WATCHSWEEP);
        assert_eq!(rec.code, 0, "record watchsweep: {}", rec.stderr);
        t
    })
}
fn crashy() -> &'static Path {
    static C: OnceLock<PathBuf> = OnceLock::new();
    C.get_or_init(|| {
        let (rec, t) = util::record_dynamic(retrace_guest::CRASHY);
        assert_eq!(rec.code, 139, "record crashy: {}", rec.stderr);
        t
    })
}
fn hexs(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }

#[test]
fn the_handshake_stops_at_the_start_of_recording() {
    let mut c = Rsp::spawn(watchsweep(), &[]);
    let sup = c.send("qSupported:xmlRegisters=i386,arm,mips,arc;multiprocess+;swbreak+;hwbreak+");
    assert!(sup.contains("ReverseContinue+") && sup.contains("qXfer:features:read+"), "{sup}");
    let stop = c.send("?");
    assert!(stop.starts_with("T05thread:1;threads:1;"), "{stop}");
    assert_eq!(r::description(&stop).as_deref(), Some("start of recording"));
    assert!(stop.contains("replaylog:begin;"), "{stop}");
    assert_eq!(c.send("qC"), "QC1");
    assert_eq!(c.send("qfThreadInfo"), "m1");
    assert_eq!(c.send("qsThreadInfo"), "l");
    assert_eq!(c.send("vCont?"), "vCont;c;C;s;S");
    let xml = c.send("qXfer:features:read:target.xml:0,1ffff");
    assert!(xml.starts_with('l') && xml.contains(r#"regnum="67""#), "{xml}");
    assert_eq!(c.where_(), format!("at (1, 0) phase=Bp pc={:#x} thread=1",
        retrace_core::seek(watchsweep(), 1, 0).unwrap().pc()));
}

#[test]
fn registers_match_a_replay_session_at_the_same_position() {
    let s = retrace_core::seek(watchsweep(), 1, 0).unwrap();
    // The oracle is the script debugger's text dump, not thread_ctx. It pads single-digit names
    // (`x1 =0x…`, `format_gprs`), so close the gap before splitting.
    let text = s.dbg_regs().replace(" =", "=");
    let field = |name: &str| text.split_whitespace().find_map(|w| w.strip_prefix(&format!("{name}=")))
        .map(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).unwrap())
        .unwrap_or_else(|| panic!("no {name}= in dbg_regs:\n{text}"));
    let mut c = Rsp::spawn(watchsweep(), &[]);
    assert_eq!(r::le_u64(&c.send("p20;thread:1;")), s.pc(), "pc");
    assert_eq!(r::le_u64(&c.send("p1f;thread:1;")), field("sp"), "sp");
    let g = c.send("g");
    assert_eq!(g.len(), 788 * 2);
    for i in 0..31usize {
        assert_eq!(r::le_u64(&g[i * 16..i * 16 + 16]), field(&format!("x{i}")), "g's x{i}");
    }
    assert_eq!(r::le_u64(&g[32 * 16..32 * 16 + 16]), s.pc(), "g's pc slot");
    assert_eq!(c.send("p0;thread:9;"), "E01", "a thread that does not exist (Review Focus 4)");
    assert_eq!(c.send("p44;thread:1;"), "E01", "a register past fpcr");
}

#[test]
fn memory_reads_match_the_recording_and_a_straddling_read_returns_its_prefix() {
    let s = retrace_core::seek(watchsweep(), 1, 0).unwrap();
    let pc = s.pc();
    let mut c = Rsp::spawn(watchsweep(), &[]);
    assert_eq!(c.send(&format!("m{pc:x},10")), hexs(&s.read_mem(pc, 16).unwrap()));
    // Find the end of the mapping above pc, 16 KiB at a time, then read across it.
    let mut end = None;
    for j in 1..=256u64 {
        let a = (pc & !0x3fff) + j * 0x4000;
        if c.send(&format!("m{a:x},1")) == "E08" { end = Some(a); break; }
    }
    let end = end.expect("an unmapped page within 4 MiB of the code");
    assert_eq!(c.send(&format!("m{:x},10", end - 8)).len(), 16, "8 readable bytes, then the gap");
    assert_eq!(c.send("mfffffff000002010,8"), "E08", "lldb's kernel probe: refused, no panic (Review Focus 2)");
}

#[test]
fn writes_are_refused_and_move_nothing() {
    let mut c = Rsp::spawn(watchsweep(), &[]);
    let before = c.where_();
    for p in ["P0=0000000000000000", "G00", "M100004000,1:00", "X100004000,0:", "QSaveRegisterState",
              "QRestoreRegisterState:1"] {
        assert_eq!(c.send(p), "E01", "{p}");
    }
    assert_eq!(c.send("_M1000,rwx"), "", "no allocation (spec R8)");
    assert_eq!(c.where_(), before);
}

/// The first `LC_UUID` in a thin Mach-O file, formatted as lldb prints it.
fn file_uuid(path: &str) -> String {
    let f = std::fs::read(path).unwrap();
    let u32at = |o: usize| u32::from_le_bytes(f[o..o + 4].try_into().unwrap());
    let (mut off, n) = (32usize, u32at(16));
    for _ in 0..n {
        if u32at(off) == 0x1b {
            let h = hexs(&f[off + 8..off + 24]).to_uppercase();
            return format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]);
        }
        off += u32at(off + 4) as usize;
    }
    panic!("no LC_UUID in {path}")
}

#[test]
fn the_exe_is_listed_from_argv0_when_the_recording_has_one() {
    let mut c = Rsp::spawn(crashy(), &[]);
    assert!(c.send("qHostInfo").contains("os_version:"), "a known path selects lldb's newer loader");
    assert_eq!(c.send("jGetLoadedDynamicLibrariesInfos:"), "OK");
    let j = c.send(r#"jGetLoadedDynamicLibrariesInfos:{"fetch_all_solibs":true}"#);
    assert!(j.contains(&format!(r#""pathname":"{}""#, retrace_guest::CRASHY)), "{j}");
    assert!(j.contains(r#""load_address":4294967296"#), "{j}");
    assert!(j.contains(&format!(r#""uuid":"{}""#, file_uuid(retrace_guest::CRASHY))), "{j}");
}

#[test]
fn a_static_guest_lists_nothing_unless_given_its_path() {
    let mut c = Rsp::spawn(watchsweep(), &[]);
    assert!(!c.send("qHostInfo").contains("os_version:"), "no path: lldb's older loader (t0 L2)");
    assert_eq!(c.send("jGetLoadedDynamicLibrariesInfos:"), "");
    drop(c);
    let mut c = Rsp::spawn(watchsweep(), &["--exe", retrace_guest::WATCHSWEEP]);
    assert!(c.send("qHostInfo").contains("os_version:"));
    let j = c.send(r#"jGetLoadedDynamicLibrariesInfos:{"fetch_all_solibs":true}"#);
    assert!(j.contains(&format!(r#""pathname":"{}""#, retrace_guest::WATCHSWEEP)), "{j}");
}

#[test]
fn k_and_d_end_the_server_cleanly_and_so_does_a_dropped_connection() {
    let (reply, code) = Rsp::spawn(watchsweep(), &[]).kill();
    assert_eq!((reply.as_str(), code), ("X09", 0));
    let (reply, code) = Rsp::spawn(watchsweep(), &[]).detach();
    assert_eq!((reply.as_str(), code), ("OK", 0));
    assert_eq!(Rsp::spawn(watchsweep(), &[]).drop_connection(), 0, "EOF ends the session (Review Focus 3)");
}

#[test]
fn a_peer_that_resets_the_connection_ends_the_server_cleanly() {
    // Before Ruling T2-a this exited 5, `GDBSERVER ERROR: socket: Connection reset by peer`.
    assert_eq!(Rsp::spawn(watchsweep(), &[]).reset_connection(), 0, "an RST is the peer leaving too (§3a)");
}

#[test]
fn a_second_connection_is_refused_while_the_first_is_served() {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    let (mut child, port, err_p) = r::spawn_server(watchsweep(), &[]);
    let _ = std::fs::remove_file(err_p);
    let mut first = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    first.set_read_timeout(Some(std::time::Duration::from_secs(180))).unwrap();
    first.write_all(b"$?#3f").unwrap();
    let mut ack = [0u8; 1];
    assert_eq!(first.read(&mut ack).unwrap(), 1, "the first connection is served, so it was accepted");
    let second = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.kind());
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(second.err(), Some(std::io::ErrorKind::ConnectionRefused), "one connection per server (§3a)");
}

/// `&buf[40]`: the address watchsweep publishes in its write(1, …) (the watchsweep_e2e oracle).
fn ws_target() -> u64 {
    let mut s = retrace_core::ReplaySession::open(watchsweep()).unwrap();
    loop {
        if let Some((4, args)) = s.peek_syscall() { if args[0] == 1 { return args[1]; } }
        s.advance().unwrap();
    }
}
fn pc_of(stop: &str) -> u64 { r::le_u64(r::key(stop, "20").expect("an expedited pc")) }
fn mem_u64(c: &mut Rsp, a: u64) -> u64 { r::le_u64(&c.send(&format!("m{a:x},8"))) }

#[test]
fn a_forward_watch_is_reported_after_its_store_and_a_reverse_one_before_it() {
    // §3c rows 2 and 5. watchsweep writes buf[40] twice: the sweeping store (0x1111…+40), then
    // a second store (0xbeef).
    let t = ws_target();
    let mut c = Rsp::spawn(watchsweep(), &[]);
    assert_eq!(c.send(&format!("Z2,{t:x},8")), "OK");
    let s1 = c.send("c");
    assert_eq!(r::key(&s1, "watch"), Some(format!("{t:x}").as_str()), "{s1}");
    assert_eq!(mem_u64(&mut c, t), 0x1111_1111_1111_1111 + 40, "post-retire: the new value");
    let sweep_after = pc_of(&s1);
    let s2 = c.send("c");
    assert_eq!(mem_u64(&mut c, t), 0xbeef, "{s2}");
    let second_after = pc_of(&s2);
    // Backward: the second store, pre-retire, with the old value in memory.
    let b1 = c.send("bc");
    assert_eq!(r::key(&b1, "watch"), Some(format!("{t:x}").as_str()), "{b1}");
    assert_eq!(pc_of(&b1), second_after - 4, "before the store");
    assert_eq!(mem_u64(&mut c, t), 0x1111_1111_1111_1111 + 40);
    // §3c row 5's point: from a reverse stop, forward reports that same store again (control C2).
    let f = c.send("c");
    assert_eq!(pc_of(&f), second_after, "{f}");
    assert_eq!(mem_u64(&mut c, t), 0xbeef);
    let b2 = c.send("bc");
    assert_eq!(pc_of(&b2), second_after - 4);
    let b3 = c.send("bc");
    assert_eq!(pc_of(&b3), sweep_after - 4, "the sweeping store, before it wrote buf[40]");
    assert_eq!(mem_u64(&mut c, t), 0);
}

#[test]
fn no_earlier_hit_goes_to_the_start_and_the_end_of_recording_is_reversible() {
    // §3c rows 7 and 8, and the exit terminal.
    let mut c = Rsp::spawn(watchsweep(), &[]);
    let entry = retrace_core::seek(watchsweep(), 1, 0).unwrap().pc();
    let end = c.send("c");
    assert!(end.starts_with("T05") && end.contains("replaylog:end;"), "{end}");
    assert_eq!(r::description(&end).as_deref(), Some("exited (code 0)"));
    assert_eq!(c.send("c"), end, "continue at the end reports the end again");
    let back = c.send("bc");
    assert_eq!(r::description(&back).as_deref(), Some("start of recording"), "{back}");
    assert_eq!(pc_of(&back), entry);
    assert!(c.where_().starts_with("at (1, 0) phase=Bp"));
}

#[test]
fn a_syscall_write_is_reported_after_the_syscall_forward_and_at_its_trap_backward() {
    // §3c rows 3 and 6, on crashy's fstat(1, &g.st).
    let (st, _ptr) = util::discover_crashy_addrs(crashy());
    let mut c = Rsp::spawn(crashy(), &[]);
    assert_eq!(c.send(&format!("Z2,{st:x},8")), "OK");
    let f = c.send("c");
    assert_eq!(r::key(&f, "watch"), Some(format!("{st:x}").as_str()), "{f}");
    let written = mem_u64(&mut c, st);
    assert_ne!(written, 0, "after the syscall: its write is in memory");
    let b = c.send("bc");
    assert_eq!(r::key(&b, "watch"), Some(format!("{st:x}").as_str()), "{b}");
    let pc = pc_of(&b);
    assert_eq!(r::le_u64(&c.send(&format!("m{pc:x},4"))) as u32, 0xd400_1001, "parked at the svc #0x80");
    assert_eq!(mem_u64(&mut c, st), 0, "before the syscall: g.st is still BSS");
    let again = c.send("c");
    assert_eq!(again, f, "forward from the trap crosses it and reports the same write");
}

#[test]
fn a_step_whose_crossing_writes_a_watched_cell_answers_watch_and_reverse_finds_the_write() {
    // Final review Minor 2: §3d rule 3's crossing with a syscall write (`step_thread`'s
    // `WatchSyscall` arm, then `reply_forward`'s `WatchSys`). `s` on crashy's fstat(1, &g.st) svc.
    let (st, _ptr) = util::discover_crashy_addrs(crashy());
    let ev = retrace_trace::Reader::open(crashy()).unwrap();
    let n = ev.iter().position(|e| matches!(e, retrace_trace::Event::Syscall { num, args, .. }
            if matches!(*num, retrace_arch::SYS_FSTAT | retrace_arch::SYS_FSTAT64) && args[1] == st))
        .expect("crashy's fstat(1, &g.st)");
    let svc = r::trap_pc(crashy(), n);
    // Old and new by in-process seeks, either side of the syscall, before the server spawns.
    let cell = |k: usize| u64::from_le_bytes(retrace_core::seek(crashy(), k, 0).unwrap()
        .read_mem(st, 8).unwrap().try_into().unwrap());
    let (old, new) = (cell(n), cell(n + 1));
    assert_ne!(old, new, "the syscall writes the watched word");
    let mut c = Rsp::spawn(crashy(), &[]);
    assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
    let (_, at) = r::continue_to_window(&mut c, n);
    assert_eq!(pc_of(&at), svc, "{at}");
    assert_eq!(c.send(&format!("z0,{svc:x},4")), "OK");
    assert_eq!(c.send(&format!("Z2,{st:x},8")), "OK");
    assert_eq!(mem_u64(&mut c, st), old);
    let s = c.send("s");
    assert_eq!(r::key(&s, "watch"), Some(format!("{st:x}").as_str()), "not a plain trace: {s}");
    assert_eq!(pc_of(&s), svc + 4, "the syscall returned");
    assert_eq!(mem_u64(&mut c, st), new, "the new value");
    assert!(c.where_().starts_with(&format!("at ({}, 0) phase=Bp", n + 1)), "{}", c.where_());
    let b = c.send("bc");
    assert_eq!(r::key(&b, "watch"), Some(format!("{st:x}").as_str()), "the same write: {b}");
    assert_eq!(pc_of(&b), svc, "at its trap");
    assert_eq!(mem_u64(&mut c, st), old, "before the syscall");
}

#[test]
fn the_crash_is_exc_bad_access_and_reverse_reaches_the_corrupting_store() {
    // The headline, without lldb: §3c's terminal and row 5 on crashy.
    const GARBAGE_VA: u64 = 0x4000_DEAD_0000;
    let (_st, ptr) = util::discover_crashy_addrs(crashy());
    let crash_pc = retrace_trace::Reader::open(crashy()).unwrap().iter().find_map(|e| match e {
        retrace_trace::Event::Crash { pc, .. } => Some(*pc), _ => None }).unwrap();
    let mut c = Rsp::spawn(crashy(), &[]);
    let crash = c.send("c");
    assert!(crash.starts_with("T0b"), "{crash}");
    assert!(crash.contains(&format!("metype:1;mecount:2;medata:1;medata:{GARBAGE_VA:x};")), "{crash}");
    assert_eq!(pc_of(&crash), crash_pc);
    assert_eq!(c.send(&format!("Z2,{ptr:x},8")), "OK");
    let b = c.send("bc");
    assert_eq!(r::key(&b, "watch"), Some(format!("{ptr:x}").as_str()), "{b}");
    assert_eq!(mem_u64(&mut c, ptr), ptr - 32, "before the store: g.ptr is still &g.buf[0]");
    let f = c.send("c");
    assert_eq!(mem_u64(&mut c, ptr), GARBAGE_VA, "after it: the garbage");
    assert_eq!(pc_of(&f), pc_of(&b) + 4);
    assert_eq!(c.send("c"), crash, "then the crash again");
}

/// `name`'s address in the fixture binary (`nm`, as `llsc_e2e`'s `sym` does). A static guest loads
/// at its link address, so this is the guest pc.
fn sym(bin: &str, name: &str) -> u64 {
    let out = std::process::Command::new("nm").arg(bin).output().expect("nm");
    assert!(out.status.success(), "nm {bin}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().lines().find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        (f.len() == 3 && f[2] == name).then(|| u64::from_str_radix(f[0], 16).unwrap())
    }).unwrap_or_else(|| panic!("no symbol {name} in {bin}"))
}

#[test]
fn a_breakpoint_is_reported_at_its_pc_forward_and_backward() {
    // Ruling T3-a: §3c rows 1 and 4 on the wire. watchsweep's sweeping store runs once per pass of
    // its 64-pass loop, so one breakpoint on it hits again and again.
    let (start, sweep) = (sym(retrace_guest::WATCHSWEEP, "_start"), sym(retrace_guest::WATCHSWEEP, "sweep"));
    let store = sweep + 4; // `add x4, x2, x3`, then THE store
    // Straight-line code from `_start` to the first pass, and a five-instruction loop body
    // (watchsweep.s), so the cursor at each hit is known from the binary alone.
    let at = |k: u64| format!("at (1, {k}) phase=Bp pc={store:#x} thread=1");
    let first_k = (store - start) / 4;
    let mut c = Rsp::spawn(watchsweep(), &[]);
    assert_eq!(c.send(&format!("Z0,{store:x},4")), "OK");
    let f1 = c.send("c");
    assert!(f1.starts_with("T05") && f1.ends_with("reason:breakpoint;"), "{f1}");
    assert_eq!(pc_of(&f1), store);
    assert_eq!(c.where_(), at(first_k), "the first pass");
    let f2 = c.send("c");
    assert!(f2.ends_with("reason:breakpoint;"), "{f2}");
    assert_eq!(pc_of(&f2), store);
    assert_eq!(c.where_(), at(first_k + 5), "the next pass: the breakpoint it stood on is not re-reported");
    let b = c.send("bc");
    assert!(b.starts_with("T05") && b.ends_with("reason:breakpoint;"), "{b}");
    assert_eq!(pc_of(&b), store);
    assert_eq!(c.where_(), at(first_k), "back to the first pass");
}

#[test]
fn breakpoints_cap_at_six_and_reinsertion_is_idempotent() {
    // §3f, spec R4 as amended (Ruling T3-b), Review Focus 5. Six is the hardware's count
    // (DBGBVR0-5) and `cmd_break`'s own limit. Seven distinct pcs: base..base+20, and base+40.
    let mut c = Rsp::spawn(watchsweep(), &[]);
    let base = retrace_core::seek(watchsweep(), 1, 0).unwrap().pc();
    for i in 0..6u64 { assert_eq!(c.send(&format!("Z0,{:x},4", base + 4 * i)), "OK"); }
    assert_eq!(c.send(&format!("Z0,{base:x},4")), "OK", "a duplicate is not a seventh");
    assert_eq!(c.send(&format!("Z1,{:x},4", base + 40)), "E01", "the seventh is refused: all six slots are armed");
    assert_eq!(c.send(&format!("z0,{base:x},4")), "OK");
    assert_eq!(c.send(&format!("z0,{base:x},4")), "OK", "removing an absent one is OK");
    assert_eq!(c.send(&format!("Z0,{:x},4", base + 40)), "OK", "the slot is free again");
}

#[test]
fn watchpoints_are_write_only_and_cap_at_four() {
    let t = ws_target();
    let mut c = Rsp::spawn(watchsweep(), &[]);
    assert_eq!(c.send(&format!("Z3,{t:x},8")), "E01", "read");
    assert_eq!(c.send(&format!("Z4,{t:x},8")), "E01", "access");
    assert_eq!(c.send(&format!("Z2,{:x},8", t + 1)), "E01", "misaligned");
    assert_eq!(c.send(&format!("Z2,{t:x},3")), "E01", "a size that is not 1, 2, 4 or 8");
    for i in 0..4u64 { assert_eq!(c.send(&format!("Z2,{:x},8", t + 8 * i)), "OK"); }
    assert_eq!(c.send(&format!("Z2,{t:x},8")), "OK", "a duplicate is not a fifth");
    assert_eq!(c.send(&format!("Z2,{:x},8", t + 64)), "E01", "the fifth");
    assert_eq!(c.send(&format!("z2,{:x},8", t + 128)), "OK", "removing an absent watch is OK");
}

#[test]
fn a_divergence_is_a_stop_at_the_saved_cursor_never_an_error_reply() {
    // §3b and M41's owed `?`-armed session: the scan diverges mid-flight with a breakpoint armed. The
    // reply is an exception stop (an `E` would drop lldb, t0 L7), and the cursor is where it was.
    let bad = util::tamper_last_write(watchsweep());
    let exit_svc = retrace_core::seek(watchsweep(), 2, 0).unwrap().pc() + 8; // mov x0; mov x16; svc
    let mut c = Rsp::spawn(&bad, &[]);
    assert_eq!(c.send(&format!("Z0,{exit_svc:x},4")), "OK");
    let before = c.where_();
    let s = c.send("c");
    assert!(s.starts_with("T05") && s.contains("reason:exception;"), "{s}");
    assert!(r::description(&s).unwrap().contains("diverged"), "{s}");
    assert_eq!(c.where_(), before, "recovered to the saved cursor");
    assert_eq!(c.send("c"), s, "deterministic: the same divergence again");
}

#[test]
fn a_flag_given_last_without_its_value_is_a_usage_error() {
    // A trace that does not exist: before the fix, `--port` last meant port 0 and the server went on
    // to fail opening it (exit 5). A usage error is found before any file is touched.
    for flag in ["--port", "--exe"] {
        let out = std::process::Command::new(util::bin()).args(["gdbserver", "no-such.trace", flag])
            .output().expect("run retrace");
        assert_eq!(out.status.code(), Some(2), "{flag}: {}", String::from_utf8_lossy(&out.stderr));
    }
}

#[test]
fn a_step_crosses_a_syscall_and_lands_after_it() {
    // §3d rule 3, AtTrap: `si` on an `svc` is ordinary. watchsweep's window 1 ends at its write.
    let svc = r::trap_pc(watchsweep(), 1);
    let mut c = Rsp::spawn(watchsweep(), &[]);
    assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
    let at = c.send("c");
    assert_eq!(pc_of(&at), svc, "{at}");
    assert_eq!(c.send(&format!("z0,{svc:x},4")), "OK");
    let s = c.send("vCont;s:1");
    assert!(s.contains("reason:trace;"), "{s}");
    assert_eq!(pc_of(&s), svc + 4, "the syscall returned");
    assert!(c.where_().starts_with("at (2, 0) phase=Bp"), "{}", c.where_());
}

#[test]
fn a_step_over_a_blocking_syscall_ends_when_the_stepped_thread_runs_again() {
    // §3d, the until-thread run (control C4). The wait blocks, other threads run, and the step
    // ends on the stepped thread at its svc + 4, some landmarks later. Answering on another thread
    // would loop lldb forever (t0 L7, 349,194 steps in 60 s).
    let (tr, n, t) = r::threadrust_block();
    let svc = r::trap_pc(tr, n);
    let mut c = Rsp::spawn(tr, &[]);
    assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
    let (_, at) = r::continue_to_window(&mut c, n);
    assert_eq!(r::key(&at, "thread"), Some(format!("{:x}", t + 1).as_str()), "{at}");
    assert_eq!(pc_of(&at), svc);
    assert_eq!(c.send(&format!("z0,{svc:x},4")), "OK");
    let s = c.send(&format!("vCont;s:{:x}", t + 1));
    assert!(s.contains("reason:trace;"), "{s}");
    assert_eq!(r::key(&s, "thread"), Some(format!("{:x}", t + 1).as_str()), "on the stepped thread: {s}");
    assert_eq!(pc_of(&s), svc + 4);
    let w = c.where_();
    let landed: usize = w["at (".len()..].split(',').next().unwrap().parse().unwrap();
    assert!(landed > n + 1, "other threads ran in between: {w}");
}

#[test]
fn a_blocked_step_stops_at_another_threads_breakpoint_on_the_stepped_thread() {
    // M44 B6(a), replacing M43's R7 fallback row: during a blocked step, another thread's hit ends
    // the step. The stop is named on the STEPPED thread with reason exception (t0 L7's
    // measured-safe form) — M43 measured lldb looping forever (307,016 × `vCont;s:1` in 60 s) when
    // the same hit was reported as `reason:breakpoint` on the other thread.
    let (tr, n, t) = r::threadrust_block();
    let svc = r::trap_pc(tr, n);
    let b = retrace_core::seek(tr, n + 1, 0).unwrap().pc();
    let mut c = Rsp::spawn(tr, &[]);
    assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
    r::continue_to_window(&mut c, n);
    assert_eq!(c.send(&format!("z0,{svc:x},4")), "OK");
    assert_eq!(c.send(&format!("Z0,{b:x},4")), "OK");
    let s = c.send(&format!("vCont;s:{:x}", t + 1));
    assert!(s.contains("reason:exception;"), "{s}");
    assert_eq!(r::key(&s, "thread"), Some(format!("{:x}", t + 1).as_str()), "on the stepped thread: {s}");
    let d = r::description(&s).unwrap();
    assert!(d.contains(&format!("breakpoint at {b:#x}")), "names the other thread's hit: {d}");
    assert!(c.where_().starts_with(&format!("at ({}, 0)", n + 1)), "parked at the hit: {}", c.where_());
}

/// The thread that runs window `w`: the one whose trap ends it.
fn window_thread(trace: &Path, w: usize) -> u32 {
    match retrace_trace::Reader::open(trace).unwrap()[w] {
        retrace_trace::Event::Syscall { thread, .. } => thread,
        _ => panic!("landmark {w} is not a syscall"),
    }
}

#[test]
fn a_blocked_step_stops_at_another_threads_breakpoint_mid_window() {
    // M44 B6(a), Ruling T11-a: the row above's breakpoint sits on the other thread's first
    // instruction, which the until-run's finish reports at the boundary. This one sits three
    // instructions in, so the scan's hardware breakpoint finds it mid-window and resolves it from
    // the scan's own start (`resolve_nth` from kctx = start_k).
    let (tr, n, t) = r::threadrust_block();
    let svc = r::trap_pc(tr, n);
    let b = retrace_core::seek(tr, n + 1, 3).unwrap().pc();
    assert!(b != retrace_core::seek(tr, n + 1, 0).unwrap().pc() && b != svc, "{b:#x} is mid-window");
    let other = window_thread(tr, n + 1);
    let mut c = Rsp::spawn(tr, &[]);
    assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
    r::continue_to_window(&mut c, n);
    assert_eq!(c.send(&format!("z0,{svc:x},4")), "OK");
    assert_eq!(c.send(&format!("Z0,{b:x},4")), "OK");
    let s = c.send(&format!("vCont;s:{:x}", t + 1));
    assert!(s.contains("reason:exception;"), "{s}");
    assert_eq!(r::key(&s, "thread"), Some(format!("{:x}", t + 1).as_str()), "on the stepped thread: {s}");
    assert_eq!(r::description(&s).unwrap(),
        format!("thread {} hit breakpoint at {b:#x} during thread {}'s step", other + 1, t + 1));
    assert!(c.where_().starts_with(&format!("at ({}, 3) phase=Bp", n + 1)), "resolved at k = 3: {}", c.where_());
}

#[test]
fn a_blocked_step_stops_after_another_threads_watched_store_on_the_stepped_thread() {
    // M44 B6(a), Ruling T11-a: a store watch that ends a blocked step is re-parked exactly as
    // `continue`'s is (§3c: reported AFTER the store retires), not left pre-retire, where memory
    // still reads the old value and a step of the writer reports the same store again.
    // watchthread's main blocks in `h.join()`, and only then does the child store to its cell.
    let (tr, n, t, cell) = r::watchthread_block();
    let (w, k) = r::first_store(tr, n + 1, cell); // the child's store is the instruction at (w, k)
    let word = |k| u64::from_le_bytes(retrace_core::seek(tr, w, k).unwrap().read_mem(cell, 8).unwrap().try_into().unwrap());
    let (old, new) = (word(k), word(k + 1));
    assert_ne!(old, new, "the store writes the watched word");
    let child = window_thread(tr, w);
    assert_ne!(child, t, "another thread stores");
    let svc = r::trap_pc(tr, n);
    let mut c = Rsp::spawn(tr, &[]);
    assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
    r::continue_to_window(&mut c, n);
    assert_eq!(c.send(&format!("z0,{svc:x},4")), "OK");
    assert_eq!(c.send(&format!("Z2,{cell:x},8")), "OK");
    let s = c.send(&format!("vCont;s:{:x}", t + 1));
    assert!(s.contains("reason:exception;"), "{s}");
    assert_eq!(r::key(&s, "thread"), Some(format!("{:x}", t + 1).as_str()), "on the stepped thread: {s}");
    assert_eq!(r::description(&s).unwrap(),
        format!("thread {} hit a store to watched {cell:#x} during thread {}'s step", child + 1, t + 1));
    assert_eq!(mem_u64(&mut c, cell), new, "the store retired, as a forward watch stop has it: {s}");
    assert!(c.where_().starts_with(&format!("at ({w}, {}) phase=Bp", k + 1)), "after the store: {}", c.where_());
    // Backward from there: the same store, pre-retire, with the old value in memory.
    let b = c.send("bc");
    assert_eq!(r::key(&b, "watch"), Some(format!("{cell:x}").as_str()), "{b}");
    assert_eq!(mem_u64(&mut c, cell), old, "before the store");
    assert!(c.where_().starts_with(&format!("at ({w}, {k}) phase=Bp")), "{}", c.where_());
}

#[test]
fn a_step_on_a_thread_that_is_not_running_runs_until_it_is_scheduled() {
    // M44 B6(b), M43 T5-a's successor: a step of a live thread that is not running runs until that
    // thread is scheduled, then steps it one instruction — as a blocked step's tail already does.
    let (tr, n, t) = r::threadrust_block();
    let svc = r::trap_pc(tr, n);
    let other = if t == 0 { 1u32 } else { 0 }; // retrace's id of the thread that is not t
    // The oracle, without the server: the first landmark after n where `other` is current, then
    // one instruction of it. Bounded by the recording's length (Ruling P4): a seek that fails
    // yields None, so an unbounded search would never end.
    let len = retrace_trace::Reader::open(tr).unwrap().len();
    let want = (n + 1..len).find_map(|m| {
        let mut s = retrace_core::seek(tr, m, 0).ok()?;
        (s.current_thread() == other).then(|| { s.step_insns(1).unwrap(); s.pc() })
    }).expect("the other thread runs again");
    let mut c = Rsp::spawn(tr, &[]);
    assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
    r::continue_to_window(&mut c, n);
    assert_eq!(c.send(&format!("z0,{svc:x},4")), "OK"); // nothing armed: B6(b) alone
    let s = c.send(&format!("vCont;s:{:x}", other + 1));
    assert!(s.contains("reason:trace;"), "{s}");
    assert_eq!(r::key(&s, "thread"), Some(format!("{:x}", other + 1).as_str()), "{s}");
    assert_eq!(pc_of(&s), want, "one instruction of the other thread, once it runs");
}

#[test]
fn a_step_on_a_thread_that_does_not_exist_is_refused_in_place() {
    // §3d rule 1 survives B6(b) for a thread that is not live: refused, nothing moves.
    let (tr, n, _) = r::threadrust_block();
    let svc = r::trap_pc(tr, n);
    let mut c = Rsp::spawn(tr, &[]);
    assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
    r::continue_to_window(&mut c, n);
    let before = c.where_();
    let s = c.send("vCont;s:63");
    assert!(s.contains("reason:exception;"), "{s}");
    assert!(r::description(&s).unwrap().contains("cannot step thread 99"), "{s}");
    assert_eq!(c.where_(), before);
}

#[test]
fn a_step_across_the_stepped_threads_own_exit_stops_there() {
    // M44 B3: the child's `bsdthread_terminate` is its last trap. Before M44 the step ran on until
    // the child was current again — never — and so to the end of the recording. The stop is named
    // on the thread now running, because the exited one is not in the reply's `threads:` list.
    let (tr, x, child) = r::threadrust_child_exit();
    // The thread running at (x + 1, 0), from the recording: landmark x + 1's own thread tag.
    let running = match retrace_trace::Reader::open(tr).unwrap()[x + 1] {
        retrace_trace::Event::Syscall { thread, .. } => thread,
        _ => panic!("landmark {} is not a syscall", x + 1),
    };
    assert_ne!(running, child);
    let svc = r::trap_pc(tr, x);
    let mut c = Rsp::spawn(tr, &[]);
    assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
    let (_, at) = r::continue_to_window(&mut c, x);
    assert_eq!(r::key(&at, "thread"), Some(format!("{:x}", child + 1).as_str()), "{at}");
    assert_eq!(c.send(&format!("z0,{svc:x},4")), "OK");
    let s = c.send(&format!("vCont;s:{:x}", child + 1));
    assert!(s.contains("reason:exception;"), "not trace, and not the end of the recording: {s}");
    assert!(!s.contains("replaylog:end;"), "{s}");
    assert!(r::description(&s).unwrap().contains(&format!("thread {} exited during the step", child + 1)), "{s}");
    assert_eq!(r::key(&s, "thread"), Some(format!("{:x}", running + 1).as_str()), "on the running thread: {s}");
    // An arrival (Bp), as every step's stop is: a forward `c` from here reports the next hit.
    let w = c.where_();
    assert!(w.starts_with(&format!("at ({}, 0) phase=Bp", x + 1)), "at the exit's boundary, as an arrival: {w}");
}

#[test]
fn every_thread_is_served_its_own_registers_at_a_blocked_stop() {
    // Final review Important 1: lldb's `thread list`, `thread select N; register read` and a `bt` on
    // another thread read a NOT-running thread's registers, by four routes: `thread-pcs` in the stop
    // reply, `p…;thread:N;`, `Hg` + `g`, and `qThreadStopInfo`. At threadrust's blocked window n one
    // thread is current and another is parked with a saved context.
    let (tr, n, _) = r::threadrust_block();
    let svc = r::trap_pc(tr, n);
    let to_window = |c: &mut Rsp| {
        assert_eq!(c.send(&format!("Z0,{svc:x},4")), "OK");
        r::continue_to_window(c, n);
        c.where_()
    };
    // The position, from the server's own `where`. That server is gone before the oracle opens.
    let w = to_window(&mut Rsp::spawn(tr, &[]));
    assert!(w.contains(" phase=Bp "), "an arrival, so a plain seek stands at the same point: {w}");
    let (wn, wk) = w["at (".len()..].split_once(')').unwrap().0.split_once(", ").unwrap();
    let (wn, wk): (usize, u64) = (wn.parse().unwrap(), wk.parse().unwrap());
    // The oracle: `dbg_regs_of` in a fresh session at that position, the current thread's off the
    // vCPU and any other's saved (M15). Read into plain values and dropped before the next server.
    let (cur, oracle) = {
        let s = retrace_core::seek(tr, wn, wk).unwrap();
        let o: Vec<(u32, u64, u64)> = s.thread_summaries().into_iter()
            .filter(|t| !matches!(t.state, retrace_core::ThreadState::Exited(_)))
            .map(|t| {
                let text = s.dbg_regs_of(t.tid as usize).unwrap();
                (t.tid + 1, r::dbg_field(&text, "pc"), r::dbg_field(&text, "sp"))
            }).collect();
        (s.current_thread() + 1, o)
    };
    let cur_pc = oracle.iter().find(|o| o.0 == cur).expect("the current thread is live").1;
    assert!(oracle.iter().any(|o| o.0 != cur), "a stop with a non-current thread, or this row proves nothing: {oracle:x?}");

    let mut c = Rsp::spawn(tr, &[]);
    assert_eq!(to_window(&mut c), w, "the oracle's position");
    let tids: Vec<u32> = c.send("qfThreadInfo")[1..].split(',').map(|t| u32::from_str_radix(t, 16).unwrap()).collect();
    assert_eq!(tids, oracle.iter().map(|o| o.0).collect::<Vec<_>>(), "qfThreadInfo");
    let stop = c.send("?");
    let list = |k: &str| r::key(&stop, k).unwrap_or_else(|| panic!("no {k}: in {stop}")).split(',')
        .map(|v| u64::from_str_radix(v, 16).unwrap()).collect::<Vec<_>>();
    assert_eq!(list("threads"), tids.iter().map(|&t| u64::from(t)).collect::<Vec<_>>(), "{stop}");
    assert_eq!(list("thread-pcs"), oracle.iter().map(|o| o.1).collect::<Vec<_>>(), "in threads: order: {stop}");
    for &(tid, pc, sp) in &oracle {
        assert_eq!(r::le_u64(&c.send(&format!("p20;thread:{tid:x};"))), pc, "tid {tid}'s pc");
        assert_eq!(r::le_u64(&c.send(&format!("p1f;thread:{tid:x};"))), sp, "tid {tid}'s sp");
        assert_eq!(c.send(&format!("Hg{tid:x}")), "OK");
        let g = c.send("g");
        assert_eq!(r::le_u64(&g[32 * 16..33 * 16]), pc, "Hg{tid:x} + g's pc slot");
        assert_eq!(r::le_u64(&g[31 * 16..32 * 16]), sp, "Hg{tid:x} + g's sp slot");
        if tid == cur { continue; }
        // A server that served the running thread's registers for every tid would pass every
        // assertion above only if the two pcs were equal. They are not.
        assert_ne!(pc, cur_pc, "tid {tid} is parked away from the running thread's pc");
        let q = c.send(&format!("qThreadStopInfo{tid:x}"));
        assert!(q.starts_with(&format!("T00thread:{tid:x};")), "{q}");
        assert!(!q.contains("reason:"), "a thread that did not stop has no reason: {q}");
    }
}

#[test]
fn a_reverse_step_moves_back_one_and_stops_at_the_start() {
    // §3e: `bs`, and `bc` armed by `qRcmd arm-rsi` (what `rsi` sends).
    let mut c = Rsp::spawn(watchsweep(), &[]);
    let entry = retrace_core::seek(watchsweep(), 1, 0).unwrap().pc();
    for _ in 0..3 { assert!(c.send("s").contains("reason:trace;")); }
    let b = c.send("bs");
    assert!(b.contains("reason:trace;"), "{b}");
    assert_eq!(pc_of(&b), entry + 8);
    let hexcmd: String = "arm-rsi".bytes().map(|x| format!("{x:02x}")).collect();
    assert_eq!(c.send_collect(&format!("qRcmd,{hexcmd}")).1, "OK");
    let a = c.send("bc");
    assert!(a.contains("reason:trace;"), "an armed bc is one step back: {a}");
    assert_eq!(pc_of(&a), entry + 4);
    // The arming is spent: the very next `bc`, at (1, 1), is a reverse continue, which with nothing
    // armed runs to the start. A still-armed server would answer `trace` at the entry instead.
    let start = c.send("bc");
    assert_eq!(r::description(&start).as_deref(), Some("start of recording"), "unarmed again: {start}");
    assert_eq!(pc_of(&start), entry);
    assert_eq!(c.send("bs"), start, "a reverse step at the start stops there again");
    // Any other resume clears the arming (review Minor 5): `arm-rsi`, `s`, then `bc` is a reverse
    // CONTINUE, to the start, not a step back to entry + 12.
    for _ in 0..3 { assert!(c.send("s").contains("reason:trace;")); }
    assert_eq!(c.send_collect(&format!("qRcmd,{hexcmd}")).1, "OK");
    assert!(c.send("s").contains("reason:trace;"));
    assert_eq!(c.send("bc"), start, "the `s` after `arm-rsi` cleared it");
}

#[test]
fn disarm_rsi_makes_the_next_bc_a_reverse_continue_again() {
    // M44 B2: what `rsi` sends when `ContinueInDirection` fails after `arm-rsi` succeeded. Armed, the
    // next `bc` is one step back; disarmed, it is a reverse continue — here, to the start. Both
    // monitor commands are idempotent: an `E` reply would make `rsi`'s own cleanup fail.
    let mut c = Rsp::spawn(watchsweep(), &[]);
    let entry = retrace_core::seek(watchsweep(), 1, 0).unwrap().pc();
    let cmd = |s: &str| format!("qRcmd,{}", hexs(s.as_bytes()));
    for _ in 0..3 { assert!(c.send("s").contains("reason:trace;")); }
    assert_eq!(c.send_collect(&cmd("disarm-rsi")).1, "OK", "disarming an unarmed server is not an error");
    assert_eq!(c.send_collect(&cmd("arm-rsi")).1, "OK");
    assert_eq!(c.send_collect(&cmd("arm-rsi")).1, "OK", "arming an armed server is not an error");
    assert_eq!(c.send_collect(&cmd("disarm-rsi")).1, "OK");
    let b = c.send("bc");
    assert_eq!(r::description(&b).as_deref(), Some("start of recording"), "disarmed: a reverse continue: {b}");
    assert_eq!(pc_of(&b), entry, "not entry + 8, where an armed bc would have stopped");
}

#[test]
fn a_step_onto_a_watched_store_reports_the_watch_after_it() {
    // §3d rule 3's `Watch` arm, §3c row 2: the store retires, then `watch:`, with the new value in
    // memory. buf[0] is written once, by the sweeping store's first pass at K = 8.
    let buf0 = ws_target() - 320;
    let mut c = Rsp::spawn(watchsweep(), &[]);
    assert_eq!(c.send(&format!("Z2,{buf0:x},8")), "OK");
    for k in 0..8 { let s = c.send("s"); assert!(s.contains("reason:trace;"), "step {k}: {s}"); }
    let w = c.send("s");
    assert_eq!(r::key(&w, "watch"), Some(format!("{buf0:x}").as_str()), "{w}");
    assert_eq!(mem_u64(&mut c, buf0), 0x1111_1111_1111_1111, "post-retire: the new value");
    assert!(c.where_().starts_with("at (1, 9) phase=Bp"), "{}", c.where_());
}

#[test]
fn every_step_packet_form_steps_the_running_thread() {
    // §3h: `S<sig>`, `vCont;S<sig>:<t>`, `Hc` + `s`, `vCont;s` with no thread, and a vCont whose
    // step follows a continue action (lldb's DoResume can order them so, review Minor 2). Each
    // moves the pc by 4, on watchsweep's straight-line start.
    let mut c = Rsp::spawn(watchsweep(), &[]);
    let entry = retrace_core::seek(watchsweep(), 1, 0).unwrap().pc();
    let mut k = 0;
    for p in ["S05", "vCont;S05:1", "Hc1", "s", "vCont;s", "vCont;c:2;s:1"] {
        let r = c.send(p);
        if p == "Hc1" { assert_eq!(r, "OK"); continue; }
        k += 1;
        assert!(r.contains("reason:trace;"), "{p}: {r}");
        assert_eq!(pc_of(&r), entry + 4 * k, "{p}");
    }
    assert!(c.where_().starts_with("at (1, 5) phase=Bp"), "{}", c.where_());
}

#[test]
fn a_step_at_the_end_of_recording_reports_the_end_again() {
    // §3d rule 2: never `trace` at a terminal (t0 L4b's loop).
    let mut c = Rsp::spawn(crashy(), &[]);
    let crash = c.send("c");
    assert_eq!(c.send("vCont;s:1"), crash);
    assert_eq!(c.send("s"), crash);
}
