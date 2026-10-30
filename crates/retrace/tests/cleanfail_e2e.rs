//! M50 (spec §3f): every wall a newcomer can reach ends in one clean line, and every input retrace
//! cannot use is refused in one. Each test runs the CLI and asserts the documented exit code,
//! exactly one `retrace:` line naming the thing, the output the guest printed before a wall, and no
//! `panicked at` on either stream. Guest walls also replay, and the replay must end at the same
//! place with the incomplete-recording line (C2).
//!
//! An exit code is never the only evidence (CLAUDE.md): `record` passes the guest's own code
//! through, so every assertion pins the line too. Fixtures: `cf_walls` (one mode per wall),
//! `cf_rs_chroot`, and `tests/fixtures/cf_script.sh`.
mod util;
use retrace_trace::Event;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The docs every wall line links to (spec §3b). Spelled out, not taken from retrace_arch, so a
/// wrong constant cannot agree with itself.
const WALLS: &str = "https://github.com/NMitchem/retrace/blob/main/docs/walls.md";
/// Each run's bound. A run the bound kills fails its test rather than stalling the gate.
const BOUND: u64 = 300;

/// A scratch path unique to this process and call.
fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!("retrace-cf-{}-{}-{name}", std::process::id(),
                                      NEXT.fetch_add(1, Ordering::Relaxed)))
}

/// The signed CLI, bounded.
fn cli(args: &[&str]) -> util::Bounded { util::run_bounded(util::bin(), args, &[], BOUND) }

/// The lines retrace itself printed as its verdict. Every test asserts how many there are.
fn retrace_lines(stderr: &str) -> Vec<&str> { stderr.lines().filter(|l| l.starts_with("retrace:")).collect() }

/// No panic reached the user, on either stream, in any spelling Rust prints.
fn assert_no_panic(r: &util::Bounded, what: &str) {
    let out = String::from_utf8_lossy(&r.stdout);
    assert!(!r.stderr.contains("panicked at") && !out.contains("panicked at"),
        "{what}: a panic reached the user. stderr:\n{}", r.stderr);
}

/// Bad input (spec §3b, R2): exactly one `retrace:` line, containing each of `says`, and exit 2.
fn assert_bad_input(r: &util::Bounded, what: &str, says: &[&str]) {
    assert_no_panic(r, what);
    let lines = retrace_lines(&r.stderr);
    assert_eq!(lines.len(), 1, "{what}: exactly one retrace: line. stderr:\n{}", r.stderr);
    for s in says { assert!(lines[0].contains(s), "{what}: the line must say {s:?}: {}", lines[0]); }
    assert!(!lines[0].contains("internal error"), "{what}: bad input is not a bug: {}", lines[0]);
    assert_eq!(r.code, Some(2), "{what}: bad input exits 2. stderr:\n{}", r.stderr);
}

/// A script handed to record-dyn is refused by name; before M50 it failed `parse_macho`'s assert.
#[test]
fn a_shell_script_is_refused_as_not_a_mach_o() {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/cf_script.sh");
    let t = scratch("script.bin");
    let r = cli(&["record-dyn", script, "-o", t.to_str().unwrap()]);
    assert_bad_input(&r, "script", &[script, "is a script, not a Mach-O executable"]);
    assert!(!t.exists(), "nothing is recorded for bad input");
}

/// An empty file panicked slicing its magic before M50.
#[test]
fn an_empty_file_is_refused_as_not_a_mach_o() {
    let empty = scratch("empty");
    std::fs::write(&empty, b"").unwrap();
    let t = scratch("empty.bin");
    let r = cli(&["record-dyn", empty.to_str().unwrap(), "-o", t.to_str().unwrap()]);
    assert_bad_input(&r, "empty", &["is empty or too short to be a Mach-O executable"]);
    let _ = std::fs::remove_file(&empty);
}

/// A thin x86_64 binary passed the magic check before M50 and was loaded as if it were arm64.
#[test]
fn a_thin_x86_64_binary_is_refused_by_name() {
    let exe = scratch("x86_64");
    let built = std::process::Command::new("clang")
        .args(["-arch", "x86_64", "-o", exe.to_str().unwrap(), retrace_guest::CF_WALLS_SRC]).output();
    if !built.as_ref().is_ok_and(|o| o.status.success()) {
        util::announce("SKIPPED a_thin_x86_64_binary_is_refused_by_name: `clang -arch x86_64` did not \
            build cf_walls.c (no x86_64 SDK support), so this row did NOT run and is not evidence of anything.");
        return;
    }
    let t = scratch("x86_64.bin");
    let r = cli(&["record-dyn", exe.to_str().unwrap(), "-o", t.to_str().unwrap(), "--", "chroot"]);
    assert_bad_input(&r, "x86_64", &["is an x86_64 program, and retrace runs arm64 programs only"]);
    let _ = std::fs::remove_file(&exe);
}

/// A missing operand is a usage error with exit 2 on every subcommand (R2). Before M50 `record-dyn`
/// alone indexed past its arguments and panicked with exit 101.
#[test]
fn missing_arguments_are_a_usage_error_not_a_panic() {
    let cases: [&[&str]; 7] = [&["record-dyn"], &["record-dyn", retrace_guest::CF_WALLS],
        &["record-dyn", retrace_guest::CF_WALLS, "-o"], &["record"], &["replay"], &["frobnicate"], &[]];
    for args in cases {
        assert_bad_input(&cli(args), &format!("{args:?}"), &["usage: retrace"]);
    }
}

/// Without the hypervisor entitlement `hv_vm_create` is denied. That is one line naming the
/// entitlement and the command that grants it, not a panic in `Vm::create().expect`.
#[test]
fn a_retrace_without_the_hypervisor_entitlement_says_how_to_sign_it() {
    // Signed ad hoc WITHOUT the entitlement: arm64 runs only signed code, so unsigned is no option,
    // and util::bin()'s copy carries the entitlement.
    let copy = scratch("retrace-unentitled");
    std::fs::copy(env!("CARGO_BIN_EXE_retrace"), &copy).unwrap();
    let signed = std::process::Command::new("codesign").args(["-s", "-", "-f", copy.to_str().unwrap()])
        .output().unwrap();
    assert!(signed.status.success(), "codesign: {}", String::from_utf8_lossy(&signed.stderr));
    let t = scratch("unentitled.bin");
    let r = util::run_bounded(copy.to_str().unwrap(),
        &["record-dyn", retrace_guest::CF_WALLS, "-o", t.to_str().unwrap(), "--", "chroot"], &[], BOUND);
    assert_bad_input(&r, "unentitled", &["cannot create a VM", "com.apple.security.hypervisor", "codesign"]);
    assert!(!t.exists(), "the check comes before the trace is created");
    let _ = std::fs::remove_file(&copy);
}

/// The bug banner (spec §3b): a panic that is not a wall prints three lines, naming the bug, the
/// issues page and the panic's own message and location, then exits 70, never 101 (R3). The trigger
/// is `RETRACE_DEBUG_PANIC`, a seam that exists only in debug builds (§3f), so no program reaches it.
#[test]
fn a_retrace_bug_prints_the_banner_and_exits_70() {
    let t = scratch("bug.bin");
    let r = util::run_bounded(util::bin(),
        &["record-dyn", retrace_guest::CF_WALLS, "-o", t.to_str().unwrap(), "--", "chroot"],
        &[("RETRACE_DEBUG_PANIC", "1")], BOUND);
    assert_no_panic(&r, "banner");
    let lines: Vec<&str> = r.stderr.lines().collect();
    let at = lines.iter()
        .position(|l| *l == "retrace: internal error - this is a bug in retrace, not in your program.")
        .unwrap_or_else(|| panic!("no banner. stderr:\n{}", r.stderr));
    assert_eq!(lines.get(at + 1).copied(),
        Some("  please report it at https://github.com/NMitchem/retrace/issues and include the next line:"));
    let msg = lines.get(at + 2).copied().unwrap_or("");
    assert!(msg.starts_with("  RETRACE_DEBUG_PANIC is set: a deliberate internal error")
            && msg.contains(" (at crates/retrace/src/main.rs:"), "the panic's message and location: {msg}");
    assert_eq!(retrace_lines(&r.stderr).len(), 1, "the banner's first line is the only retrace: line");
    assert_eq!(r.code, Some(70), "a retrace bug exits 70 (R3). stderr:\n{}", r.stderr);
}

/// A guest path that does not exist panicked in `fs::read(..).expect` before M50.
#[test]
fn an_unreadable_guest_path_is_one_clean_line() {
    let t = scratch("missing.bin");
    let r = cli(&["record-dyn", "/no/such/program", "-o", t.to_str().unwrap()]);
    assert_bad_input(&r, "missing guest", &["cannot read /no/such/program"]);
}

/// A guest wall (spec §3f). The recording prints the guest's output, then exactly one wall line that
/// starts `retrace: unsupported: <what>` and links `#<anchor>`, as the last line, and exits 4. Its
/// replay prints the same output, ends with the incomplete-recording line (C2), and exits 4.
/// Returns the wall line for a test's own further checks.
fn assert_guest_wall(guest: &str, args: &[&str], stdout: &str, what: &str, anchor: &str) -> String {
    assert_guest_wall_both(guest, args, stdout, what, anchor).0
}

/// `assert_guest_wall`, returning the replay's incomplete line as well, for a row that pins its text.
fn assert_guest_wall_both(guest: &str, args: &[&str], stdout: &str, what: &str, anchor: &str) -> (String, String) {
    let t = scratch("wall.bin");
    let tp = t.to_str().unwrap();
    let mut argv = vec!["record-dyn", guest, "-o", tp];
    if !args.is_empty() { argv.push("--"); argv.extend_from_slice(args); }
    let rec = cli(&argv);
    assert_no_panic(&rec, what);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), stdout, "{what}: the output before the wall is printed first");
    let lines = retrace_lines(&rec.stderr);
    assert_eq!(lines.len(), 1, "{what}: exactly one retrace: line. stderr:\n{}", rec.stderr);
    let line = lines[0].to_owned();
    assert!(line.starts_with(&format!("retrace: unsupported: {what}")), "{line}");
    assert!(line.ends_with(&format!(" - see {WALLS}#{anchor}")), "{line}");
    assert!(line.is_ascii(), "R1: the line is ASCII: {line}");
    assert_eq!(rec.stderr.lines().last(), Some(line.as_str()), "{what}: the wall line is the last line");
    assert_eq!(rec.code, Some(4), "{what}: a wall exits 4. stderr:\n{}", rec.stderr);
    let rp = cli(&["replay", tp]);
    assert_no_panic(&rp, what);
    assert_eq!(rp.stdout, rec.stdout, "{what}: replay prints what the recording printed");
    let rlines = retrace_lines(&rp.stderr);
    assert_eq!(rlines.len(), 1, "{what}: replay's one retrace: line. stderr:\n{}", rp.stderr);
    assert!(rlines[0].starts_with("retrace: incomplete recording: it ends at ")
            && rlines[0].ends_with(&format!(" - see {WALLS}#incomplete-recordings")), "{}", rlines[0]);
    assert!(!rp.stderr.contains("DIVERGENCE"), "{what}: the end of a walled recording is not a divergence");
    assert_eq!(rp.code, Some(4), "{what}: replay of a walled recording exits 4. stderr:\n{}", rp.stderr);
    let _ = std::fs::remove_file(&t);
    (line, rlines[0].to_owned())
}

/// A recording cut before its terminal event, as a killed recorder leaves it.
fn cut_before_end(full: &Path, cut: &Path) {
    let (events, torn) = retrace_trace::Reader::open_checked(full).unwrap();
    assert!(!torn, "the full recording is whole");
    let end = events.iter().position(|e| matches!(e, Event::Exit { .. })).expect("the recording exits");
    let mut w = retrace_trace::Writer::create(cut).unwrap();
    for e in &events[..end] { w.append(e).unwrap(); }
}

/// `hello_dyn` recorded whole, then cut before its exit: a recording whose recorder was killed.
fn killed_recording() -> PathBuf {
    let full = scratch("hello.bin");
    let rec = cli(&["record-dyn", retrace_guest::HELLO_DYN, "-o", full.to_str().unwrap()]);
    assert_eq!((rec.code, rec.stdout.as_slice()), (Some(0), &b"hi\n"[..]), "{}", rec.stderr);
    let cut = scratch("hello-cut.bin");
    cut_before_end(&full, &cut);
    let _ = std::fs::remove_file(&full);
    cut
}

/// Review Focus 5: a newcomer's typo in `-o` was a recorder error before M50; it is bad input.
#[test]
fn an_unwritable_trace_path_is_one_clean_line() {
    let r = cli(&["record-dyn", retrace_guest::CF_WALLS, "-o", "/no/such/dir/t.bin", "--", "chroot"]);
    assert_bad_input(&r, "unwritable", &["cannot write the trace /no/such/dir/t.bin"]);
}

/// Review Focus 4: no terminal event and no wall. Replay says the recording is incomplete, with the
/// output it has, and exits 4; before M50 it was `DIVERGENCE … got None` and exit 3.
#[test]
fn replaying_a_recording_whose_recorder_was_killed_reports_it_incomplete() {
    let cut = killed_recording();
    let rp = cli(&["replay", cut.to_str().unwrap()]);
    assert_no_panic(&rp, "killed");
    assert_eq!(rp.stdout, b"hi\n", "replay prints what the program printed before the cut");
    let lines = retrace_lines(&rp.stderr);
    assert_eq!(lines.len(), 1, "{}", rp.stderr);
    assert!(lines[0].starts_with("retrace: incomplete recording: it ends at landmark ")
            && lines[0].ends_with(&format!(" - see {WALLS}#incomplete-recordings")), "{}", lines[0]);
    assert!(!rp.stderr.contains("DIVERGENCE"), "{}", rp.stderr);
    assert_eq!(rp.code, Some(4), "{}", rp.stderr);
    let _ = std::fs::remove_file(&cut);
}

/// Review Focus 4, R5: `debug` and `gdbserver` refuse an incomplete recording at the door, with one
/// line and exit 5, before any socket. Without the check, `gdbserver` would listen until the bound.
#[test]
fn debug_and_gdbserver_refuse_an_incomplete_recording() {
    let cut = killed_recording();
    let c = cut.to_str().unwrap();
    for (args, prefix) in [(vec!["debug", c, "--script", "continue"], "DEBUG ERROR: "),
                           (vec!["gdbserver", c], "GDBSERVER ERROR: ")] {
        let r = util::run_bounded(util::bin(), &args, &[], 60);
        assert_no_panic(&r, prefix);
        let said: Vec<&str> = r.stderr.lines().filter(|l| l.contains("is an incomplete recording")).collect();
        assert_eq!(said.len(), 1, "{prefix}: one line. stderr:\n{}", r.stderr);
        assert!(said[0].starts_with(prefix) && said[0].ends_with(&format!(" - see {WALLS}#incomplete-recordings")),
            "{}", said[0]);
        assert!(!r.stderr.contains("listening on"), "{prefix}: refused before the socket");
        assert_eq!(r.code, Some(5), "{prefix}: exit 5. stderr:\n{}", r.stderr);
    }
    let _ = std::fs::remove_file(&cut);
}

/// D4, narrowed by M51 (R6): an Apple implementation-defined instruction (AMX) native runs is still
/// a wall in `exceptions`. The line names the instruction and its pc. The `brk` this row pinned until
/// M51 is now the crash native reports (traps_e2e).
#[test]
fn an_instruction_native_runs_and_retrace_cannot_is_a_wall_in_exceptions() {
    let line = assert_guest_wall(retrace_guest::CF_WALLS, &["amx"], "cf: amx\n",
        "an implementation-defined instruction (such as Apple's AMX) at pc 0x", "exceptions");
    assert!(line.contains("(exception class 0x3f, instruction 0x00201220)"), "{line}");
}

/// `Route::Unsupported`: a Mach message retrace does not answer (`host_get_io_main`, msgh_id 205,
/// dddiagnose's wall) is a wall in `mach-messages`.
#[test]
fn a_mach_message_retrace_does_not_answer_is_a_wall_in_mach_messages() {
    let line = assert_guest_wall(retrace_guest::CF_WALLS, &["iomain"], "cf: iomain\n", "mach_msg2 at pc 0x",
        "mach-messages");
    assert!(line.contains("msgh_id 205"), "{line}");
}

/// Spec §3b's own example shape: an unrowed syscall is one line naming it by the SDK's name and
/// number. Before M50 it was the M33 panic.
#[test]
fn a_program_that_calls_chroot_stops_at_one_clean_line() {
    let (line, replay) = assert_guest_wall_both(retrace_guest::CF_WALLS, &["chroot"], "cf: chroot\n",
        "chroot (syscall 61)", "syscalls");
    assert_eq!(line, format!("retrace: unsupported: chroot (syscall 61) - see {WALLS}#syscalls"));
    // The wall is raised by a record arm, so replay has nothing to meet and names the last landmark
    // (walls.md's Incomplete recordings section quotes this form).
    assert!(replay.contains("it ends at landmark"), "{replay}");
}

/// M54 t2: a `posix_spawn` that would start a child is a ProcessCreation wall, not M38's EFAULT.
#[test]
fn a_child_posix_spawn_ends_the_recording_at_one_clean_line() {
    let (line, _) = assert_guest_wall_both(retrace_guest::FC_SPAWN_CHILD, &[], "parent: spawning\n",
        "posix_spawn of a child process (syscall 244)", "process-creation");
    assert_eq!(line, format!("retrace: unsupported: posix_spawn of a child process (syscall 244) - see {WALLS}#process-creation"));
}

/// Review Focus 2: the wall on a thread that is not main. The output main printed still comes first,
/// and the line is the same line: it names the call, never the thread.
#[test]
fn a_worker_threads_chroot_stops_the_recording_cleanly() {
    let line = assert_guest_wall(retrace_guest::CF_WALLS, &["threadchroot"], "cf: threadchroot\n",
        "chroot (syscall 61)", "syscalls");
    assert_eq!(line, format!("retrace: unsupported: chroot (syscall 61) - see {WALLS}#syscalls"));
}

/// Review Focus 1: a Rust program, whose runtime installs SIGSEGV and SIGBUS handlers and an
/// alternate stack before main, stops at the same one line.
#[test]
fn a_rust_program_that_calls_chroot_stops_at_one_clean_line() {
    let line = assert_guest_wall(retrace_guest::CF_RS_CHROOT, &[], "cf: rs_chroot\n", "chroot (syscall 61)", "syscalls");
    assert_eq!(line, format!("retrace: unsupported: chroot (syscall 61) - see {WALLS}#syscalls"));
}

/// `pause()` with nothing to signal it is the deadlock wall (M53 C6): D1's line, with a clause naming the
/// thread that waits in `sigsuspend`. Natively this program hangs forever.
#[test]
fn a_program_that_pauses_for_a_signal_stops_at_one_clean_line() {
    let line = assert_guest_wall(retrace_guest::CF_WALLS, &["pause"], "cf: pause\n",
        "M14: DEADLOCK - every thread is blocked and nothing is left to wake one", "threads");
    assert!(line.contains("; thread 0 waits in sigsuspend for a signal nothing can send - see "), "{line}");
}

/// `socketpair` has no row. (M52 §3e made `sched_yield` a modelled call, so this row replaced its wall, C7.)
#[test]
fn a_socketpair_stops_at_one_clean_line() {
    assert_guest_wall(retrace_guest::CF_WALLS, &["socketpair"], "cf: socketpair\n", "socketpair (syscall 135)", "syscalls");
}

/// M53 t4 (C1): `poll` on a regular file is outside the pipe model and refused by value, naming the
/// call and the descriptor (the guest's number for it, 4 under retrace).
#[test]
fn a_poll_on_a_file_stops_at_one_clean_line() {
    let line = assert_guest_wall(retrace_guest::CF_WALLS, &["pollfile"], "cf: pollfile\n",
        "poll (syscall 230) on fd 4, not a guest pipe end", "threads");
    assert_eq!(line, format!("retrace: unsupported: poll (syscall 230) on fd 4, not a guest pipe end - see {WALLS}#threads"));
}

/// M53 t4 (Ruling T4-closepolled): another thread closing the descriptor a blocked poller watches is
/// unmeasured, so it is refused by value, as Task 3's close of a waited kqueue is.
#[test]
fn a_close_under_a_blocked_poll_stops_at_one_clean_line() {
    assert_guest_wall(retrace_guest::POLLSEL_DYN, &["pollclose"], "pollsel: pollclose\n",
        "M53: close of descriptor ", "threads");
}

/// D1: every thread blocked with nothing left to wake one. Natively this program hangs forever; under
/// retrace it is a wall in `threads` whose line says so, and says to report it if it does not hang.
#[test]
fn a_deadlock_is_a_wall_that_says_natively_the_program_would_hang() {
    let line = assert_guest_wall(retrace_guest::CF_WALLS, &["deadlock"], "cf: deadlock\n",
        "M14: DEADLOCK - every thread is blocked", "threads");
    assert!(line.contains("Natively this program would hang here; if it does not, this is a bug in retrace"), "{line}");
}

/// Spec §2d corrected M49's note: `clock_sleep_trap` reaches the M33 panic before any forward, so it
/// is a refusal target, not a hang.
#[test]
fn a_clock_sleep_trap_stops_at_one_clean_line() {
    assert_guest_wall(retrace_guest::CF_WALLS, &["clocksleep"], "cf: clocksleep\n",
        "clock_sleep_trap (mach trap -62)", "syscalls");
}

/// The same for `mach_wait_until`.
#[test]
fn a_mach_wait_until_stops_at_one_clean_line() {
    assert_guest_wall(retrace_guest::CF_WALLS, &["waituntil"], "cf: waituntil\n",
        "mach_wait_until (mach trap -90)", "syscalls");
}

/// Spec §2c: block SIGUSR1, raise it, unblock it. Its default action ends the process at the unblock,
/// which retrace does not record yet; dropping the signal would turn a death into a clean exit.
#[test]
fn a_signal_raised_while_blocked_stops_at_one_clean_line_when_it_is_unblocked() {
    assert_guest_wall(retrace_guest::CF_WALLS, &["unblock"], "cf: unblock\n",
        "signal 30 became deliverable at an unmask or a wake and its default action is Terminate", "signals");
}

/// Review Focus 3: `RUST_BACKTRACE=1` is common among Rust developers. A wall reached by a `refuse!`
/// panic still prints exactly one line and no backtrace, on record and on replay: the hook is silent
/// for a wall, and Rust's default hook, which would print `panicked at` and the frames, is replaced.
/// M51: the `unblock` wall (`take_pending_delivery`'s `refuse!`), since signal 0 is answered now.
#[test]
fn a_wall_prints_no_backtrace_even_with_rust_backtrace_set() {
    let t = scratch("bt.bin");
    let tp = t.to_str().unwrap();
    let env = [("RUST_BACKTRACE", "1")];
    let rec = util::run_bounded(util::bin(), &["record-dyn", retrace_guest::CF_WALLS, "-o", tp, "--", "unblock"], &env, BOUND);
    let rp = util::run_bounded(util::bin(), &["replay", tp], &env, BOUND);
    for (r, side) in [(&rec, "record"), (&rp, "replay")] {
        assert_no_panic(r, side);
        assert!(!r.stderr.contains("stack backtrace") && !r.stderr.lines().any(|l| l.trim_start().starts_with("0: ")),
            "{side}: no backtrace for a wall. stderr:\n{}", r.stderr);
        assert_eq!(retrace_lines(&r.stderr).len(), 1, "{side}: one retrace: line. stderr:\n{}", r.stderr);
        assert_eq!(r.code, Some(4), "{side}: exit 4. stderr:\n{}", r.stderr);
    }
    assert!(retrace_lines(&rec.stderr)[0].starts_with("retrace: unsupported: "), "{}", rec.stderr);
    let _ = std::fs::remove_file(&t);
}

/// T3-closewait: the close of the kqueue descriptor a thread waits in `kevent` on is refused, because
/// natively the waiter wakes at once with EBADF. The refusal is raised through `note_fd_effects`
/// after record appended the close's landmark, so replay must reach the incomplete line.
#[test]
fn closing_a_kqueue_descriptor_a_thread_waits_on_stops_at_one_clean_line_and_replays_to_its_end() {
    assert_guest_wall(retrace_guest::KQDUP_DYN, &["closewait"], "closewait: closing\n",
        "M53: close of kqueue descriptor ", "threads");
}
