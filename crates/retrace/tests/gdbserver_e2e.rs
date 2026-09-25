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
