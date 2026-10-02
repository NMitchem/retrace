//! M47 gate (spec §1 part 1, §3f): git's local workflow, recorded and replayed. Xcode's git, because
//! `/usr/bin/git` is an xcrun shim that reaches M38's posix_spawn refusal. Every test builds a fresh
//! repository NATIVELY first, with the same empty environment the guest runs under (`load_dynamic`
//! pushes an empty envp), so native git and recorded git read the same configuration: none. Every
//! git invocation names its repository with `-C`, which is the chdir (12) row at work (R1).
//!
//! What each asserts is the difference M47 makes, never an exit code alone:
//! - reads: stdout byte-equal to native, and two replays byte-equal to the recording;
//! - writes: the repository state native git reads back afterwards;
//! - commit under default config: the refused maintenance fork (the recorder's line and git's own
//!   error line) and the tree a native twin commit writes. The commit ID differs from the twin's by
//!   design: the guest's timestamps are the host's clock, recorded (spec §2a).
//!
//! Git under retrace used to abort intermittently in libmalloc ("pointer being freed was not
//! allocated": `log -1` about 1 run in 15, `commit` 9 in 20) because retrace ignored
//! `mach_vm_map`'s alignment mask, so xzone's 4 MiB segment landed unaligned. Task 3b fixed it
//! (`docs/sweep-evidence/2026-09-30-m47-abort/`). The deterministic guard is `vmalign_e2e`; these
//! tests exercise the fix end to end, and after it `commit` aborted 0 in 10 and `log -1` 0 in 20
//! (Task 3b's validation). A green run here is as strong as those counts, not stronger.
//!
//! NOT a repo artifact: without Xcode's git each test announces a skip and passes, which is not
//! evidence of anything.
mod util;
use std::path::{Path, PathBuf};
use std::process::Command;

const GIT: &str = "/Applications/Xcode.app/Contents/Developer/usr/bin/git";
const IDENT: [&str; 4] = ["-c", "user.name=retrace", "-c", "user.email=retrace@example.invalid"];
/// t0 M3(c): git's own stderr line when its auto-maintenance fork fails with EAGAIN, natively.
const CANNOT_FORK: &str = "error: cannot fork() for maintenance: Resource temporarily unavailable";

fn have_git(test: &str) -> bool {
    if Path::new(GIT).exists() { return true; }
    util::announce(&format!("SKIPPED {test}: {GIT} not installed (Xcode). \
        This gate did NOT run — it is not evidence of anything."));
    false
}

/// Native git in `repo`, with the guest's empty environment. Asserts success.
fn native(repo: &Path, args: &[&str]) -> Vec<u8> {
    let o = Command::new(GIT).env_clear().arg("-C").arg(repo).args(args).output().unwrap();
    assert!(o.status.success(), "native git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    o.stdout
}

/// A fresh repository on `main` with one commit (`a.txt`), then `a.txt` modified and `b.txt` and
/// `c.txt` untracked, so status, diff and add have something to act on.
fn repo(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("retrace-m47-git-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    native(&d, &["init", "-q", "-b", "main"]);
    std::fs::write(d.join("a.txt"), "one\n").unwrap();
    native(&d, &["add", "a.txt"]);
    let mut a = IDENT.to_vec();
    a.extend(["-c", "maintenance.auto=false", "commit", "-q", "-m", "first"]);
    native(&d, &a);
    std::fs::write(d.join("a.txt"), "one\ntwo\n").unwrap();
    std::fs::write(d.join("b.txt"), "untracked\n").unwrap();
    std::fs::write(d.join("c.txt"), "added\n").unwrap();
    d
}

/// Record git in `repo` with `args`, then replay twice; both replays must match the recording.
fn recorded(repo: &Path, args: &[&str]) -> util::RunOut {
    let mut argv = vec!["-C", repo.to_str().unwrap()];
    argv.extend_from_slice(args);
    let (rec, trace) = util::record_dynamic_args(GIT, &argv);
    assert_eq!(rec.code, 0, "record git {args:?}: {}", rec.stderr);
    for i in 0..2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 0, "replay {i} of git {args:?}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "replay {i} of git {args:?}: stdout differs from the recording");
    }
    let _ = std::fs::remove_file(&trace);
    rec
}

/// t0 M4's in-list read commands (the measurements file, Decisions).
const READS: &[&[&str]] = &[
    &["status", "--porcelain"],
    &["status"],
    &["diff"],
    &["diff", "--cached"],
    &["log", "-1"],
    &["show", "--stat", "HEAD"],
    &["rev-parse", "HEAD"],
];

/// RED at `427fa0a`: the M33 panic at chdir (12). Native runs first on the same repository, so both
/// read the same objects and the same index.
#[test]
fn every_read_command_matches_native_and_replays_identically() {
    if !have_git("every_read_command_matches_native_and_replays_identically") { return; }
    let d = repo("reads");
    for args in READS {
        let want = native(&d, args);
        let rec = recorded(&d, args);
        assert_eq!(String::from_utf8_lossy(&rec.stdout), String::from_utf8_lossy(&want),
            "git {args:?}: the recorded stdout differs from native");
    }
}

/// RED at `427fa0a`: the M33 panic at chdir (12), then mkdir (136) and link (9). `hash-object`
/// without `-w` names the blob without writing it, so the object existing afterwards is the guest's
/// write.
#[test]
fn add_writes_the_blob_native_git_reads_back() {
    if !have_git("add_writes_the_blob_native_git_reads_back") { return; }
    let d = repo("add");
    let blob = String::from_utf8(native(&d, &["hash-object", "c.txt"])).unwrap().trim().to_string();
    recorded(&d, &["add", "c.txt"]);
    assert_eq!(String::from_utf8(native(&d, &["ls-files", "--stage", "c.txt"])).unwrap(), format!("100644 {blob} 0\tc.txt\n"));
    assert_eq!(native(&d, &["cat-file", "-t", &blob]), b"blob\n");
}

/// RED at `427fa0a`: the M33 panic at chdir (12). Past the rows, default config reached the 3403
/// record error. The difference: the refused fork (the recorder's line and git's own line), a clean
/// record, and the tree a native commit of the same index writes.
#[test]
fn commit_under_default_config_refuses_the_maintenance_fork_and_writes_the_native_tree() {
    if !have_git("commit_under_default_config_refuses_the_maintenance_fork_and_writes_the_native_tree") { return; }
    let (d, twin) = (repo("commit"), repo("commit-twin"));
    for r in [&d, &twin] { native(r, &["add", "-A"]); }
    let mut args = IDENT.to_vec();
    args.extend(["commit", "-q", "-m", "second"]);
    let rec = recorded(&d, &args);
    assert!(rec.stderr.contains("[retrace] refusing fork (syscall 2)"), "the recorder's refusal line: {}", rec.stderr);
    // The guest's stderr is merged into the record's stdout (t0 M4; record_box's console arm), so
    // git's own line arrives on stdout, while the recorder's line above is its own eprintln.
    let out = String::from_utf8_lossy(&rec.stdout);
    assert!(out.contains(CANNOT_FORK), "git's own line when its fork fails (t0 M3(c)): {out}");
    let mut twin_args = vec!["-c", "maintenance.auto=false"];
    twin_args.extend(&args);
    native(&twin, &twin_args);
    assert_eq!(native(&d, &["log", "-1", "--format=%T %s"]), native(&twin, &["log", "-1", "--format=%T %s"]),
        "the recorded commit must carry the tree and subject a native commit of the same index writes");
}

#[test]
fn branch_and_tag_point_at_head() {
    if !have_git("branch_and_tag_point_at_head") { return; }
    let d = repo("branch");
    recorded(&d, &["branch", "topic"]);
    recorded(&d, &["tag", "v1"]);
    let head = native(&d, &["rev-parse", "HEAD"]);
    assert_eq!(native(&d, &["rev-parse", "topic"]), head);
    assert_eq!(native(&d, &["rev-parse", "v1^{commit}"]), head);
}

#[test]
fn switch_c_moves_head_to_a_new_branch() {
    if !have_git("switch_c_moves_head_to_a_new_branch") { return; }
    let d = repo("switch");
    recorded(&d, &["switch", "-q", "-c", "topic"]);
    assert_eq!(native(&d, &["symbolic-ref", "HEAD"]), b"refs/heads/topic\n");
}

#[test]
fn mv_renames_the_index_entry_and_the_file() {
    if !have_git("mv_renames_the_index_entry_and_the_file") { return; }
    let d = repo("mv");
    recorded(&d, &["mv", "a.txt", "moved.txt"]);
    assert_eq!(native(&d, &["ls-files", "a.txt", "moved.txt"]), b"moved.txt\n");
    assert!(d.join("moved.txt").exists() && !d.join("a.txt").exists());
}

#[test]
fn rm_cached_drops_the_index_entry_and_keeps_the_file() {
    if !have_git("rm_cached_drops_the_index_entry_and_keeps_the_file") { return; }
    let d = repo("rm");
    recorded(&d, &["rm", "-q", "--cached", "a.txt"]);
    assert_eq!(native(&d, &["ls-files", "a.txt"]), b"");
    assert!(d.join("a.txt").exists());
}
