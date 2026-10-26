//! M53 t1 (spec §3b, §3g): the rows for the census's filesystem and process calls and their
//! neighbours. Each mode of `posix_dyn` runs natively, is recorded, and is replayed twice: all
//! three must print the same lines, and the lines that matter are pinned by value, so a row that
//! forwarded the wrong thing (a dirfd as a plain number, a path as a pointer) fails here and not by
//! passing quietly. The mode that would wait for a lock (`flockwait`) is the one wall.
mod util;

const WALLS: &str = "https://github.com/NMitchem/retrace/blob/main/docs/walls.md";
const BOUND: u64 = 300;

/// Run `posix_dyn <mode>` natively and under retrace, replay twice, and return the output. Every
/// run exits 0 and prints exactly what native printed; each `pinned` line is in it.
fn as_natively(mode: &str, pinned: &[&str]) -> String {
    let native = util::run_bounded(retrace_guest::POSIX_DYN, &[mode], &[], BOUND);
    assert_eq!(native.code, Some(0), "{mode}: native exits 0. stderr:\n{}", native.stderr);
    let (rec, trace) = util::record_dynamic_args(retrace_guest::POSIX_DYN, &[mode]);
    assert_eq!(rec.code, 0, "{mode}: the recording exits 0. stderr:\n{}", rec.stderr);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), String::from_utf8_lossy(&native.stdout),
        "{mode}: the recording prints what native printed");
    for i in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 0, "{mode}: replay {i} exits 0. stderr:\n{}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "{mode}: replay {i} prints what the recording printed");
    }
    let out = String::from_utf8_lossy(&native.stdout).into_owned();
    for line in pinned { assert!(out.lines().any(|l| l == *line), "{mode}: no line {line:?} in:\n{out}"); }
    assert!(out.lines().last() == Some("removed=yes"), "{mode}: the directory is removed:\n{out}");
    let _ = std::fs::remove_file(&trace);
    out
}

#[test]
fn files_truncate_sync_and_lock_as_natively() {
    as_natively("files", &[
        "ftruncate rc=0 errno=0", "size=6", "truncate rc=0 errno=0", "size=4",
        "fsync rc=0 errno=0", "fsync_nocancel rc=0 errno=0", "fdatasync rc=0 errno=0",
        "fpathconf(_PC_NAME_MAX) rc=255 errno=0",
        "flock(LOCK_EX|LOCK_NB) rc=0 errno=0",
        // The second descriptor of a held file: the lock is refused, and the guest sees it as native does.
        "flock(LOCK_EX|LOCK_NB, other fd, held) rc=-1 errno=35",
        "flock(LOCK_UN) rc=0 errno=0", "flock(bad descriptor) rc=-1 errno=9",
    ]);
}

#[test]
fn links_and_renames_land_as_natively() {
    as_natively("links", &[
        "symlink rc=0 errno=0", "symlinkat rc=0 errno=0", "readlink rc=2 errno=0", "readlinkat rc=2 errno=0",
        "target=f1", "link rc=0 errno=0", "linkat rc=0 errno=0", "linkat(exists) rc=-1 errno=17", "nlink=3",
        "renameat rc=0 errno=0",
        "renameatx_np(RENAME_EXCL, exists) rc=-1 errno=17", "renameatx_np(RENAME_EXCL, fresh) rc=0 errno=0",
        "faccessat(fresh) rc=0 errno=0", "faccessat(hl2, renamed away) rc=-1 errno=2",
    ]);
}

#[test]
fn a_directory_tree_is_made_and_removed_through_a_guest_dirfd() {
    as_natively("dirs", &[
        "mkdirat rc=0 errno=0", "mkdirat(again) rc=-1 errno=17", "subfd_opened=1",
        "mkdirat(inner, on subfd) rc=0 errno=0", "xfd_opened=1", "faccessat(x, on subfd) rc=0 errno=0",
        "faccessat(missing, on subfd) rc=-1 errno=2",
        // A directory without AT_REMOVEDIR is EPERM, a file is not: the flag reaches the kernel.
        "unlinkat(dir without AT_REMOVEDIR) rc=-1 errno=1", "unlinkat(AT_REMOVEDIR) rc=0 errno=0",
        "unlinkat(file) rc=0 errno=0", "faccessat(x, removed) rc=-1 errno=2",
        "rmdir rc=0 errno=0", "rmdir(missing) rc=-1 errno=2",
    ]);
}

#[test]
fn modes_owners_and_flags_change_as_natively() {
    as_natively("modes", &[
        "after chmod mode=0600 flags=0", "after fchmod mode=0640 flags=0", "after fchmodat mode=0644 flags=0",
        "chown(own uid/gid) rc=0 errno=0", "fchown(own uid/gid) rc=0 errno=0",
        "lchown(own uid/gid, symlink) rc=0 errno=0", "fchownat(own uid/gid) rc=0 errno=0",
        "after chflags mode=0644 flags=1", "after fchflags mode=0644 flags=0",
        "mkfifo rc=0 errno=0", "mkfifo(exists) rc=-1 errno=17", "fifo mode=0600 flags=0",
    ]);
    // Who the process is: the group list's size, the process group and session, the priority.
    as_natively("ids", &[
        "groups_count_matches=yes", "pgid_is_pgrp=yes", "sid_ok=yes",
        "getpgid(999999) rc=-1 errno=3", "getpriority(bad which) rc=-1 errno=22",
    ]);
}

#[test]
fn timestamps_set_through_setattrlist_read_back_as_natively() {
    as_natively("times", &[
        "futimes rc=0 errno=0", "after futimes mtime=1000000001",
        "futimens rc=0 errno=0", "after futimens mtime=1000000002",
        "utimensat rc=0 errno=0", "after utimensat mtime=1000000003",
        "setattrlist rc=0 errno=0", "after setattrlist mtime=1000000004",
        "setattrlist(bufferSize 0) rc=-1 errno=22",
    ]);
}

#[test]
fn a_clone_and_the_xattr_calls_answer_as_natively() {
    as_natively("clone", &[
        "clonefile rc=0 errno=0", "clonefile(exists) rc=-1 errno=17", "fclonefileat rc=0 errno=0",
        "cl1 size=16 sum=1481", "cl2 size=16 sum=1481",
    ]);
    as_natively("xattr", &[
        "setxattr rc=0 errno=0", "setxattr(XATTR_CREATE, exists) rc=-1 errno=17",
        // The length query writes nothing and answers the size; a short buffer is ERANGE.
        "getxattr(size query) rc=3 errno=0", "getxattr rc=3 errno=0", "value=abc",
        "getxattr(short buffer) rc=-1 errno=34", "getxattr(missing) rc=-1 errno=93",
        "listxattr has_name=1", "fsetxattr rc=0 errno=0", "fgetxattr rc=3 errno=0", "flistxattr has_name=1",
        "setxattr(name 199 bytes) rc=-1 errno=63", "setxattr(name 127 bytes) rc=0 errno=0",
        "removexattr rc=0 errno=0", "removexattr(missing) rc=-1 errno=93", "fremovexattr rc=0 errno=0",
    ]);
}

#[test]
fn setpriority_and_the_extended_stat_calls_answer_as_natively() {
    // 19 is the maximum nice value, always permitted, so the lines do not depend on the inherited one.
    as_natively("prio", &[
        "setpriority(19) rc=0 errno=0", "getpriority=19",
        "setpriority(bad which) rc=-1 errno=22", "getpriority(bad which) rc=-1 errno=22",
    ]);
    // A NULL xsecurity writes only the struct stat; a buffer also gets the filesec, whose size (44, a
    // file with no ACL) is written back, a short buffer included; the errors are the plain stat's.
    as_natively("ext", &[
        "stat64_extended(NULL xsecurity) rc=0 errno=0 fmt=100000 size=5",
        "stat64_extended(buffer) rc=0 errno=0 fmt=100000 size=5 xsize=44",
        "stat64_extended(short buffer) rc=0 errno=0 fmt=100000 size=5 xsize=44",
        "stat64_extended(missing) rc=-1 errno=2",
        "lstat64_extended(NULL xsecurity) rc=0 errno=0 fmt=120000 size=2",
        "lstat64_extended(buffer) rc=0 errno=0 fmt=120000 size=2 xsize=44",
        "lstat64_extended(missing) rc=-1 errno=2",
        "fstat64_extended(NULL xsecurity) rc=0 errno=0 fmt=100000 size=5",
        "fstat64_extended(buffer) rc=0 errno=0 fmt=100000 size=5 xsize=44",
        "fstat64_extended(bad fd) rc=-1 errno=9",
    ]);
}

#[test]
fn a_mach_port_destruct_answers_as_natively() {
    // Construct, destruct (0), destruct again (KERN_INVALID_NAME, 17): the trap is forwarded, and
    // replay applies the recorded answers.
    as_natively("ports", &[
        "mach_port_construct kr=0 name_nonzero=1", "mach_port_destruct kr=0", "mach_port_destruct(again) kr=17",
    ]);
}

#[test]
fn a_flock_that_would_wait_ends_the_recording_at_one_clean_line() {
    let t = std::env::temp_dir().join(format!("retrace-posix-{}-flockwait.bin", std::process::id()));
    let tp = t.to_str().unwrap();
    // Natively the second descriptor waits for the first and is released after 50 ms.
    let native = util::run_bounded(retrace_guest::POSIX_DYN, &["flockwait"], &[], BOUND);
    assert_eq!(String::from_utf8_lossy(&native.stdout), "locked\nremoved=yes\n", "native waits and then locks");
    let rec = util::run_bounded(util::bin(),
        &["record-dyn", retrace_guest::POSIX_DYN, "-o", tp, "--", "flockwait"], &[], BOUND);
    assert!(!rec.stderr.contains("panicked at"), "{}", rec.stderr);
    let lines: Vec<&str> = rec.stderr.lines().filter(|l| l.starts_with("retrace:")).collect();
    assert_eq!(lines.len(), 1, "exactly one retrace: line. stderr:\n{}", rec.stderr);
    let (head, tail) = ("retrace: unsupported: flock (syscall 131) on fd ",
        &format!(" would wait for a lock another descriptor holds - see {WALLS}#syscalls"));
    assert!(lines[0].starts_with(head) && lines[0].ends_with(tail.as_str()), "{}", lines[0]);
    let fd = &lines[0][head.len()..lines[0].len() - tail.len()];
    assert!(!fd.is_empty() && fd.bytes().all(|b| b.is_ascii_digit()), "the descriptor is a number: {}", lines[0]);
    assert!(rec.stdout.is_empty(), "nothing was printed before the wall");
    assert_eq!(rec.code, Some(4), "a wall exits 4. stderr:\n{}", rec.stderr);
    let rep = util::run_bounded(util::bin(), &["replay", tp], &[], BOUND);
    let rlines: Vec<&str> = rep.stderr.lines().filter(|l| l.starts_with("retrace:")).collect();
    assert_eq!(rlines.len(), 1, "replay's one retrace: line. stderr:\n{}", rep.stderr);
    assert!(rlines[0].starts_with("retrace: incomplete recording: it ends at "), "{}", rlines[0]);
    assert!(!rep.stderr.contains("DIVERGENCE"), "the end of a walled recording is not a divergence");
    assert_eq!(rep.code, Some(4), "replay of a walled recording exits 4. stderr:\n{}", rep.stderr);
    let _ = std::fs::remove_file(&t);
}
