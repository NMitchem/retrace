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
