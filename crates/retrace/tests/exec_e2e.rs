//! M54 (spec §3b-§3e, §3h): `execve` in place. Each mode of `execself_dyn` runs natively, is
//! recorded, is compared with native line by line, and is replayed twice: a state reset both runs get
//! wrong in the same way would pass record against replay, so every assertion is against native, and
//! the lines that matter are pinned by value as well. The refusals are by name: one `retrace:` line,
//! exit 4, and a replay that ends at the incomplete-recording line.
//!
//! Task 3 holds `execve`; tasks 4 and 5 add `posix_spawn(SETEXEC)` and `#!` scripts to this file.
mod util;
use retrace_trace::Event;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const WALLS: &str = "https://github.com/NMitchem/retrace/blob/main/docs/walls.md";
const BOUND: u64 = 300;

/// A path unique to this process and call.
fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!("retrace-exec-{}-{}-{name}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
}

fn cli(args: &[&str]) -> util::Bounded { util::run_bounded(util::bin(), args, &[], BOUND) }

/// `record-dyn guest -- args`, bounded; the trace path comes back too.
fn rec(guest: &str, args: &[&str]) -> (util::Bounded, PathBuf) {
    let t = scratch("t.bin");
    let mut argv = vec!["record-dyn", guest, "-o", t.to_str().unwrap()];
    if !args.is_empty() { argv.push("--"); argv.extend_from_slice(args); }
    (cli(&argv), t)
}

fn lossy(b: &[u8]) -> String { String::from_utf8_lossy(b).into_owned() }

/// Run `execself_dyn args` natively, recorded, and replayed twice. All must exit 0 and print what native
/// printed (stdout, and stderr for the mode that reads it). Returns native's stdout and the trace.
fn as_natively(args: &[&str], stderr_too: bool) -> (String, PathBuf) {
    let native = util::run_bounded(retrace_guest::EXECSELF_DYN, args, &[], BOUND);
    assert_eq!(native.code, Some(0), "{args:?}: native exits 0. stderr:\n{}", native.stderr);
    let (r, t) = rec(retrace_guest::EXECSELF_DYN, args);
    assert_eq!(r.code, Some(0), "{args:?}: the recording exits 0. stderr:\n{}", r.stderr);
    assert_eq!(lossy(&r.stdout), lossy(&native.stdout), "{args:?}: the recording prints what native printed");
    if stderr_too { assert_eq!(r.stderr, native.stderr, "{args:?}: and the same stderr"); }
    for i in 0..2 {
        let rp = cli(&["replay", t.to_str().unwrap()]);
        assert_eq!(rp.code, Some(0), "{args:?}: replay {i} exits 0. stderr:\n{}", rp.stderr);
        assert_eq!(rp.stdout, r.stdout, "{args:?}: replay {i} prints what the recording printed");
        if stderr_too { assert_eq!(rp.stderr, r.stderr, "{args:?}: replay {i}'s stderr"); }
    }
    (lossy(&native.stdout), t)
}

fn has(out: &str, line: &str) { assert!(out.lines().any(|l| l == line), "no line {line:?} in:\n{out}"); }
fn lacks_prefix(out: &str, prefix: &str) { assert!(!out.lines().any(|l| l.starts_with(prefix)), "a line starting {prefix:?} in:\n{out}"); }

fn execs(trace: &Path) -> Vec<Event> {
    retrace_trace::Reader::open(trace).unwrap().into_iter().filter(|e| matches!(e, Event::Exec { .. })).collect()
}

#[test]
fn an_execve_runs_the_new_image_as_natively() {
    let target = retrace_guest::EXEC_TARGET_DYN;
    // Every row of spec 2b's execve column.
    let (out, t) = as_natively(&["execve"], false);
    has(&out, "stage1 fds keep=20 cloexec=21 pipe=22,23 kq=24");
    has(&out, "pid_same=yes");
    has(&out, "tid_changed=yes");
    has(&out, "argv[3]=<a b>"); has(&out, "argv[4]=<>");
    has(&out, "fd 20 open (file) cloexec=0");            // a plain descriptor survives
    has(&out, "fd 22 open (fifo) cloexec=0"); has(&out, "fd 23 open (fifo) cloexec=0"); // both pipe ends
    has(&out, "fd 25 open (file) cloexec=0");                  // dup2(1, 25): open, FD_CLOEXEC clear
    lacks_prefix(&out, "fd 21 ");                         // the O_CLOEXEC file is closed
    lacks_prefix(&out, "fd 24 ");                         // the kqueue is closed
    lacks_prefix(&out, "fd 26 ");                        // dup2(1, 26) + FD_CLOEXEC is closed
    has(&out, "USR1 DFL"); has(&out, "USR2 IGN");        // a handler resets, an ignore stays
    has(&out, "mask HUP=1 INT=0 USR2=0");                // the mask is kept
    has(&out, "pending HUP=1 USR2=0");                   // and so is a masked pending signal, not delivered
    has(&out, "altstack flags=4 size=0");                // SS_DISABLE
    has(&out, "itimer armed=1");                         // ITIMER_REAL is kept
    has(&out, "umask=027"); has(&out, "cwd=/private/tmp"); has(&out, "env X=carried");
    let ev = execs(&t);
    assert_eq!(ev.len(), 1, "one Exec landmark");
    if let Event::Exec { path, image, dylinker, closed, thread, .. } = &ev[0] {
        assert_eq!(path.as_slice(), retrace_guest::EXECSELF_DYN.as_bytes(), "the path as the guest passed it");
        assert!(image.len() > 1000 && dylinker.len() > 1_000_000, "the image and dyld travel in the event");
        // The close set is the host's answer: the O_CLOEXEC file (21) and the FD_CLOEXEC alias (26), and
        // nothing that survives. (The kqueue, 24, is closed by rule whether or not its host fd says so.)
        assert!(closed.contains(&21) && closed.contains(&26), "the O_CLOEXEC file and the FD_CLOEXEC alias: {closed:?}");
        assert!(closed.iter().all(|f| ![0, 1, 2, 20, 22, 23, 25].contains(f)), "no survivor is closed: {closed:?}");
        assert_eq!(*thread, 0);
    } else { unreachable!() }
    let _ = std::fs::remove_file(&t);

    // FD_CLOEXEC on fd 1: its slot closes, 0 and 2 stay. Stage 2 reports on stderr.
    let (_, t) = as_natively(&["cloexec1"], true);
    let _ = std::fs::remove_file(&t);

    // A relative path, resolved against the cwd the guest chose.
    let dir = scratch("rel");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(target, dir.join("exec_target_dyn")).unwrap();
    let (out, t) = as_natively(&["relpath", dir.to_str().unwrap()], false);
    has(&out, "target argc=2"); has(&out, "argv[1]=72656c");
    if let Event::Exec { path, .. } = &execs(&t)[0] { assert_eq!(path.as_slice(), b"./exec_target_dyn"); }
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&t);

    // Bytes that are not UTF-8 cross the exec exactly.
    let (out, t) = as_natively(&["bytes", target], false);
    has(&out, "argv[1]=fffe"); has(&out, "X=80");
    let _ = std::fs::remove_file(&t);

    // A pipe's bytes survive: the target reads what the program wrote before the exec.
    let (out, t) = as_natively(&["pipe", target], false);
    has(&out, "fd 20 fifo"); has(&out, "fd 21 fifo"); has(&out, "pipe=68656c6c6f");
    let _ = std::fs::remove_file(&t);
}

#[test]
fn an_execve_from_a_worker_ends_every_other_thread() {
    // Main blocks in pthread_join; the worker execs. The join never returns, and stage 2 runs normally
    // with a new thread id.
    let (out, t) = as_natively(&["worker"], false);
    has(&out, "tid_changed=yes");
    assert!(!out.contains("join returned"), "main's join does not return:\n{out}");
    let ev = execs(&t);
    if let Event::Exec { thread, .. } = &ev[0] { assert_eq!(*thread, 1, "the caller was the worker, thread 1"); } else { unreachable!() }
    let _ = std::fs::remove_file(&t);

    // A worker blocked in pthread_cond_wait with a signal pending on it: main masks the signal, so the
    // pending signal moves to the new image's only thread and stays pending (Ruling T0-i).
    let (out, t) = as_natively(&["pendmasked"], false);
    has(&out, "pending HUP=0 USR2=1");
    has(&out, "mask HUP=0 INT=0 USR2=1");
    has(&out, "tid_changed=yes");
    let _ = std::fs::remove_file(&t);
}

#[test]
fn a_chain_of_twenty_images_records_and_replays() {
    let (out, t) = as_natively(&["chain", "20"], false);
    assert_eq!(out, "chain done\n");
    assert_eq!(execs(&t).len(), 20, "twenty Exec landmarks");
    let size = std::fs::metadata(&t).unwrap().len();
    let per = size / 20;
    // H6: the spec's halt is 16 MiB an exec. dyld's slice alone is about 1.3 MB.
    assert!(per < 16 * 1024 * 1024, "trace size per exec {per} bytes (total {size}) is past the 16 MiB halt");
    util::announce(&format!("H6: chain 20 trace {size} bytes, {per} bytes per exec"));
    let _ = std::fs::remove_file(&t);
}

#[test]
fn execves_errors_answer_as_natively() {
    let dir = scratch("errs");
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.to_str().unwrap();
    let args = ["errs", d, retrace_guest::EXEC_TARGET_DYN];
    let (out, t) = as_natively(&args, false);
    for line in ["missing errno=2", "noexec errno=13", "directory errno=13", "garbage errno=8", "badpath errno=14",
                 // The file is checked before the argument strings are read (measured natively).
                 "missing_badargv errno=2", "garbage_badargv errno=8",
                 "badargv errno=14", "badargventry errno=14", "badenvp errno=14", "badenventry errno=14"] {
        has(&out, line);
    }
    // A failed exec is an ordinary landmark: nothing replaced the image.
    assert!(execs(&t).is_empty(), "no Exec landmark for a failed exec");
    let _ = std::fs::remove_file(&t);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A bad total is E2BIG as natively; past retrace's start-stack limit but under native's is a wall.
#[test]
fn an_argument_vector_past_native_is_e2big_and_past_retrace_is_a_wall() {
    let target = retrace_guest::EXEC_TARGET_DYN;
    let (out, t) = as_natively(&["e2big", target], false);
    has(&out, "exec failed: errno=7");
    let _ = std::fs::remove_file(&t);
    let line = assert_wall(&["bigargs", target], "execve with ", "process-creation", "");
    assert!(line.contains("past retrace's 128 KiB start-stack limit"), "{line}");
}

/// The recording carries the image, so replay never opens the binary (spec §3l): overwrite it after the
/// recording and the replay still prints what the recording printed; delete it and it still does.
#[test]
fn replay_never_reads_the_new_binary() {
    let dir = scratch("nodisk");
    std::fs::create_dir_all(&dir).unwrap();
    let t_path = dir.join("t");
    std::fs::copy(retrace_guest::EXEC_TARGET_DYN, &t_path).unwrap();
    let tp = t_path.to_str().unwrap();
    let (r, trace) = rec(retrace_guest::EXECSELF_DYN, &["copy", tp]);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    assert!(lossy(&r.stdout).contains("target argc=2"), "the recording ran the target:\n{}", lossy(&r.stdout));
    // Control: the file is now the x86 image, so natively it prints x86. If it did not, the overwrite
    // below would prove nothing.
    std::fs::copy(retrace_guest::EXEC_X86, &t_path).unwrap();
    let native = util::run_bounded(tp, &[], &[], BOUND);
    assert_eq!(lossy(&native.stdout), "x86\n", "control: the overwritten file is a different program");
    let rp = cli(&["replay", trace.to_str().unwrap()]);
    assert_eq!((rp.code, rp.stdout.as_slice()), (Some(0), r.stdout.as_slice()), "replay after the overwrite. stderr:\n{}", rp.stderr);
    std::fs::remove_file(&t_path).unwrap();
    let rp = cli(&["replay", trace.to_str().unwrap()]);
    assert_eq!((rp.code, rp.stdout.as_slice()), (Some(0), r.stdout.as_slice()), "replay after the delete. stderr:\n{}", rp.stderr);
    let _ = std::fs::remove_file(&trace);
    let _ = std::fs::remove_dir_all(&dir);
}

/// One wall: record's output, one `retrace: unsupported: <what>` line linking `#<anchor>`, exit 4, and
/// a replay that ends at the incomplete-recording line. Returns the wall line.
fn assert_wall(args: &[&str], what: &str, anchor: &str, stdout_so_far: &str) -> String {
    let (r, t) = rec(retrace_guest::EXECSELF_DYN, args);
    let lines: Vec<&str> = r.stderr.lines().filter(|l| l.starts_with("retrace:")).collect();
    assert_eq!(lines.len(), 1, "{args:?}: exactly one retrace: line. stderr:\n{}", r.stderr);
    let line = lines[0].to_owned();
    assert!(line.starts_with(&format!("retrace: unsupported: {what}")), "{args:?}: {line}");
    assert!(line.ends_with(&format!(" - see {WALLS}#{anchor}")), "{args:?}: {line}");
    assert!(!r.stderr.contains("panicked at") && !r.stderr.contains("internal error"), "{args:?}: a wall, not a bug:\n{}", r.stderr);
    assert!(lossy(&r.stdout).starts_with(stdout_so_far), "{args:?}: the output before the wall:\n{}", lossy(&r.stdout));
    assert_eq!(r.code, Some(4), "{args:?}: a wall exits 4. stderr:\n{}", r.stderr);
    let rp = cli(&["replay", t.to_str().unwrap()]);
    let rl: Vec<&str> = rp.stderr.lines().filter(|l| l.starts_with("retrace:")).collect();
    assert_eq!(rl.len(), 1, "{args:?}: replay's one retrace: line. stderr:\n{}", rp.stderr);
    assert!(rl[0].starts_with("retrace: incomplete recording: it ends at ") && rl[0].ends_with(&format!(" - see {WALLS}#incomplete-recordings")), "{}", rl[0]);
    assert!(!rp.stderr.contains("DIVERGENCE"), "{}", rp.stderr);
    assert_eq!(rp.code, Some(4), "{args:?}: replay exits 4. stderr:\n{}", rp.stderr);
    assert!(execs(&t).is_empty(), "{args:?}: the refused exec left no Exec landmark");
    let _ = std::fs::remove_file(&t);
    line
}

#[test]
fn an_x86_64_only_image_is_refused_by_name() {
    let line = assert_wall(&["copy", retrace_guest::EXEC_X86], "execve of an x86_64-only image (needs Rosetta)", "process-creation", "");
    assert!(line.is_ascii(), "{line}");
}

#[test]
fn a_static_image_is_refused_by_name() {
    assert_wall(&["copy", retrace_guest::HELLO], "execve of a static image (no LC_LOAD_DYLINKER)", "process-creation", "");
}

#[test]
fn a_setuid_image_is_refused_by_name() {
    use std::os::unix::fs::PermissionsExt;
    let p = scratch("suid");
    std::fs::copy(retrace_guest::EXEC_TARGET_DYN, &p).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o4755)).unwrap();
    assert_wall(&["copy", p.to_str().unwrap()], "execve of a setuid or setgid image", "process-creation", "");
    let _ = std::fs::remove_file(&p);
}

/// Ruling T3-a (interim, until task 4's C5): a signal pending on a worker, which main does not mask,
/// would kill the new image as it starts, natively. retrace refuses it by value. The native control
/// shows the exit the refusal stands for.
#[test]
fn a_pending_signal_the_new_image_would_take_is_refused_by_value() {
    let native = util::run_bounded(retrace_guest::EXECSELF_DYN, &["workersig"], &[], BOUND);
    assert_eq!(native.code, Some(128 + 31), "control: natively SIGUSR2 ends the new image (159)");
    assert_wall(&["workersig"], "execve with pending signal 31 unmasked, which the new image would take at once", "signals", "stage1 kind=workersig\n");
}
