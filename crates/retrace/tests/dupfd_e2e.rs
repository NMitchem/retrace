// M38 gate. F_DUPFD is modelled: the returned descriptor is a GUEST number honouring the guest
// minimum (a host dup could never return 10 in a process holding 0-16 open), the file receives
// the bytes written through it, F_DUPFD_CLOEXEC on stdout is a console alias the M9 mirror sees,
// and F_SETFD's int argument is forwarded verbatim. Bytes and trace, never an exit code alone.
mod util;

const EXPECT_STDOUT: &[u8] = b"n=10\nsetfd=0\nalias\n";
const EXPECT_FILE: &[u8] = b"dupfd\n";

// Per-test suffix (dup2_e2e's lesson): the CLI runs in subprocesses, which HVF's one-VM-per-process
// rule does not serialise, so under a bare `cargo test` two tests sharing one path would race.
fn scratch_file(test: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("retrace-dupfd-{}-{test}.txt", std::process::id()))
}

#[test]
fn f_dupfd_honours_the_guest_minimum_and_writes_reach_the_file() {
    let path = scratch_file("bytes");
    let _ = std::fs::remove_file(&path);
    let out = util::assert_rung_records_and_replays(retrace_guest::DUPFD_DYN, &[path.to_str().unwrap()], EXPECT_STDOUT);
    let file = std::fs::read(&path).expect("the fixture created its file on record");
    assert_eq!(file, EXPECT_FILE, "file bytes: {:?}", String::from_utf8_lossy(&file));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.starts_with("n=10\n"), "fcntl(f, F_DUPFD, 10) must return the guest number 10. Got:\n{s}");
    assert!(s.ends_with("alias\n"), "write through the F_DUPFD_CLOEXEC alias of stdout must be a mirrored console write. Got:\n{s}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_trace_carries_f_dupfd_returning_the_guest_slot_and_f_setfd_forwarded_verbatim() {
    let path = scratch_file("trace");
    let _ = std::fs::remove_file(&path);
    let out = util::assert_rung_records_and_replays(retrace_guest::DUPFD_DYN, &[path.to_str().unwrap()], EXPECT_STDOUT);
    let events = retrace_trace::Reader::open(&out.trace).unwrap();
    let (mut dupfds, mut setfds) = (Vec::new(), Vec::new());
    for e in events.iter() {
        if let retrace_trace::Event::Syscall { num, args, ret, err, writes, .. } = e {
            if *num == retrace_arch::SYS_FCNTL || *num == retrace_arch::SYS_FCNTL_NOCANCEL {
                match args[1] {
                    retrace_arch::F_DUPFD | retrace_arch::F_DUPFD_CLOEXEC => {
                        assert!(writes.is_empty(), "F_DUPFD writes no guest memory");
                        dupfds.push((args[0], args[2], *ret, *err));
                    }
                    retrace_arch::F_SETFD => setfds.push((args[0], args[2], *ret, *err)),
                    _ => {}
                }
            }
        }
    }
    // (f, 10) -> 10 and (1, 12) -> 12: the return is the lowest free GUEST slot >= min.
    assert_eq!(dupfds.len(), 2, "expected two F_DUPFD landmarks, saw {dupfds:?}");
    assert!(dupfds.iter().all(|(_, min, ret, err)| !*err && ret == min),
        "each F_DUPFD must return exactly its minimum (nothing that high is open): {dupfds:?}");
    assert!(setfds.iter().any(|(fd, arg, ret, err)| *fd == 10 && *arg == 1 && *ret == 0 && !*err),
        "fcntl(10, F_SETFD, 1) must be forwarded with its int argument verbatim and succeed: {setfds:?}");
    let _ = std::fs::remove_file(&path);
}
