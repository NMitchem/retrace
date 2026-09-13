// M37 gate. dup2 is modelled: the console is a slot KIND, so an alias of stdout is still mirrored
// into the trace (record == replay stdout), and a console slot displaced by a file is a file
// write on both sides. Asserts on the bytes, never on an exit code (CLAUDE.md's first gate rule).
mod util;

const EXPECT_STDOUT: &[u8] = b"alias\nself=1\nebadf=1\n";
const EXPECT_FILE: &[u8] = b"file18\nfile17\nvia1\n";

// Per-test suffix (review M4): the CLI runs in subprocesses, which HVF's one-VM-per-process rule
// does not serialise, so under a bare `cargo test` two tests sharing one path would race.
fn scratch_file(test: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("retrace-dup2-{}-{test}.txt", std::process::id()))
}

#[test]
fn console_aliases_are_mirrored_and_displaced_console_slots_write_the_file() {
    let path = scratch_file("bytes");
    let _ = std::fs::remove_file(&path);
    let out = util::assert_rung_records_and_replays(retrace_guest::DUP2_DYN, &[path.to_str().unwrap()], EXPECT_STDOUT);
    // The file is written on RECORD only (replay forwards nothing); its bytes are the other half
    // of the model: 18 and 17 were plain duplicates of f, and 1 became one.
    let file = std::fs::read(&path).expect("the fixture created its file on record");
    assert_eq!(file, EXPECT_FILE, "file bytes: {:?}", String::from_utf8_lossy(&file));
    // These two restate the helper's byte-equality with `EXPECT_STDOUT` (they cannot fail once it
    // has passed — review M3) and are kept for the failure MESSAGE: each names the half of the
    // model it stands for, so a future loosening of `EXPECT_STDOUT` still has to answer to both.
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.starts_with("alias\n"), "write(17) after dup2(1, 17) must be a mirrored console write. Got:\n{s}");
    assert!(!s.contains("via1"), "printf after dup2(f, 1) must reach the file, not the console. Got:\n{s}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_trace_carries_dup2_as_a_plain_landmark_returning_the_guest_target() {
    let path = scratch_file("trace");
    let _ = std::fs::remove_file(&path);
    let out = util::assert_rung_records_and_replays(retrace_guest::DUP2_DYN, &[path.to_str().unwrap()], EXPECT_STDOUT);
    let events = retrace_trace::Reader::open(&out.trace).unwrap();
    let mut seen = Vec::new();
    for e in events.iter() {
        if let retrace_trace::Event::Syscall { num, args, ret, err, writes, .. } = e {
            if *num == retrace_arch::SYS_DUP2 {
                assert!(writes.is_empty(), "dup2 writes no guest memory");
                seen.push((args[0], args[1], *ret, *err));
            }
        }
    }
    // (1,17) (f,18) (f,17) (f,f) (40,19)->EBADF (f,1): six landmarks; the successful ones return
    // their TARGET (a guest number < 20 — 17 and 18 are targets), never a host descriptor.
    assert_eq!(seen.len(), 6, "expected six dup2 landmarks, saw {seen:?}");
    for (fd, fd2, ret, err) in &seen {
        if *fd == 40 { assert!(*err && *ret == 9, "dup2(40, 19) is EBADF: {seen:?}"); }
        else { assert!(!*err && *ret == *fd2 && *fd2 < 20, "dup2({fd}, {fd2}) returned {ret}: {seen:?}"); }
    }
    let _ = std::fs::remove_file(&path);
}

// M37 fix round 1 (review I3). The mirror's byte-compare, verified able to fail — on the pattern of
// `retrace-core/tests/fdreplay.rs::a_recorded_host_shaped_fd_is_caught_as_divergence`: a passing
// replay proves nothing about the compare on its own (if it were vacuous every replay would still
// pass, and brief control 2 only showed the TABLE update is load-bearing). Tampering the recorded
// return is what makes the oracle demonstrate it can fail, and keeps demonstrating it.
#[test]
fn a_tampered_dup2_return_is_caught_as_divergence() {
    let path = scratch_file("tamper");
    let _ = std::fs::remove_file(&path);
    let out = util::assert_rung_records_and_replays(retrace_guest::DUP2_DYN, &[path.to_str().unwrap()], EXPECT_STDOUT);
    let mut events = retrace_trace::Reader::open(&out.trace).unwrap();
    let mut tampered = false;
    for e in events.iter_mut() {
        if let retrace_trace::Event::Syscall { num, ret, err, .. } = e {
            if *num == retrace_arch::SYS_DUP2 && !*err && !tampered {
                *ret = 99; // a target the guest never named; the table yields 17
                tampered = true;
            }
        }
    }
    assert!(tampered, "no successful dup2 landmark found to tamper");
    let mut w = retrace_trace::Writer::create(&out.trace).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);

    let rep = util::replay(&out.trace);
    assert_ne!(rep.code, 0, "replay must reject a dup2 return the guest's own table cannot produce. stdout:\n{}",
        String::from_utf8_lossy(&rep.stdout));
    assert!(rep.stderr.contains("dup2 divergence"),
        "the divergence must name the dup2 mismatch, got stderr:\n{}", rep.stderr);
    let _ = std::fs::remove_file(&path);
}
