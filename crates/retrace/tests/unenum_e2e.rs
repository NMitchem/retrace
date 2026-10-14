mod util;
use retrace_guest::UNENUM;

// M33: a syscall with no `arg_kinds` row must not reach the host kernel. The guest issues 8 — the
// kernel's `nosys` slot (`old creat`), the one number that can never legitimately gain a row —
// and the recorder must refuse it BY NAME.
//
// Asserted on the MESSAGE, never the exit code. Under spec §6 control 2 (`forwarded_shape` made
// silent) the call forwards, the kernel answers ENOSYS, the guest ignores it and exits 0, and so
// does `record`: an exit-code assertion would be red for the wrong reason, and "any panic" would
// be green for any panic at all. The message is the difference this milestone makes.
#[test]
fn an_unenumerated_syscall_is_refused_by_name() {
    let (out, _trace) = util::record(UNENUM);
    // M50 §3c (B3): the pre-forward check's line, before the M33 panic could fire. The arch unit test
    // an_unenumerated_syscall_panics_by_name keeps guarding that panic as the backstop. 8 is the
    // kernel's nosys slot, which the SDK does not name.
    assert!(out.stderr.lines().any(|l| l == "retrace: unsupported: syscall 8 - see \
        https://github.com/NMitchem/retrace/blob/main/docs/walls.md#syscalls"),
        "recorder did not refuse syscall 8 by name; code={} stderr=\n{}", out.code, out.stderr);
    assert_eq!(out.code, 4, "a wall exits 4: {}", out.stderr);
}
