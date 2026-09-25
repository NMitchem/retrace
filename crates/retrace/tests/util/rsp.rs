//! M43: a minimal gdb-remote client for `gdbserver_e2e`. It spawns `retrace gdbserver` (the
//! codesigned copy), learns the port from its one stderr line, and exchanges packets. The
//! handshake uses ack mode; everything after `QStartNoAckMode` uses no-ack mode, as lldb does.
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct Rsp { child: Child, stream: TcpStream, buf: Vec<u8> }

/// Seconds a reply may take before the test fails instead of hanging the gate.
const REPLY_BOUND: u64 = 180;

/// Start `retrace gdbserver <trace> --port 0 <extra…>` (the codesigned copy) and wait for its
/// `listening on 127.0.0.1:<port>` line. The server's stderr goes to a file, never a pipe: a pipe
/// nobody reads after the first line would block a server that writes more (`debug_bounded`'s
/// reasoning). It polls with `thread::sleep`, because clippy bans reading a clock. Returns the child,
/// the port, and the stderr file (the caller removes it).
pub fn spawn_server(trace: &Path, extra: &[&str]) -> (Child, u16, PathBuf) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let err_p = std::env::temp_dir().join(format!("retrace-gdbserver-{}-{n}.err", std::process::id()));
    let mut child = Command::new(super::bin())
        .args(["gdbserver", trace.to_str().unwrap(), "--port", "0"]).args(extra)
        .stdin(Stdio::null()).stdout(Stdio::null())
        .stderr(std::fs::File::create(&err_p).expect("create gdbserver stderr file"))
        .spawn().expect("spawn gdbserver");
    for _ in 0..REPLY_BOUND * 20 {
        let s = std::fs::read_to_string(&err_p).unwrap_or_default();
        if let Some(line) = s.split_once("listening on 127.0.0.1:").and_then(|(_, r)| r.split_once('\n')) {
            return (child, line.0.trim().parse().expect("a port number"), err_p);
        }
        if let Some(st) = child.try_wait().unwrap() {
            panic!("gdbserver exited ({st:?}) before listening:\n{s}");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = child.kill();
    panic!("gdbserver never printed its port");
}

fn encode(p: &str) -> Vec<u8> {
    let mut body = Vec::new();
    for &c in p.as_bytes() {
        if matches!(c, b'#' | b'$' | b'}' | b'*') { body.push(b'}'); body.push(c ^ 0x20); } else { body.push(c); }
    }
    let sum = body.iter().fold(0u8, |a, &c| a.wrapping_add(c));
    let mut out = vec![b'$'];
    out.extend(body);
    out.extend(format!("#{sum:02x}").as_bytes());
    out
}

impl Rsp {
    /// Start `retrace gdbserver <trace> --port 0 <extra…>` and connect, completing
    /// `QStartNoAckMode` in ack mode.
    pub fn spawn(trace: &Path, extra: &[&str]) -> Rsp {
        let (child, port, err_p) = spawn_server(trace, extra);
        let _ = std::fs::remove_file(err_p);
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream.set_read_timeout(Some(std::time::Duration::from_secs(REPLY_BOUND))).unwrap();
        let mut r = Rsp { child, stream, buf: Vec::new() };
        r.stream.write_all(&encode("QStartNoAckMode")).unwrap();
        assert_eq!(r.read_byte(), b'+', "the server acks in ack mode");
        assert_eq!(r.read_packet(), "OK");
        r.stream.write_all(b"+").unwrap();
        r
    }

    fn read_byte(&mut self) -> u8 {
        while self.buf.is_empty() {
            let mut b = [0u8; 65536];
            let n = self.stream.read(&mut b).expect("read (a timeout here means the server hung)");
            assert!(n > 0, "the server closed the connection");
            self.buf.extend_from_slice(&b[..n]);
        }
        self.buf.remove(0)
    }

    fn read_packet(&mut self) -> String {
        while self.read_byte() != b'$' {}
        let mut body = Vec::new();
        loop {
            match self.read_byte() {
                b'#' => break,
                b'}' => { let n = self.read_byte(); body.push(n ^ 0x20); }
                c => body.push(c),
            }
        }
        let (_, _) = (self.read_byte(), self.read_byte()); // the checksum; framing is the unit tests' job
        String::from_utf8(body).expect("a UTF-8 reply")
    }

    /// Send one packet and return the one reply.
    pub fn send(&mut self, p: &str) -> String {
        self.stream.write_all(&encode(p)).unwrap();
        self.read_packet()
    }

    /// Send one packet, collect `O` output packets (hex-decoded) until the final reply.
    pub fn send_collect(&mut self, p: &str) -> (String, String) {
        self.stream.write_all(&encode(p)).unwrap();
        let mut out = String::new();
        loop {
            let r = self.read_packet();
            match r.strip_prefix('O') {
                Some(h) if !h.is_empty() && h.len() % 2 == 0 && h.bytes().all(|c| c.is_ascii_hexdigit()) =>
                    out += &String::from_utf8((0..h.len()).step_by(2)
                        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect()).unwrap(),
                _ => return (out, r),
            }
        }
    }

    pub fn send_raw(&mut self, bytes: &[u8]) { self.stream.write_all(bytes).unwrap(); }

    /// `qRcmd,where`'s text, trimmed: `at (n, k) phase=… pc=0x… thread=t`.
    pub fn where_(&mut self) -> String {
        let hexcmd: String = "where".bytes().map(|b| format!("{b:02x}")).collect();
        let (out, fin) = self.send_collect(&format!("qRcmd,{hexcmd}"));
        assert_eq!(fin, "OK");
        out.trim().to_string()
    }

    fn wait_exit(&mut self) -> i32 {
        for _ in 0..REPLY_BOUND * 20 {
            if let Some(st) = self.child.try_wait().unwrap() { return st.code().unwrap_or(-1); }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = self.child.kill();
        panic!("gdbserver did not exit");
    }

    /// `k`: the reply, and the server's exit status.
    pub fn kill(mut self) -> (String, i32) { let r = self.send("k"); (r, self.wait_exit()) }
    /// `D`: the reply, and the server's exit status.
    pub fn detach(mut self) -> (String, i32) { let r = self.send("D"); (r, self.wait_exit()) }
    /// Close the socket without a word (Review Focus 3); the server's exit status.
    pub fn drop_connection(mut self) -> i32 {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        self.wait_exit()
    }
}

impl Drop for Rsp {
    fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); }
}

/// Decode a stop reply's `description:` (hex) field, if it has one.
pub fn description(stop: &str) -> Option<String> {
    let h = stop.split("description:").nth(1)?.split(';').next()?;
    String::from_utf8((0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect()).ok()
}

/// A stop reply's `key:` value (the first one). The reply's first three bytes are `T<sig>`, so the
/// first field reads `thread:<tid>` once they are skipped.
pub fn key<'a>(stop: &'a str, k: &str) -> Option<&'a str> {
    stop.get(3..)?.split(';').find_map(|f| f.strip_prefix(k)?.strip_prefix(':'))
}

/// A register value out of `p`'s little-endian hex.
pub fn le_u64(h: &str) -> u64 {
    let b: Vec<u8> = (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect();
    let mut a = [0u8; 8];
    a[..b.len().min(8)].copy_from_slice(&b[..b.len().min(8)]);
    u64::from_le_bytes(a)
}
