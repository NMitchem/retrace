//! M43: `retrace gdbserver <trace>` — a gdb-remote server over one recording, for lldb (spec
//! `docs/superpowers/specs/2026-09-25-retrace-m43-lldb-design.md`). A translation layer, not a
//! second debugger: every motion is one of `Exec`'s, so M41's hit order and M42's pair handling hold
//! under lldb by construction. This file owns the socket, the dispatch, §3c's position mapping and
//! §3d's step rule.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use retrace_core::{ReplaySession, ThreadState, EXE_BASE};
use crate::debug::Exec;
use crate::rsp::{self, Decoder, Frame, StopKind};

/// §3g, spec R10: fixed, so lldb's transcripts are stable (t0 L10), and `os_version` only selects
/// lldb's newer macOS loader (t0 L2).
const QHOSTINFO: &str = "cputype:16777228;cpusubtype:0;ostype:macosx;vendor:apple;endian:little;ptrsize:8;watchpoint_exceptions_received:after;";
const OS_VERSION: &str = "os_version:26.0.0;";
const QPROCESSINFO: &str = "pid:1;parent-pid:1;cputype:100000c;cpusubtype:0;ostype:macosx;vendor:apple;endian:little;ptrsize:8;";
const QSUPPORTED: &str = "PacketSize=20000;QStartNoAckMode+;qXfer:features:read+;QThreadSuffixSupported+;QListThreadsInStopReply+;ReverseContinue+;ReverseStep+";
/// The largest `m` answered, in bytes: half the advertised PacketSize, in hex.
const MAX_READ: usize = 0x10000;

/// Serve one lldb connection on `127.0.0.1:port` (0 picks a free port), then return. The one line
/// on stderr is how a caller learns the port.
///
/// The session is opened BEFORE the port is announced. Opening decodes the whole recording
/// (seconds for CPython), and lldb gives its first packet only its packet timeout. Once the line is
/// printed, every reply is ready.
pub fn serve(trace: &Path, port: u16, exe: Option<String>) -> Result<(), String> {
    let mut srv = Server::new(trace, exe)?;
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("bind 127.0.0.1:{port}: {e}"))?;
    let port = listener.local_addr().map_err(|e| format!("local_addr: {e}"))?.port();
    eprintln!("listening on 127.0.0.1:{port}");
    let (mut sock, _) = listener.accept().map_err(|e| format!("accept: {e}"))?;
    let _ = sock.set_nodelay(true);
    let mut dec = Decoder::default();
    let mut buf = vec![0u8; 64 * 1024];
    let mut last_sent: Vec<u8> = Vec::new();
    let io = |e: std::io::Error| format!("socket: {e}");
    loop {
        let n = sock.read(&mut buf).map_err(io)?;
        if n == 0 { return Ok(()); } // the peer closed without `k` or `D` (Review Focus 3)
        dec.push(&buf[..n]);
        while let Some(frame) = dec.next() {
            match frame {
                Frame::Ack | Frame::Interrupt => {} // §3h: 0x03 outside a motion is ignored
                Frame::Nak => { if !srv.no_ack { sock.write_all(&last_sent).map_err(io)?; } }
                Frame::Bad => { if !srv.no_ack { sock.write_all(b"-").map_err(io)?; } }
                Frame::Packet(p) => {
                    if !srv.no_ack { sock.write_all(b"+").map_err(io)?; }
                    let (replies, close) = srv.handle(&p);
                    for r in replies {
                        last_sent = rsp::encode(r.as_bytes());
                        sock.write_all(&last_sent).map_err(io)?;
                    }
                    if p == "QStartNoAckMode" { srv.no_ack = true; } // after its OK went out acked
                    if close { return Ok(()); }
                }
            }
        }
    }
}

pub(crate) struct Server<'a> {
    ex: Exec<'a>,
    /// §3g: the exe's path when known. With it, lldb gets `os_version` and the image list.
    exe: Option<String>,
    /// §3g: the exe's `mach_header_64` and load commands, read once from the opening snapshot.
    exe_hdr: Vec<u8>,
    no_ack: bool,
    /// The reply to `?`: the last stop reported.
    last_stop: String,
    /// The RSP tid `last_stop` names, for `qThreadStopInfo`.
    last_tid: u32,
    /// `Hg`: the thread register reads without a thread suffix go to (an RSP tid; 0 means current).
    hg: u32,
}

impl<'a> Server<'a> {
    fn new(trace: &'a Path, exe_arg: Option<String>) -> Result<Self, String> {
        let ex = Exec::new(trace)?;
        let s = ex.sess();
        let head = s.read_mem_prefix(EXE_BASE, 32);
        let sizeofcmds = head.get(20..24).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
        let exe_hdr = s.read_mem_prefix(EXE_BASE, 32 + sizeofcmds);
        let exe = exe_arg.or_else(|| argv0(s));
        let mut srv = Server { ex, exe, exe_hdr, no_ack: false, last_stop: String::new(), last_tid: 0, hg: 0 };
        srv.stop(StopKind::HistoryBegin("start of recording".into()), None); // the answer to `?`
        Ok(srv)
    }

    /// `(RSP tid, pc)` for every thread that has not exited, in id order.
    fn live_threads(&self) -> Vec<(u32, u64)> {
        let s = self.ex.sess();
        s.thread_summaries().into_iter()
            .filter(|t| !matches!(t.state, ThreadState::Exited(_)))
            .filter_map(|t| s.thread_ctx(t.tid as usize).map(|c| (t.tid + 1, c.regs.pc)))
            .collect()
    }

    /// §3c: a stop reply for `thread` (retrace's id; the current thread when None), and remember
    /// it as the answer to `?`.
    fn stop(&mut self, kind: StopKind, thread: Option<u32>) -> String {
        let t = thread.unwrap_or_else(|| self.ex.sess().current_thread());
        let ctx = self.ex.sess().thread_ctx(t as usize).expect("a stop names a thread in the table");
        let r = rsp::stop_reply(t + 1, &ctx, &self.live_threads(), &kind);
        self.last_stop = r.clone();
        self.last_tid = t + 1;
        r
    }

    /// Which retrace thread a register read addresses: the packet's `;thread:<tid>;` suffix, else
    /// `Hg`, else the current thread. None for an id that does not exist (Review Focus 4).
    fn reg_thread(&self, suffix: Option<&str>) -> Option<usize> {
        let rsp_tid = match suffix { Some(t) => u32::from_str_radix(t, 16).ok()?, None => self.hg };
        let cur = self.ex.sess().current_thread();
        let t = if rsp_tid == 0 { cur } else { rsp_tid - 1 };
        self.live_threads().iter().any(|&(r, _)| r == t + 1).then_some(t as usize)
    }

    /// One packet's replies, and whether the session ends after them.
    pub(crate) fn handle(&mut self, p: &str) -> (Vec<String>, bool) {
        let one = |s: &str| (vec![s.to_string()], false);
        let (body, suffix) = match p.split_once(";thread:") {
            Some((b, t)) => (b, Some(t.trim_end_matches(';'))),
            None => (p, None),
        };
        match body {
            "QStartNoAckMode" | "QThreadSuffixSupported" | "QListThreadsInStopReply" | "QEnableErrorStrings" => one("OK"),
            "qHostInfo" => one(&match self.exe { Some(_) => format!("{QHOSTINFO}{OS_VERSION}"), None => QHOSTINFO.into() }),
            "qProcessInfo" => one(QPROCESSINFO),
            "vCont?" => one("vCont;c;C;s;S"),
            "?" => (vec![self.last_stop.clone()], false),
            "qC" => one(&format!("QC{:x}", self.ex.sess().current_thread() + 1)),
            "qfThreadInfo" => one(&format!("m{}", self.live_threads().iter()
                .map(|(t, _)| format!("{t:x}")).collect::<Vec<_>>().join(","))),
            "qsThreadInfo" => one("l"),
            "g" => match self.reg_thread(suffix).and_then(|t| self.ex.sess().thread_ctx(t)) {
                Some(ctx) => one(&rsp::all_regs_hex(&ctx)),
                None => one("E01"),
            },
            "k" => (vec!["X09".into()], true),
            // `D;<pid>` is the multiprocess form.
            _ if body == "D" || body.starts_with("D;") => (vec!["OK".into()], true),
            _ if body.starts_with("qSupported") => one(QSUPPORTED),
            _ if body.starts_with("qXfer:features:read:target.xml:") => {
                let range = &body["qXfer:features:read:target.xml:".len()..];
                match range.split_once(',').and_then(|(o, l)| Some((usize::from_str_radix(o, 16).ok()?, usize::from_str_radix(l, 16).ok()?))) {
                    Some((o, l)) => one(&rsp::xfer_chunk(rsp::target_xml().as_bytes(), o, l)),
                    None => one("E01"),
                }
            }
            _ if body.starts_with("qThreadStopInfo") => {
                let Ok(t) = u32::from_str_radix(&body["qThreadStopInfo".len()..], 16) else { return one("E01") };
                if t == self.last_tid { return (vec![self.last_stop.clone()], false); }
                match t.checked_sub(1).and_then(|i| self.ex.sess().thread_ctx(i as usize)) {
                    Some(ctx) if self.live_threads().iter().any(|&(r, _)| r == t) =>
                        one(&rsp::stop_reply(t, &ctx, &self.live_threads(), &StopKind::None)),
                    _ => one("E01"),
                }
            }
            _ if body.starts_with("Hg") || body.starts_with("Hc") => {
                if let Some(t) = body.strip_prefix("Hg") {
                    // `Hg-1` and `Hg0` both mean "any thread": the current one.
                    self.hg = u32::from_str_radix(t, 16).unwrap_or(0);
                }
                one("OK")
            }
            _ if body.starts_with('p') => {
                let Ok(n) = usize::from_str_radix(&body[1..], 16) else { return one("E01") };
                match self.reg_thread(suffix).and_then(|t| self.ex.sess().thread_ctx(t))
                    .and_then(|ctx| rsp::reg_bytes(&ctx, n)) {
                    Some(b) => one(&rsp::hex(&b)),
                    None => one("E01"),
                }
            }
            _ if body.starts_with('m') => {
                let parsed = body[1..].split_once(',').and_then(|(a, l)|
                    Some((u64::from_str_radix(a, 16).ok()?, usize::from_str_radix(l, 16).ok()?)));
                let Some((addr, len)) = parsed else { return one("E01") };
                let bytes = self.ex.sess().read_mem_prefix(addr, len.min(MAX_READ));
                if bytes.is_empty() && len > 0 { one("E08") } else { one(&rsp::hex(&bytes)) }
            }
            // §3h: a recording is read-only. Refusing P also stops expression evaluation from
            // resuming the replay under registers lldb invented (t0 L8).
            _ if body.starts_with('P') || body.starts_with('G') || body.starts_with('M') || body.starts_with('X')
                || body.starts_with("QSaveRegisterState") || body.starts_with("QRestoreRegisterState") => one("E01"),
            _ if body.starts_with("qRcmd,") => self.monitor(&body["qRcmd,".len()..]),
            _ if body.starts_with("jGetLoadedDynamicLibrariesInfos:") => {
                let arg = &body["jGetLoadedDynamicLibrariesInfos:".len()..];
                match &self.exe {
                    None => one(""),
                    Some(_) if arg.is_empty() => one("OK"), // the support probe (t0 L2)
                    Some(path) => {
                        let wants_exe = arg.contains("\"fetch_all_solibs\":true")
                            || arg.contains(&EXE_BASE.to_string());
                        let json = if wants_exe {
                            rsp::image_json(&self.exe_hdr, EXE_BASE, path).unwrap_or_else(|_| r#"{"images":[]}"#.into())
                        } else {
                            r#"{"images":[]}"#.into()
                        };
                        one(&json)
                    }
                }
            }
            _ => one(""), // §3h: every other packet is unsupported, motion included until Task 3
        }
    }

    /// `qRcmd`: `process plugin packet monitor <cmd>` (§3e). Output goes out as `O` packets, then
    /// the final `OK`.
    fn monitor(&mut self, hexcmd: &str) -> (Vec<String>, bool) {
        let cmd = rsp::unhex(hexcmd).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        match cmd.trim() {
            "where" => {
                let (n, k, phase) = self.ex.cursor();
                let s = self.ex.sess();
                let text = format!("at ({n}, {k}) phase={phase:?} pc={:#x} thread={}\n", s.pc(), s.current_thread() + 1);
                (vec![format!("O{}", rsp::hex(text.as_bytes())), "OK".into()], false)
            }
            _ => (vec!["E01".into()], false),
        }
    }
}

/// §3g (t0 R3): `record-dyn`'s argv[0], out of the opening stack: `[sp]` is the main image's
/// mach_header (the KernelArgs `build_start_stack` writes) and `[sp + 16]` points at argv[0]. Only
/// an absolute path is trusted, because the recording's working directory is not recorded. A
/// static guest has no KernelArgs, so `[sp]` is not `EXE_BASE` and this is None.
fn argv0(s: &ReplaySession) -> Option<String> {
    let sp = s.thread_ctx(0)?.regs.sp_el0;
    let word = |a: u64| <[u8; 8]>::try_from(s.read_mem_prefix(a, 8)).ok().map(u64::from_le_bytes);
    if word(sp)? != EXE_BASE { return None; }
    let bytes = s.read_mem_prefix(word(sp + 16)?, 1024);
    let end = bytes.iter().position(|&b| b == 0)?;
    let path = String::from_utf8(bytes[..end].to_vec()).ok()?;
    path.starts_with('/').then_some(path)
}
