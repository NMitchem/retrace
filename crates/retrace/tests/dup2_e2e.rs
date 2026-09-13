// M37 gate. dup2 is modelled: the console is a slot KIND, so an alias of stdout is still mirrored
// into the trace (record == replay stdout), and a console slot displaced by a file is a file
// write on both sides. Asserts on the bytes, never on an exit code (CLAUDE.md's first gate rule).
mod util;

const EXPECT_STDOUT: &[u8] = b"alias\nself=1\nebadf=1\n";
const EXPECT_FILE: &[u8] = b"file18\nfile17\nvia1\n";

fn scratch_file() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("retrace-dup2-{}.txt", std::process::id()))
}

#[test]
fn console_aliases_are_mirrored_and_displaced_console_slots_write_the_file() {
    let path = scratch_file();
    let _ = std::fs::remove_file(&path);
    let out = util::assert_rung_records_and_replays(retrace_guest::DUP2_DYN, &[path.to_str().unwrap()], EXPECT_STDOUT);
    // The file is written on RECORD only (replay forwards nothing); its bytes are the other half
    // of the model: 18 and 17 were plain duplicates of f, and 1 became one.
    let file = std::fs::read(&path).expect("the fixture created its file on record");
    assert_eq!(file, EXPECT_FILE, "file bytes: {:?}", String::from_utf8_lossy(&file));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.starts_with("alias\n"), "write(17) after dup2(1, 17) must be a mirrored console write. Got:\n{s}");
    assert!(!s.contains("via1"), "printf after dup2(f, 1) must reach the file, not the console. Got:\n{s}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_trace_carries_dup2_as_a_plain_landmark_returning_the_guest_target() {
    let path = scratch_file();
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
    // their TARGET (a guest number < 16), never a host descriptor.
    assert_eq!(seen.len(), 6, "expected six dup2 landmarks, saw {seen:?}");
    for (fd, fd2, ret, err) in &seen {
        if *fd == 40 { assert!(*err && *ret == 9, "dup2(40, 19) is EBADF: {seen:?}"); }
        else { assert!(!*err && *ret == *fd2 && *fd2 < 20, "dup2({fd}, {fd2}) returned {ret}: {seen:?}"); }
    }
    let _ = std::fs::remove_file(&path);
}
