//! M43: `retrace gdbserver <trace>` — a gdb-remote server over one recording, for lldb (spec
//! `docs/superpowers/specs/2026-09-25-retrace-m43-lldb-design.md`). A translation layer, not a
//! second debugger: every motion is one of `Exec`'s, so M41's hit order and M42's pair handling hold
//! under lldb by construction. This file owns the socket, the dispatch, §3c's position mapping and
//! §3d's step rule.
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use retrace_core::{Outcome, ReplaySession, ThreadState, EXE_BASE};
use crate::debug::{Exec, Halt, Hit, Phase};
use crate::rsp::{self, Decoder, Frame, StopKind};

/// §3g, spec R10: fixed, so lldb's transcripts are stable (t0 L10), and `os_version` only selects
/// lldb's newer macOS loader (t0 L2).
// M44 B5: `addressing_bits:47` — the guest VA width (T0SZ = 17). Without it lldb cannot strip a
// PAC-signed saved LR and an arm64e `bt` stops early (measured: 3 frames, only #0 symbolicated —
// #1 and #2 printed still signed, 0x0024000100000328 and 0x005000010000030c, and f1's caller lost).
const QHOSTINFO: &str = "cputype:16777228;cpusubtype:0;ostype:macosx;vendor:apple;endian:little;ptrsize:8;watchpoint_exceptions_received:after;addressing_bits:47;";
const OS_VERSION: &str = "os_version:26.0.0;";
const QPROCESSINFO: &str = "pid:1;parent-pid:1;cputype:100000c;cpusubtype:0;ostype:macosx;vendor:apple;endian:little;ptrsize:8;";
const QSUPPORTED: &str = "PacketSize=20000;QStartNoAckMode+;qXfer:features:read+;QThreadSuffixSupported+;QListThreadsInStopReply+;ReverseContinue+;ReverseStep+";
/// The largest `m` answered, in bytes: half the advertised PacketSize, in hex.
const MAX_READ: usize = 0x10000;
/// The most of the exe's header and load commands read. `sizeofcmds` comes from guest memory, so a
/// corrupt header must not send the server walking a huge range.
const MAX_HDR: usize = 1 << 20;

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
    let (sock, _) = listener.accept().map_err(|e| format!("accept: {e}"))?;
    // One connection per server (§3a): a second connect is refused, rather than queued unserved.
    drop(listener);
    let _ = sock.set_nodelay(true);
    match session(&mut srv, sock) {
        Ok(()) => Ok(()),
        // Ruling T2-a, §3a: the peer leaving ends the session with status 0, however it leaves. A
        // peer killed with a reply still unread resets the connection instead of closing it
        // (measured: ECONNRESET on the next read); a write after it has gone is a broken pipe.
        Err(e) if matches!(e.kind(), ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
                                     | ErrorKind::BrokenPipe) => Ok(()),
        Err(e) => Err(format!("socket: {e}")),
    }
}

/// The packet loop over one accepted connection, until `k`, `D` or the peer closes.
fn session(srv: &mut Server<'_>, mut sock: TcpStream) -> std::io::Result<()> {
    let mut dec = Decoder::default();
    let mut buf = vec![0u8; 64 * 1024];
    let mut last_sent: Vec<u8> = Vec::new();
    loop {
        let n = match sock.read(&mut buf) {
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            r => r?,
        };
        if n == 0 { return Ok(()); } // the peer closed without `k` or `D` (Review Focus 3)
        dec.push(&buf[..n]);
        while let Some(frame) = dec.next() {
            match frame {
                Frame::Ack | Frame::Interrupt => {} // §3h: 0x03 outside a motion is ignored
                Frame::Nak => { if !srv.no_ack { sock.write_all(&last_sent)?; } }
                Frame::Bad => { if !srv.no_ack { sock.write_all(b"-")?; } }
                Frame::Packet(p) => {
                    if !srv.no_ack { sock.write_all(b"+")?; }
                    let (replies, close) = srv.handle(&p);
                    for r in replies {
                        last_sent = rsp::encode(r.as_bytes());
                        sock.write_all(&last_sent)?;
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
    /// §3g: the exe's one-image JSON, built once from the opening snapshot's `mach_header_64` and
    /// load commands. None when no path is known or that header does not parse, and then lldb gets
    /// neither `os_version` nor an image list. Advertising `os_version` without an image to answer
    /// with is t0 `l2_osver`: the new loader runs and unloads the exe.
    ///
    /// M54 §3i, T6-b: one per epoch. Epoch 0's is built at the start; epoch k's when the server
    /// first stops at the k-th exec (`exec_image_of` at that position: the header from memory at
    /// `EXE_BASE`, `argv0` from the new start stack), because the exec stop is the only way into an
    /// epoch. `--exe` names epoch 0's image only.
    exe_images: Vec<Option<String>>,
    /// The trace file's directory: where a relative image path is tried after the working directory (§3c).
    trace_dir: PathBuf,
    no_ack: bool,
    /// The reply to `?`: the last stop reported.
    last_stop: String,
    /// The RSP tid `last_stop` names, for `qThreadStopInfo`.
    last_tid: u32,
    /// `Hg`: the thread register reads without a thread suffix go to (an RSP tid; 0 means current).
    hg: u32,
    /// `Hc`: the thread a step without a thread operand steps (an RSP tid; 0 means current).
    hc: u32,
    /// §3e: `qRcmd arm-rsi` makes the next resume, if it is a `bc`, a reverse step. That `bc`
    /// spends it, and any other resume clears it.
    rsi_armed: bool,
    /// §3b: why the server has no session, once a failed motion's re-seek has failed too. From
    /// then on `handle_dead` answers every packet.
    dead: Option<String>,
}

impl<'a> Server<'a> {
    fn new(trace: &'a Path, exe_arg: Option<String>) -> Result<Self, String> {
        let ex = Exec::new(trace)?;
        let mut exe_images = vec![None; ex.epoch_count()];
        let trace_dir = match trace.parent() { Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(), _ => PathBuf::from(".") };
        exe_images[0] = exec_image_of(ex.sess(), exe_arg, &trace_dir);
        let mut srv = Server { ex, exe_images, trace_dir, no_ack: false, last_stop: String::new(), last_tid: 0, hg: 0, hc: 0,
                               rsi_armed: false, dead: None };
        srv.stop(StopKind::HistoryBegin("start of recording".into()), None); // the answer to `?`
        Ok(srv)
    }

    /// The image list of the epoch the cursor is in.
    fn exe_image(&self) -> &Option<String> {
        self.exe_images.get(self.ex.epoch_index()).unwrap_or(&None)
    }

    /// M54 §3i: stopped at an exec, so build the new epoch's image list from this position.
    fn exec_stop(&mut self, path: &[u8]) -> String {
        let k = self.ex.epoch_index();
        if let Some(slot) = self.exe_images.get_mut(k) { *slot = exec_image_of(self.ex.sess(), None, &self.trace_dir); }
        self.stop(StopKind::Exec(String::from_utf8_lossy(path).into_owned()), None)
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
        if let Some(why) = &self.dead { return self.handle_dead(body, why); }
        // §3e: `arm-rsi` arms the NEXT resume only if it is a `bc`, which spends it. Any other resume
        // clears it, so a later `c -R` is never turned into a step.
        if is_resume(body) && body != "bc" { self.rsi_armed = false; }
        match body {
            "QStartNoAckMode" | "QThreadSuffixSupported" | "QListThreadsInStopReply" | "QEnableErrorStrings" => one("OK"),
            "qHostInfo" => one(&match self.exe_image() { Some(_) => format!("{QHOSTINFO}{OS_VERSION}"), None => QHOSTINFO.into() }),
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
                // `H?-1` and `H?0` both mean "any thread": the current one.
                let t = u32::from_str_radix(&body[2..], 16).unwrap_or(0);
                if body.starts_with("Hg") { self.hg = t; } else { self.hc = t; }
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
                match self.exe_image() {
                    None => one(""),
                    Some(_) if arg.is_empty() => one("OK"), // the support probe (t0 L2)
                    Some(image) => {
                        let wants_exe = arg.contains("\"fetch_all_solibs\":true")
                            || arg.contains(&EXE_BASE.to_string());
                        one(if wants_exe { image.as_str() } else { r#"{"images":[]}"# })
                    }
                }
            }
            // §3h: the continue and step forms. A signal to deliver is ignored: a recording's signals
            // are its own.
            _ if body == "c" || body.starts_with('C') => (vec![self.resume_forward()], false),
            "s" => (vec![self.step(None)], false),
            _ if body.starts_with('S') => (vec![self.step(None)], false),
            _ if body.starts_with("vCont;") => {
                // A step action ANYWHERE makes it a step (§3d), on the thread the first one names
                // (`s:<tid>`, `S05:<tid>`, or none). lldb may list continue actions first
                // (`vCont;c:2;s:1`). Those are moot: the other threads run as the recording ran
                // them, and a step of a thread that is not running runs them until it is (M44
                // B6(b)). Only a vCont of continue actions alone is a continue.
                let acts: Vec<&str> = body["vCont;".len()..].split(';').collect();
                if let Some(act) = acts.iter().find(|a| a.starts_with(['s', 'S'])) {
                    let tid = act.split_once(':').and_then(|(_, t)| u32::from_str_radix(t, 16).ok());
                    (vec![self.step(tid)], false)
                } else if acts.iter().all(|a| a.starts_with(['c', 'C'])) {
                    (vec![self.resume_forward()], false)
                } else {
                    one("") // an action `vCont?` does not offer
                }
            }
            "bs" => (vec![self.back_step()], false),
            // §3e: an armed `bc` is `rsi`'s reverse step, and spends the arming.
            "bc" => (vec![if std::mem::take(&mut self.rsi_armed) { self.back_step() } else {
                self.motion(|s| { s.refuse_reverse_across_exec()?; let h = s.ex.cmd_reverse_continue(&mut std::io::sink())?; s.reply_backward(h) })
            }], false),
            _ if body.starts_with('Z') || body.starts_with('z') => one(&self.z_packet(body)),
            _ => one(""), // §3h: every other packet is unsupported
        }
    }

    /// §3d: step thread `rsp_tid` (0 or none: the `Hc` thread, else the current one).
    fn step(&mut self, rsp_tid: Option<u32>) -> String {
        let pick = rsp_tid.filter(|&t| t != 0 && t != u32::MAX).or(Some(self.hc).filter(|&t| t != 0 && t != u32::MAX));
        self.motion(|s| {
            let t = match pick { Some(r) => r - 1, None => s.ex.sess().current_thread() };
            match s.ex.step_thread(t, &mut std::io::sink())? {
                Halt::Stepped => Ok(s.stop(StopKind::Trace, None)),
                // Reported on the thread lldb stepped, t0 L7's measured-safe form
                // (`l7_stepfail3_desc`). Named on another thread, lldb-2100 ignores the stop (it
                // suspended that thread for the step) and re-steps forever (Task 4's measurement:
                // 80,103 × `vCont;s:2` in 60 s). Since M44 B6(b), a refused thread that is live is
                // the running one after a trap that returns to itself, and is not running only when
                // the recording ended before it ran (Ruling T12-a): `then_some(t)` names that one.
                // A thread that does not exist, or has exited, is not in the reply's `threads:`
                // list (`live_threads`), so it is not named, though `thread_ctx` may still hold an
                // exited one's stale context (B3's arm below): the running one is named instead.
                Halt::Refused(why) => {
                    let on = s.live_threads().iter().any(|&(r, _)| r == t + 1).then_some(t);
                    Ok(s.stop(StopKind::Exception { signal: 5, text: why }, on))
                }
                Halt::WatchStepped { watched } => Ok(s.stop(StopKind::Watch(watched), None)),
                // M44 B3: the stepped thread has exited, so it is not in the reply's `threads:` list
                // (`live_threads` drops exited threads, though `thread_ctx` still holds its stale
                // context), and a `thread:` outside that list is not sent. Named on the running
                // thread, saying why. lldb-2100 ignores it, since it suspended that thread for the
                // step, and resumes (`lldb_e2e`, Ruling T8-a).
                Halt::ThreadExited { thread } => Ok(s.stop(StopKind::Exception { signal: 5,
                    text: format!("thread {} exited during the step", thread + 1) }, None)),
                // M44 B6(a): named on the stepped thread (t0 L7's measured-safe form), saying which
                // thread hit what. The hit is re-parked first, exactly as `continue`'s is
                // (`forward_hit`, Ruling T11-a), and described from where it now stands.
                Halt::StepInterrupted { thread, by, hit } => {
                    let kind = s.forward_hit(hit)?;
                    let what = match hit {
                        Hit::Break => format!("breakpoint at {:#x}", s.ex.sess().pc()), // not re-parked
                        Hit::Watch { watched } => format!("a store to watched {watched:#x}"),
                        Hit::WatchSys { watched } => format!("a syscall write to watched {watched:#x}"),
                    };
                    let mut text = format!("thread {} hit {what} during thread {}'s step", by + 1, thread + 1);
                    if let StopKind::Exception { text: why, .. } = kind {
                        text = format!("{text}, and {why}"); // the store did not retire
                    }
                    Ok(s.stop(StopKind::Exception { signal: 5, text }, Some(thread)))
                }
                other => s.reply_forward(other),
            }
        })
    }

    /// §3e: one instruction back, on whichever thread ran it.
    fn back_step(&mut self) -> String {
        self.motion(|s| {
            s.refuse_reverse_across_exec()?;
            match s.ex.cmd_reverse_stepi(1, &mut std::io::sink())? {
            Halt::Stepped => Ok(s.stop(StopKind::Trace, None)),
            Halt::AtStart => Ok(s.stop(StopKind::HistoryBegin("start of recording".into()), None)),
            other => Err(format!("reverse-stepi reported {other:?}")),
            }
        })
    }

    /// M54 §3i, T6-e: a reverse resume from an exec stop would leave the new epoch for the old one.
    /// Measured (`lldb_e2e`, `docs/…/t6/lldb-reverse.txt`): lldb-2100 re-reads its module list at a
    /// `reason:exec` stop and at no other, so a history stop in the old epoch would be read against the
    /// NEW image's modules and symbolised wrongly. So it is refused by value, through `motion`'s
    /// non-moving exception stop, and the position stays on the exec stop. The reverse stop AT the
    /// exec is still reported (`reply_backward`'s `Halt::Exec`).
    fn refuse_reverse_across_exec(&self) -> Result<(), String> {
        if self.ex.at_exec_stop() {
            return Err("reverse execution across an exec is not supported".into());
        }
        Ok(())
    }

    /// §3b: one motion, bracketed. An `Err` re-seeks the saved cursor and becomes a non-moving
    /// exception stop: never an `E`, which drops lldb's connection (t0 L7). If the re-seek fails as
    /// well, the server has no session left, and `handle_dead` answers from then on.
    fn motion(&mut self, f: impl FnOnce(&mut Self) -> Result<String, String>) -> String {
        let at = self.ex.cursor();
        match f(self) {
            Ok(reply) => reply,
            Err(e) => match self.ex.recover(at) {
                Ok(()) => self.stop(StopKind::Exception { signal: 5, text: e }, None),
                Err(e2) => {
                    let d = format!("{e}; and re-seeking the cursor failed: {e2}");
                    let r = dead_stop(&d);
                    self.dead = Some(d);
                    self.last_stop = r.clone();
                    r
                }
            },
        }
    }

    /// §3h: `continue`, forward, in any of its forms.
    fn resume_forward(&mut self) -> String {
        self.motion(|s| { let h = s.ex.cmd_continue(&mut std::io::sink())?; s.reply_forward(h) })
    }

    /// §3c: forward. Each hit is re-parked and reported through `forward_hit`.
    fn reply_forward(&mut self, h: Halt) -> Result<String, String> {
        Ok(match h {
            Halt::Break => { let kind = self.forward_hit(Hit::Break)?; self.stop(kind, None) }
            Halt::Watch { watched } => { let kind = self.forward_hit(Hit::Watch { watched })?; self.stop(kind, None) }
            Halt::WatchSys { watched, thread } => {
                let kind = self.forward_hit(Hit::WatchSys { watched })?;
                self.stop(kind, Some(thread))
            }
            Halt::Terminal(r) => self.terminal(&r.outcome),
            Halt::Exec { path } => self.exec_stop(&path),
            other => return Err(format!("a forward motion reported {other:?}")),
        })
    }

    /// §3c's forward report of a hit, with its re-park, which M43 R2 gives the server rather than
    /// `Exec`. A store watch is reported AFTER the store retires, so the server steps it. A syscall
    /// write becomes an arrival at (n, 0), so a reverse `c` finds it again (the forward
    /// syscall-watch row). Both forward paths report through here, `continue`'s stop and a blocked
    /// step that another thread's hit ended (M44 B6(a)), so the two park identically by
    /// construction (Ruling T11-a).
    fn forward_hit(&mut self, hit: Hit) -> Result<StopKind, String> {
        Ok(match hit {
            Hit::Break => StopKind::Breakpoint,
            Hit::Watch { watched } => match self.ex.cmd_stepi(1, &mut std::io::sink())? {
                Halt::Stepped => StopKind::Watch(watched),
                // The store did not retire (it faults). Say so, at the store; the cursor stays ON its
                // watch, so the next `c` crosses to the fault through Exec's own finish.
                Halt::Refused(why) => StopKind::Exception { signal: 5,
                    text: format!("the watched store at {:#x} did not retire: {why}", self.ex.sess().pc()) },
                other => return Err(format!("stepping a watched store reported {other:?}")),
            },
            Hit::WatchSys { watched } => { self.ex.set_phase(Phase::Bp); StopKind::Watch(watched) }
        })
    }

    /// §3c: backward. Every stop is reported BEFORE its crossing.
    fn reply_backward(&mut self, h: Halt) -> Result<String, String> {
        Ok(match h {
            Halt::Break => self.stop(StopKind::Breakpoint, None),
            Halt::Watch { watched } => {
                // Before the store, as an arrival: a forward `c` re-reports it.
                self.ex.set_phase(Phase::Bp);
                self.stop(StopKind::Watch(watched), None)
            }
            Halt::WatchSys { watched, .. } => {
                // Before the syscall: its trap, with the old value in memory.
                let (n, _, _) = self.ex.cursor();
                self.ex.park_before_event(n)?;
                self.stop(StopKind::Watch(watched), None)
            }
            // M54 §3i: the exec stop, reported at the exec's own position, the new image's entry.
            Halt::Exec { path } => self.exec_stop(&path),
            Halt::NoEarlierHit => {
                self.ex.recover((1, 0, Phase::Bp))?;
                self.stop(StopKind::HistoryBegin("start of recording".into()), None)
            }
            other => return Err(format!("a backward motion reported {other:?}")),
        })
    }

    /// §3c's terminal list (t0 L4d, L4e), on the current thread.
    fn terminal(&mut self, o: &Outcome) -> String {
        // M51 §3b (T0-k): a misaligned SP's stop shows the SP, which only the thread's context has.
        let t = self.ex.sess().current_thread();
        let sp = self.ex.sess().thread_ctx(t as usize).expect("a stop names a thread in the table").regs.sp_el0;
        self.stop(rsp::terminal_kind(o, sp), None)
    }

    /// §3f. `Z<t>,<addr>,<kind|len>` / `z…`. 0 and 1 are hardware breakpoints, capped at the
    /// hardware's 6, which is `cmd_break`'s own limit (spec R4, amended at Task 3's review). lldb's
    /// transient step breakpoint (t0 L5) is an ordinary `Z0` that cannot be told from a user's, so no
    /// cap can keep it a slot: when all 6 are the user's, a step-over/out/in's transient is refused
    /// and lldb runs on to the next stop. 2 is a write watch, capped at 4. 3 and 4 (read, access) are
    /// refused. Re-inserting what is there, or removing what is not, is `OK` (Review Focus 5).
    fn z_packet(&mut self, body: &str) -> String {
        let insert = body.starts_with('Z');
        let mut f = body[1..].split(',');
        let (Some(t), Some(a), Some(l)) = (f.next(), f.next(), f.next()) else { return "E01".into() };
        let (Ok(addr), Ok(len)) = (u64::from_str_radix(a, 16), u64::from_str_radix(l, 16)) else { return "E01".into() };
        let sink = &mut std::io::sink();
        match (t, insert) {
            // `cmd_break` keeps an armed address once (a re-insert is `Ok` and takes no slot) and
            // refuses a seventh.
            ("0" | "1", true) => {
                if self.ex.cmd_break(addr, sink).is_ok() { "OK" } else { "E01" }.into()
            }
            ("0" | "1", false) => { let _ = self.ex.cmd_delete(addr, sink); "OK".into() }
            ("2", true) => {
                if self.ex.watches().any(|(wa, wl)| wa == addr && wl == len) { return "OK".into(); }
                if !matches!(len, 1 | 2 | 4 | 8) || addr % len != 0 { return "E01".into(); }
                if self.ex.cmd_watch(addr, len, None, sink).is_ok() { "OK" } else { "E01" }.into()
            }
            ("2", false) => { let _ = self.ex.cmd_unwatch(addr, sink); "OK".into() }
            (_, true) => "E01".into(),
            (_, false) => "OK".into(),
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
            // §3e: what `rsi` sends before `process continue -R` (t0 L3, `l3_rsi`).
            "arm-rsi" => { self.rsi_armed = true; (vec!["OK".into()], false) }
            // M44 B2: what `rsi` sends when `ContinueInDirection` fails after `arm-rsi` succeeded,
            // so the next `process continue -R` is a reverse continue again, not one step back.
            // Idempotent, like `arm-rsi`.
            "disarm-rsi" => { self.rsi_armed = false; (vec!["OK".into()], false) }
            _ => (vec!["E01".into()], false),
        }
    }
}

/// A stop that needs no session: `T05` on RSP thread 1, with the reason.
fn dead_stop(d: &str) -> String {
    format!("T05thread:1;reason:exception;description:{};", rsp::hex(d.as_bytes()))
}

/// Every packet that resumes the guest (§3h's resume forms).
fn is_resume(body: &str) -> bool {
    matches!(body, "c" | "s" | "bc" | "bs") || body.starts_with("vCont;") || body.starts_with('C') || body.starts_with('S')
}

impl Server<'_> {
    /// §3b: after a failed re-seek there is no session. Every resume repeats why, `?` repeats the
    /// last stop, `k` and `D` still end the session, and everything else is refused.
    fn handle_dead(&self, body: &str, why: &str) -> (Vec<String>, bool) {
        match body {
            "k" => (vec!["X09".into()], true),
            _ if body == "D" || body.starts_with("D;") => (vec!["OK".into()], true),
            "?" => (vec![self.last_stop.clone()], false),
            _ if is_resume(body) => (vec![dead_stop(why)], false),
            _ => (vec!["E01".into()], false),
        }
    }
}

/// §3g: the one-image JSON for the executable mapped at `EXE_BASE` in `s`: its header and load
/// commands read from memory, named by `exe_arg` or else by the start stack's `argv[0]`.
///
/// M56 §3c: a candidate path is trusted when the file it names has the image's `LC_UUID` (`resolve_image`). When
/// no candidate is trusted, the one line saying why goes to stderr, and the server sends no image. Epoch 0's is
/// printed by `Server::new`, before `listening on`.
fn exec_image_of(s: &ReplaySession, exe_arg: Option<String>, trace_dir: &Path) -> Option<String> {
    let head = s.read_mem_prefix(EXE_BASE, 32);
    let sizeofcmds = head.get(20..24).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
    let exe_hdr = s.read_mem_prefix(EXE_BASE, 32usize.saturating_add(sizeofcmds).min(MAX_HDR));
    let want = retrace_guest::macho_uuid(&exe_hdr);
    // With no recorded UUID, today's rule: `--exe` is trusted as given, and anything else only when absolute.
    if want.is_none() {
        if let Some(p) = &exe_arg { return rsp::image_json(&exe_hdr, EXE_BASE, p).ok(); }
    }
    let cands: Vec<String> = exe_arg.into_iter().chain(exec_path(s)).chain(argv0(s)).collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    match resolve_image(&cands, &cwd, trace_dir, want) {
        Ok(path) => rsp::image_json(&exe_hdr, EXE_BASE, &path).ok(),
        Err(line) => {
            if !line.is_empty() { eprintln!("{line}"); }
            None
        }
    }
}

/// M56 §3c: the first of `cands` (in order) whose file has the recorded `LC_UUID`. A relative candidate is
/// tried against `cwd`, then `trace_dir`; the trusted path is returned as it was found. `Err` is the one stderr
/// line for the first candidate, or empty when there is no candidate (a static guest with no path is not news).
fn resolve_image(cands: &[String], cwd: &Path, trace_dir: &Path, want: Option<[u8; 16]>) -> Result<String, String> {
    let Some(first) = cands.first() else { return Err(String::new()) };
    let Some(want) = want else {
        // Measured (t0 §5, Ruling T0-5): 0 of 42 `firstcontact` fixtures lack an LC_UUID, and it matched the
        // file's in all 44 programs probed, so this branch is a rare fallback to the pre-M56 rule.
        return match cands.iter().find(|c| c.starts_with('/')) {
            Some(c) => Ok(c.clone()),
            None => Err("retrace: the recorded program has no LC_UUID - symbols are loaded only from an \
                         absolute path; pass --exe <path> to name the right file".into()),
        };
    };
    let mut first_found = false;
    for (i, cand) in cands.iter().enumerate() {
        let places: Vec<PathBuf> = if cand.starts_with('/') { vec![PathBuf::from(cand)] }
            else { vec![cwd.join(cand), trace_dir.join(cand)] };
        for p in places {
            let Ok(bytes) = std::fs::read(&p) else { continue };
            if i == 0 { first_found = true; }
            if retrace_guest::file_uuid(&bytes) == Some(want) { return Ok(p.to_string_lossy().into_owned()); }
        }
    }
    Err(if first_found {
        format!("retrace: {first} is not the program that was recorded (its UUID differs) - symbols are not loaded; \
                 pass --exe <path> to name the right file")
    } else {
        format!("retrace: {first} was not found - symbols are not loaded; pass --exe <path> to name the right file")
    })
}

/// Final review part 2, M1 (amends Ruling T6-b): the path the kernel used, `executable_path=` (apple[0]) on the same start
/// stack, which a program's own `argv[0]` (a bare name from `Command::exec`, or a launcher's own) need not name. It is
/// the path as the exec call wrote it (for a script, the interpreter as written), and `argv[0]` for epoch 0 by C3.
/// Read as `argv0` reads argv, past envp's NULL: `[sp + 8]` is argc, argv follows, then envp, then apple. Only an
/// absolute path is trusted, for the same reason as there.
fn exec_path(s: &ReplaySession) -> Option<String> {
    let sp = s.thread_ctx(0)?.regs.sp_el0;
    let word = |a: u64| <[u8; 8]>::try_from(s.read_mem_prefix(a, 8)).ok().map(u64::from_le_bytes);
    if word(sp)? != EXE_BASE { return None; }
    let argc = word(sp + 8)?;
    if argc > 1 << 16 { return None; }
    // argv's `argc` entries and its NULL, then envp up to its NULL, and apple[0] is the next word.
    let mut at = sp + 16 + 8 * (argc + 1);
    for _ in 0..(1 << 16) {
        if word(at)? == 0 { break; }
        at += 8;
    }
    let bytes = s.read_mem_prefix(word(at + 8)?, 1024);
    let end = bytes.iter().position(|&b| b == 0)?;
    let path = String::from_utf8(bytes[..end].to_vec()).ok()?.strip_prefix("executable_path=")?.to_owned();
    Some(path)
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
    String::from_utf8(bytes[..end].to_vec()).ok()
}

#[cfg(test)]
mod image_tests {
    use super::resolve_image;
    use std::path::{Path, PathBuf};

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("retrace-image-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn want() -> Option<[u8; 16]> { retrace_guest::file_uuid(&std::fs::read(retrace_guest::CRASHY).unwrap()) }
    fn s(p: &Path) -> String { p.to_str().unwrap().to_owned() }

    #[test]
    fn a_relative_candidate_with_the_recorded_uuid_is_trusted() {
        let (cwd, other) = (tmp("rel-cwd"), tmp("rel-other"));
        std::fs::copy(retrace_guest::CRASHY, cwd.join("crashy")).unwrap();
        assert_eq!(resolve_image(&["./crashy".into()], &cwd, &other, want()), Ok(s(&cwd.join("./crashy"))));
    }

    #[test]
    fn the_trace_directory_is_tried_after_the_working_directory() {
        let (cwd, tdir) = (tmp("td-cwd"), tmp("td-trace"));
        std::fs::copy(retrace_guest::CRASHY, tdir.join("crashy")).unwrap();
        assert_eq!(resolve_image(&["crashy".into()], &cwd, &tdir, want()), Ok(s(&tdir.join("crashy"))));
        // A wrong file in the working directory does not stop the search.
        std::fs::copy(retrace_guest::HELLO_DYN, cwd.join("crashy")).unwrap();
        assert_eq!(resolve_image(&["crashy".into()], &cwd, &tdir, want()), Ok(s(&tdir.join("crashy"))));
        // The working directory wins when both match.
        std::fs::copy(retrace_guest::CRASHY, cwd.join("crashy")).unwrap();
        assert_eq!(resolve_image(&["crashy".into()], &cwd, &tdir, want()), Ok(s(&cwd.join("crashy"))));
    }

    #[test]
    fn a_rebuilt_binary_at_the_recorded_path_is_refused_with_the_uuid_line() {
        let d = tmp("rebuilt");
        let p = s(&d.join("crashy"));
        std::fs::copy(retrace_guest::HELLO_DYN, &p).unwrap();
        assert_eq!(resolve_image(std::slice::from_ref(&p), &d, &d, want()), Err(format!(
            "retrace: {p} is not the program that was recorded (its UUID differs) - symbols are not loaded; \
             pass --exe <path> to name the right file")));
    }

    #[test]
    fn a_missing_file_gives_the_not_found_line() {
        let d = tmp("missing");
        assert_eq!(resolve_image(&["./nope".into()], &d, &d, want()), Err(
            "retrace: ./nope was not found - symbols are not loaded; pass --exe <path> to name the right file".into()));
    }

    #[test]
    fn exe_naming_the_right_file_wins() {
        let d = tmp("exe");
        let (good, bad) = (s(&d.join("good")), s(&d.join("bad")));
        std::fs::copy(retrace_guest::CRASHY, &good).unwrap();
        std::fs::copy(retrace_guest::HELLO_DYN, &bad).unwrap();
        assert_eq!(resolve_image(&[good.clone(), bad.clone()], &d, &d, want()), Ok(good.clone()));
        assert_eq!(resolve_image(&[bad, good.clone()], &d, &d, want()), Ok(good));
    }

    #[test]
    fn without_a_recorded_uuid_only_an_absolute_path_is_trusted() {
        let d = tmp("nouuid");
        // No file is read: the absolute path need not exist.
        assert_eq!(resolve_image(&["/no/such/prog".into()], &d, &d, None), Ok("/no/such/prog".into()));
        std::fs::copy(retrace_guest::CRASHY, d.join("crashy")).unwrap();
        assert_eq!(resolve_image(&["crashy".into()], &d, &d, None), Err(
            "retrace: the recorded program has no LC_UUID - symbols are loaded only from an absolute path; \
             pass --exe <path> to name the right file".into()));
    }
}
