//! `retrace lldb <trace> [--exe <path>] [-- <lldb args>...]` (M56 spec section 3d): start the
//! gdbserver on a free port, start lldb on it with `retrace.py` loaded, and reap the server when
//! lldb ends. Child processes only, never a thread (clippy.toml, spec Ruling 5).
use std::io::Write;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

/// The script that adds `rsi`, embedded so an installed retrace needs no file from the repository.
const RETRACE_PY: &str = include_str!("../lldb/retrace.py");

const NOT_INSTALLED: &str = "lldb is not installed: install the Command Line Tools with xcode-select --install";
const LISTENING: &str = "listening on 127.0.0.1:";
/// T0-3: the bound on `lldb --version`.
const VERSION_BOUND: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(20);

extern "C" {
    fn signal(sig: i32, handler: usize) -> usize;
}
const SIGINT: i32 = 2;
const SIG_IGN: usize = 1;

fn say(line: &str) {
    let _ = writeln!(std::io::stderr(), "{line}");
}

fn code_of(st: ExitStatus) -> i32 {
    st.code().unwrap_or_else(|| 128 + st.signal().unwrap_or(0))
}

/// T0-3. Without developer tools `/usr/bin/lldb` is a shim that opens an installer dialog, so it is
/// never run unless `xcode-select -p` succeeds; then `--version` must exit 0 and print `lldb-...`.
fn lldb_usable() -> Result<PathBuf, String> {
    let path = PathBuf::from(std::env::var_os("RETRACE_LLDB").unwrap_or_else(|| "/usr/bin/lldb".into()));
    let unusable = || Err(NOT_INSTALLED.to_string());
    match Command::new("xcode-select").arg("-p").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status() {
        Ok(s) if s.success() => {}
        _ => return unusable(),
    }
    let Ok(mut child) = Command::new(&path).arg("--version").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()
    else { return unusable() };
    // The version line is tiny, so it fits the pipe and the child never blocks on it.
    let mut waited = Duration::ZERO;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if waited < VERSION_BOUND => { std::thread::sleep(POLL); waited += POLL; }
            _ => { let _ = child.kill(); let _ = child.wait(); return unusable(); }
        }
    };
    let mut out = Vec::new();
    if let Some(mut p) = child.stdout.take() { let _ = std::io::Read::read_to_end(&mut p, &mut out); }
    if status.success() && out.starts_with(b"lldb-") { Ok(path) } else { unusable() }
}

/// The server's stderr, read as it grows. Whole lines only; `done` reads a final partial one.
struct Log { path: PathBuf, seen: usize }

impl Log {
    fn new_lines(&mut self, done: bool) -> Vec<String> {
        let bytes = std::fs::read(&self.path).unwrap_or_default();
        let rest = &bytes[self.seen.min(bytes.len())..];
        let end = if done { rest.len() } else { rest.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1) };
        self.seen += end;
        String::from_utf8_lossy(&rest[..end]).lines().map(str::to_string).collect()
    }
}

fn reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// `a` is the whole argv. Exits with lldb's status; 2 for bad input; the server's code when it ends
/// before it listens.
pub fn run(a: &[String]) -> i32 {
    let usage = "usage: retrace lldb <trace> [--exe <path>] [-- <lldb args>...]";
    let Some(trace) = a.get(2).filter(|t| !t.starts_with('-')) else { return crate::bad_input(usage) };
    let mut i = 3;
    let mut exe = None;
    if a.get(i).map(String::as_str) == Some("--exe") {
        match a.get(i + 1) {
            Some(e) => { exe = Some(e.clone()); i += 2; }
            None => return crate::bad_input(usage),
        }
    }
    let lldb_args: &[String] = match a.get(i).map(String::as_str) {
        None => &[],
        Some("--") => &a[i + 1..],
        Some(_) => return crate::bad_input(usage),
    };
    // Checks come before any child exists.
    if let Err(e) = std::fs::metadata(trace) { return crate::bad_input(&format!("cannot read {trace}: {e}")); }
    if let Err(why) = retrace_core::hypervisor_check() { return crate::no_hypervisor(&why); }
    let lldb = match lldb_usable() { Ok(p) => p, Err(e) => return crate::bad_input(&e) };

    let dir = std::env::temp_dir().join(format!("retrace-lldb-{}", std::process::id()));
    if let Err(e) = std::fs::create_dir_all(&dir) { return crate::bad_input(&format!("cannot create {}: {e}", dir.display())); }
    let code = session(trace, exe.as_deref(), &lldb, lldb_args, &dir);
    let _ = std::fs::remove_dir_all(&dir);
    code
}

fn session(trace: &str, exe: Option<&str>, lldb: &Path, lldb_args: &[String], dir: &Path) -> i32 {
    let Ok(me) = std::env::current_exe() else { return crate::bad_input("cannot find retrace's own path") };
    let log_path = dir.join("server.err");
    let log_file = match std::fs::File::create(&log_path) {
        Ok(f) => f,
        Err(e) => return crate::bad_input(&format!("cannot create {}: {e}", log_path.display())),
    };
    let mut cmd = Command::new(me);
    cmd.args(["gdbserver", trace, "--port", "0"]);
    if let Some(e) = exe { cmd.args(["--exe", e]); }
    // Its own process group, so a ^C aimed at the foreground group reaches lldb and retrace only.
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(log_file).process_group(0);
    let mut server = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return crate::bad_input(&format!("cannot start the gdbserver: {e}")),
    };
    let mut log = Log { path: log_path, seen: 0 };

    // Wait for the port, or for the server to end (an incomplete recording ends it with its line).
    let port = loop {
        let exited = server.try_wait().ok().flatten();
        let mut port = None;
        for l in log.new_lines(exited.is_some()) {
            match l.strip_prefix(LISTENING).and_then(|p| p.trim().parse::<u16>().ok()) {
                Some(p) => port = Some(p),
                None => say(&l),
            }
        }
        if let Some(st) = exited { return code_of(st); }
        if let Some(p) = port { break p; }
        std::thread::sleep(POLL);
    };

    let script = dir.join("retrace.py");
    if let Err(e) = std::fs::write(&script, RETRACE_PY) {
        reap(&mut server);
        return crate::bad_input(&format!("cannot write {}: {e}", script.display()));
    }
    let spawned = Command::new(lldb)
        .arg("-o").arg(format!("gdb-remote 127.0.0.1:{port}"))
        .arg("-o").arg(format!("command script import {}", script.display()))
        .args(lldb_args).spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => { reap(&mut server); return crate::bad_input(&format!("cannot start lldb: {e}")); }
    };
    // Ignored only now: an ignored disposition survives exec, and lldb must keep its own SIGINT.
    // SAFETY: signal(2) with SIG_IGN, restored below.
    let previous = unsafe { signal(SIGINT, SIG_IGN) };
    let status = child.wait();
    // SAFETY: restores the disposition read above.
    unsafe { signal(SIGINT, previous) };

    reap(&mut server);
    for l in log.new_lines(true) { say(&l); }
    match status {
        Ok(st) => code_of(st),
        Err(e) => crate::bad_input(&format!("lldb: {e}")),
    }
}
