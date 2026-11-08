//! M56 Task 3: `retrace lldb <trace>`, the one-command front door (spec section 3d). Each test runs
//! the signed CLI bounded, and holds every child in a kill-on-drop, so a regression fails instead of
//! hanging. It skips loudly (`util::announce`) when `/usr/bin/lldb` is not usable.
mod util;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

const BOUND: u64 = 120;
const LLDB: &str = "/usr/bin/lldb";
const NOT_INSTALLED: &str =
    "retrace: lldb is not installed: install the Command Line Tools with xcode-select --install";

extern "C" { fn kill(pid: i32, sig: i32) -> i32; }

fn lldb_runs() -> bool {
    Command::new(LLDB).arg("--version").stdin(Stdio::null()).output().ok()
        .is_some_and(|o| o.status.success() && o.stdout.starts_with(b"lldb-"))
}

macro_rules! need_lldb {
    ($name:literal) => {
        if !lldb_runs() {
            util::announce(concat!("SKIPPED ", $name, ": /usr/bin/lldb did not run. This gate did NOT run."));
            return;
        }
    };
}

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!("retrace-lldbcmd-{}-{}-{name}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Copy `crashy` into `dir` and record it there as `./crashy`. Returns the watch address (`&g.ptr`).
fn record_crashy(dir: &Path) -> u64 {
    std::fs::copy(retrace_guest::CRASHY, dir.join("crashy")).unwrap();
    let r = util::run_in(dir, &["record", "./crashy"], &[], BOUND);
    assert_eq!(r.code, Some(139), "record crashy: {}", r.stderr);
    util::discover_crashy_addrs(&dir.join("crashy.retrace")).1
}

/// The args after `--`: `-x -b -s <file>`, the file holding the session's commands.
fn lldb_args(dir: &Path, ptr: u64) -> Vec<String> {
    let f = dir.join("cmds.lldb");
    let cmds = ["process continue".to_string(), format!("watchpoint set expression -w write -s 8 -- {ptr:#x}"),
        "process continue -R".into(), "rsi".into(), r#"script print("END")"#.into()];
    std::fs::write(&f, cmds.join("\n") + "\n").unwrap();
    ["--", "-x", "-b", "-s"].iter().map(|s| s.to_string()).chain([f.display().to_string()]).collect()
}

fn run_lldb(dir: &Path, trace: &str, ptr: u64, env: &[(&str, &str)]) -> util::Bounded {
    let extra = lldb_args(dir, ptr);
    let mut args = vec!["lldb", trace];
    args.extend(extra.iter().map(String::as_str));
    util::run_in(dir, &args, env, BOUND)
}

fn port_of(out: &str) -> Option<u16> {
    let key = "gdb-remote 127.0.0.1:";
    let at = out.find(key)? + key.len();
    out[at..].chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().ok()
}

fn refused(port: u16) -> bool { std::net::TcpStream::connect(("127.0.0.1", port)).is_err() }

#[test]
fn lldb_reverse_debugs_a_relative_path_recording_in_one_command() {
    need_lldb!("lldb_reverse_debugs_a_relative_path_recording_in_one_command");
    let d = scratch("one");
    let ptr = record_crashy(&d);
    let r = run_lldb(&d, "crashy.retrace", ptr, &[]);
    let out = String::from_utf8_lossy(&r.stdout);
    assert_eq!(r.code, Some(0), "stderr:\n{}\nstdout:\n{out}", r.stderr);
    assert!(out.contains("stop reason = watchpoint 1"), "{out}");
    assert!(out.contains("crashy`main"), "symbols by relative path: {out}\n{}", r.stderr);
    assert!(out.lines().any(|l| l.contains("str ")), "a str line: {out}");
    assert!(out.contains("stop reason = trace"), "rsi without a script import: {out}");
    assert!(out.contains("END"), "{out}");
}

#[test]
fn the_server_is_gone_after_lldb_exits() {
    need_lldb!("the_server_is_gone_after_lldb_exits");
    let d = scratch("gone");
    let ptr = record_crashy(&d);
    let r = run_lldb(&d, "crashy.retrace", ptr, &[]);
    let out = String::from_utf8_lossy(&r.stdout);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let port = port_of(&out).expect("the gdb-remote echo");
    assert!(refused(port), "the server must be gone");
}

#[test]
fn a_sigint_to_the_foreground_group_leaves_retrace_to_reap_the_server() {
    need_lldb!("a_sigint_to_the_foreground_group_leaves_retrace_to_reap_the_server");
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    let d = scratch("sigint");
    record_crashy(&d);
    let f = d.join("sleep.lldb");
    // lldb abandons the rest of a command file once it is interrupted, and refuses further commands while
    // the interrupt stands, so the check that needs the server is on the sleeping command's own line and
    // goes through the API: after the sleep it reads 8 bytes of the stack from the stopped process, which
    // is a packet to the server. If the SIGINT had reached the server, nothing answers (spec section 4,
    // "leaves retrace and the server alive"). The marker is built from two halves, because lldb echoes the
    // command's own text to stdout.
    let line = r#"script import time, lldb; exec("try:\n time.sleep(5)\nexcept BaseException: pass"); p = lldb.debugger.GetSelectedTarget().GetProcess(); e = lldb.SBError(); p.ReadMemory(p.GetSelectedThread().GetFrameAtIndex(0).GetSP(), 8, e); print("SERVER-" + ("ANSWERED" if e.Success() else "SILENT " + str(e)))"#;
    std::fs::write(&f, format!("{line}\n")).unwrap();
    let (out_p, err_p) = (d.join("o"), d.join("e"));
    let child = Command::new(util::bin()).current_dir(&d).env_clear()
        .args(["lldb", "crashy.retrace", "--", "-x", "-b", "-s", f.to_str().unwrap()])
        .stdin(Stdio::null()).stdout(std::fs::File::create(&out_p).unwrap()).stderr(std::fs::File::create(&err_p).unwrap())
        .process_group(0).spawn().unwrap();
    let pid = child.id() as i32;
    let mut child = util::rsp::KillOnDrop(child);
    let mut port = None;
    for _ in 0..BOUND * 20 {
        port = port_of(&std::fs::read_to_string(&out_p).unwrap_or_default());
        if port.is_some() { break; }
        if child.0.try_wait().unwrap().is_some() {
            panic!("retrace exited early: {}", std::fs::read_to_string(&err_p).unwrap_or_default());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let port = port.expect("the gdb-remote echo");
    std::thread::sleep(std::time::Duration::from_millis(500));
    // SAFETY: plain kill(2) on our own child's process group.
    unsafe { kill(-pid, 2) };
    let mut st = None;
    for _ in 0..BOUND * 20 {
        if let Some(s) = child.0.try_wait().unwrap() { st = Some(s); break; }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let st = st.expect("retrace exits on its own");
    assert_eq!(st.signal(), None, "retrace must not die of the SIGINT");
    let out = std::fs::read_to_string(&out_p).unwrap_or_default();
    assert!(out.contains("SERVER-ANSWERED"), "the server must have survived the SIGINT to answer a packet:\nstdout:\n{out}\nstderr:\n{}",
        std::fs::read_to_string(&err_p).unwrap_or_default());
    assert!(refused(port), "the server must be reaped");
}

#[test]
fn a_recording_is_found_through_the_trace_directory() {
    need_lldb!("a_recording_is_found_through_the_trace_directory");
    let d = scratch("sub-parent");
    let sub = d.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let ptr = record_crashy(&sub);
    let r = run_lldb(&d, "sub/crashy.retrace", ptr, &[]);
    let out = String::from_utf8_lossy(&r.stdout);
    assert!(out.contains("crashy`main"), "{out}\n{}", r.stderr);
}

#[test]
fn a_rebuilt_program_opens_without_symbols_and_says_why() {
    need_lldb!("a_rebuilt_program_opens_without_symbols_and_says_why");
    let d = scratch("rebuilt");
    let ptr = record_crashy(&d);
    std::fs::copy(retrace_guest::HELLO_DYN, d.join("crashy")).unwrap();
    let r = run_lldb(&d, "crashy.retrace", ptr, &[]);
    let out = String::from_utf8_lossy(&r.stdout);
    assert!(r.stderr.contains("is not the program that was recorded"), "{}", r.stderr);
    assert!(out.contains("stop reason = watchpoint 1"), "{out}\n{}", r.stderr);
    assert!(!out.contains("crashy`main"), "{out}");
}

#[test]
fn an_incomplete_recording_ends_with_the_servers_line_and_starts_no_lldb() {
    let d = scratch("wall");
    let r = util::run_in(&d, &["record", "-o", "w.retrace", retrace_guest::CF_WALLS, "chroot"], &[], BOUND);
    assert_eq!(r.code, Some(4), "{}", r.stderr);
    let marker = d.join("marker");
    let script = d.join("fakelldb.sh");
    std::fs::write(&script, format!("#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo lldb-1; exit 0; fi\ntouch {}\n", marker.display())).unwrap();
    Command::new("chmod").args(["+x", script.to_str().unwrap()]).status().unwrap();
    let r = util::run_in(&d, &["lldb", "w.retrace"], &[("RETRACE_LLDB", script.to_str().unwrap())], BOUND);
    assert_eq!(r.code, Some(5), "{}", r.stderr);
    assert!(r.stderr.contains("GDBSERVER ERROR:"), "{}", r.stderr);
    assert!(!marker.exists(), "lldb must not have been started");
}

#[test]
fn an_unusable_lldb_and_a_missing_trace_are_bad_input() {
    let d = scratch("bad");
    std::fs::write(d.join("x.retrace"), retrace_trace::TRACE_MAGIC).unwrap();
    let r = util::run_in(&d, &["lldb", "x.retrace"], &[("RETRACE_LLDB", "/nonexistent")], BOUND);
    assert_eq!(r.code, Some(2), "{}", r.stderr);
    assert_eq!(r.stderr.trim(), NOT_INSTALLED);
    let r = util::run_in(&d, &["lldb", "missing.retrace"], &[], BOUND);
    assert_eq!(r.code, Some(2), "{}", r.stderr);
    assert!(r.stderr.contains("cannot read missing.retrace"), "{}", r.stderr);
}

/// Spec section 4, "no listening socket, no child left", where reaping is the only thing that can do it:
/// lldb ends without ever connecting, so the server is still in `accept` and holds its port. The port is
/// refused after retrace exits only if retrace killed the server. (After a real lldb session the server
/// has accepted, dropped its listener and exited on EOF by itself, so that case cannot tell.)
#[test]
fn a_server_lldb_never_connected_to_is_reaped() {
    let d = scratch("reap");
    record_crashy(&d);
    let port_file = d.join("port");
    let script = d.join("fakelldb.sh");
    std::fs::write(&script, format!(
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo lldb-1; exit 0; fi\n\
         for a in \"$@\"; do case \"$a\" in \"gdb-remote 127.0.0.1:\"*) echo \"${{a#gdb-remote 127.0.0.1:}}\" > {}; esac; done\nexit 0\n",
        port_file.display())).unwrap();
    Command::new("chmod").args(["+x", script.to_str().unwrap()]).status().unwrap();
    let r = util::run_in(&d, &["lldb", "crashy.retrace"], &[("RETRACE_LLDB", script.to_str().unwrap())], BOUND);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    let port: u16 = std::fs::read_to_string(&port_file).expect("the stand-in saw a gdb-remote argument").trim().parse().unwrap();
    assert!(refused(port), "the server was left listening after retrace lldb exited");
}

/// Final review minor 9: the script path is quoted, so a TMPDIR with a space still imports `retrace.py`
/// (`rsi` exists only through that import).
#[test]
fn a_temp_directory_with_a_space_still_loads_the_script() {
    need_lldb!("a_temp_directory_with_a_space_still_loads_the_script");
    let d = scratch("space");
    let ptr = record_crashy(&d);
    let tmp = d.join("tmp with space");
    std::fs::create_dir_all(&tmp).unwrap();
    let r = run_lldb(&d, "crashy.retrace", ptr, &[("TMPDIR", tmp.to_str().unwrap())]);
    let out = String::from_utf8_lossy(&r.stdout);
    assert_eq!(r.code, Some(0), "stderr:\n{}\nstdout:\n{out}", r.stderr);
    assert!(out.contains("stop reason = trace"), "rsi needs the script import: {out}\n{}", r.stderr);
    let left: Vec<_> = std::fs::read_dir(&tmp).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert!(left.is_empty(), "retrace removes the directory it made: {left:?}");
}
