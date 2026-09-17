// M38 gate. execve/posix_spawn are REFUSED, never forwarded. Two assertions that a forward
// cannot both satisfy: (1) the recorder's stderr carries the refusal line — the only observable
// a forwarded call that happens to EFAULT cannot fake (CLAUDE.md's first gate rule: the errno
// and the empty write set are exactly what the pre-M38 forward produced, so they are the
// CONTINUITY half, asserted second and labelled as such); (2) the trace's events for 59 and 244
// carry the constant and no writes. Repo-owned because the CPython launcher test skips without
// Homebrew Python and so guards nothing on its own.
mod util;

fn expect_stdout() -> Vec<u8> {
    let e = retrace_arch::exec_refusal_errno(retrace_arch::SYS_EXECVE).unwrap();
    let p = retrace_arch::exec_refusal_errno(retrace_arch::SYS_POSIX_SPAWN).unwrap();
    format!("execve={e}\nposix_spawn={p}\n").into_bytes()
}

#[test]
fn exec_is_refused_and_says_so() {
    let (rec, trace) = util::record_dynamic(retrace_guest::EXEC_DYN);
    assert_eq!(rec.code, 0, "the fixture must run to its own exit(0); a replaced image or a crash \
        cannot print anything. stderr:\n{}", rec.stderr);
    // The difference M38 makes.
    assert!(rec.stderr.contains("[retrace] refusing execve") && rec.stderr.contains("[retrace] refusing posix_spawn"),
        "both refusal lines must be on the recorder's stderr; a FORWARDED exec prints none. stderr:\n{}", rec.stderr);
    // Continuity (spec R4): what the guest reads is what the pre-M38 forward returned.
    assert_eq!(rec.stdout, expect_stdout(), "got {:?}", String::from_utf8_lossy(&rec.stdout));
    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "replay: {}", rep.stderr);
    assert_eq!(rep.stdout, rec.stdout);
    let events = retrace_trace::Reader::open(&trace).unwrap();
    let mut seen = Vec::new();
    for e in events.iter() {
        if let retrace_trace::Event::Syscall { num, ret, err, writes, .. } = e {
            if let Some(want) = retrace_arch::exec_refusal_errno(*num) {
                assert!(*err && *ret == want && writes.is_empty(), "syscall {num}: ret {ret} err {err} writes {}", writes.len());
                seen.push(*num);
            }
        }
    }
    assert_eq!(seen, vec![retrace_arch::SYS_EXECVE, retrace_arch::SYS_POSIX_SPAWN], "both exec spellings must be landmarks: {seen:?}");
}
