// M48 gate (spec §3e, §3h, §4; Review Focus 2, 3 and 5). psynch condition variables end to end, on
// `condvar_dyn`. Every assertion is on the guest's own output compared with native's, on the trace,
// or on the recorder's own words, never on an exit code alone: a wait that never blocks, or a wake
// that answers the wrong word, still lets most of these guests exit 0.
mod util;

use retrace_trace::Event;
use std::path::{Path, PathBuf};

/// `condvar_dyn.c`'s `ROUNDS`.
const ROUNDS: usize = 8;

/// The fixture run natively: (exit code, stdout).
fn native(argv: &[&str]) -> (i32, Vec<u8>) {
    let out = std::process::Command::new(retrace_guest::CONDVAR_DYN).args(argv).output().unwrap();
    (out.status.code().unwrap_or(-1), out.stdout)
}

/// Record `argv`; assert exit 0 and native's stdout; replay twice byte-identically.
fn records_and_replays_as_native(argv: &[&str]) -> (String, PathBuf) {
    let (code, want) = native(argv);
    assert_eq!(code, 0, "{argv:?}: native exit");
    let (rec, trace) = util::record_dynamic_args(retrace_guest::CONDVAR_DYN, argv);
    assert_eq!(rec.code, 0, "{argv:?}: record: {}", rec.stderr);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), String::from_utf8_lossy(&want), "{argv:?}: recorded stdout vs native");
    for n in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 0, "{argv:?}: replay {n}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "{argv:?}: replay {n} stdout");
    }
    (String::from_utf8_lossy(&rec.stdout).into_owned(), trace)
}

/// Every landmark of syscall `num` in `trace`: (landmark index, thread, args, ret).
fn calls(trace: &Path, num: u64) -> Vec<(usize, u32, [u64; 8], u64)> {
    retrace_trace::Reader::open(trace).unwrap().into_iter().enumerate().filter_map(|(i, e)| match e {
        Event::Syscall { num: n, args, ret, thread, .. } if n == num => Some((i, thread, args, ret)),
        _ => None,
    }).collect()
}

/// §4's pingpong guard: ROUNDS rounds in strict alternation, as native prints them, with both
/// threads blocking in the kernel. Under the cooperative scheduler the counts are forced: main plays
/// ping 0 without waiting, then each side waits once per later round, and every signal but the two
/// that find nobody waiting (main's first, the ponger's last) reaches the kernel and wakes exactly
/// one waiter. A wake-any model passes one round, and a model that never blocks spins forever.
#[test]
fn pingpong_alternates_strictly_with_both_threads_waiting_in_the_kernel() {
    let (out, trace) = records_and_replays_as_native(&["pingpong"]);
    assert!(out.ends_with("ping 7\npong 7\npingpong done\n"), "{out}");
    let waits = calls(&trace, retrace_arch::SYS_PSYNCH_CVWAIT);
    for t in [0, 1] {
        assert_eq!(waits.iter().filter(|w| w.1 == t).count(), ROUNDS - 1, "thread {t}'s waits: {waits:x?}");
    }
    let signals = calls(&trace, retrace_arch::SYS_PSYNCH_CVSIGNAL);
    assert_eq!(signals.len(), 2 * (ROUNDS - 1), "signals that found a waiter: {signals:x?}");
    assert!(signals.iter().all(|s| s.3 == 0x101),
        "T0(M4): each signal wakes the one waiter and balances L and S: {signals:x?}");
    assert!(calls(&trace, retrace_arch::SYS_PSYNCH_CVBROAD).is_empty());
}

/// One `cvbroad` releases all three waiters, each of which blocked on that cv in the kernel.
#[test]
fn a_broadcast_wakes_all_three_waiters_with_one_call() {
    let (out, trace) = records_and_replays_as_native(&["broadcast"]);
    assert_eq!(out, "broadcast woke 3: saw 1 1 1\n");
    let broads = calls(&trace, retrace_arch::SYS_PSYNCH_CVBROAD);
    assert_eq!(broads.len(), 1, "{broads:x?}");
    let (_, thread, args, ret) = broads[0];
    assert_eq!((thread, ret), (0, 0x301), "main broadcasts; T0(M4): three increments and the C bit");
    let mut waiters: Vec<u32> = calls(&trace, retrace_arch::SYS_PSYNCH_CVWAIT).into_iter()
        .filter(|w| w.2[0] == args[0]).map(|w| w.1).collect();
    waiters.sort();
    assert_eq!(waiters, [1, 2, 3], "each waiter blocked on the broadcast's cv");
}

/// §4's timedout guard. A 5 ms wait nobody signals blocks, the idle jump reaches its deadline, and
/// the kernel's timeout word comes back. A wait that returns at once fails `elapsed_ge_timeout`;
/// an errno without ECVCLEARED leaves the C bit off S in the c_seq line and fails the raw errno.
#[test]
fn a_timed_wait_that_expires_returns_the_kernels_timeout_word_after_the_idle_jump() {
    let (out, trace) = records_and_replays_as_native(&["timedout"]);
    assert!(out.contains("timedout rc=60 elapsed_ge_timeout=1\n"), "{out}");
    assert!(out.contains("timedout c_seq: 00010000 01010000 00000000\n"), "T0(M4): S = 0x101 at the c_seq offset. {out}");
    assert!(out.contains("timedout raw rv=0xffffffff errno=0x13c\n"), "T0(M4): ETIMEDOUT | ECVCLEARED, carry set. {out}");
    let waits = calls(&trace, retrace_arch::SYS_PSYNCH_CVWAIT);
    assert!(waits.len() == 2 && waits.iter().all(|w| w.1 == 0 && w.2[6] == 0 && w.2[7] == 5_000_000 && w.3 == 0),
        "the libpthread wait and the raw one, both blocking (landmark word 0) with the 5 ms interval: {waits:x?}");
}

/// A timed wait signalled first returns 0, and its deadline is gone: had it fired, the idle jump
/// would have carried the guest's clock past 2 s and `before_deadline` would read 0.
#[test]
fn a_timed_wait_signalled_before_its_deadline_returns_zero_and_never_times_out() {
    let (out, trace) = records_and_replays_as_native(&["timedsignal"]);
    assert_eq!(out, "timedsignal rc=0 flag=1 before_deadline=1\n");
    let waits = calls(&trace, retrace_arch::SYS_PSYNCH_CVWAIT);
    assert!(waits.len() == 1 && (waits[0].1, waits[0].2[6], waits[0].2[7]) == (0, 2, 0),
        "main's one timed wait, {{2, 0}}: {waits:x?}");
    let signals = calls(&trace, retrace_arch::SYS_PSYNCH_CVSIGNAL);
    assert!(signals.len() == 1 && signals[0].0 > waits[0].0 && signals[0].1 == 1 && signals[0].3 == 0x101,
        "the second thread's signal, after the wait, woke it (T0(M4)): {signals:x?}");
}

/// Review Focus 2. node's shape: `{0, 1 ns}` is 0 ticks, so the wait blocks with its deadline
/// already reached and is woken in the same `schedule_after_block` while it is still the current
/// thread, where `switch_to_thread` returns early. A timeout word written only to the saved context
/// is lost there: the vCPU keeps the landmark's 0 with carry clear, libpthread reads a successful
/// wait, and rc is 0, not 60.
#[test]
fn a_timed_wait_on_the_only_waiter_answers_on_the_vcpu() {
    let (out, trace) = records_and_replays_as_native(&["onens"]);
    assert!(out.contains("onens rc=60 elapsed_ge_timeout=1\n"), "the timeout reached libpthread: {out}");
    assert!(out.contains("onens raw rv=0xffffffff errno=0x13c\n"), "T0(M4): the word and its carry reached the stub: {out}");
    let waits = calls(&trace, retrace_arch::SYS_PSYNCH_CVWAIT);
    assert!(waits.len() == 2 && waits.iter().all(|w| w.1 == 0 && w.2[6] == 0 && w.2[7] == 1),
        "two {{0, 1 ns}} waits on the only thread: {waits:x?}");
}

/// Spec §3j restore parity. A checkpoint taken while the ponger is blocked in `cvwait`, continued
/// across main's signal that wakes it, must equal a cold seek past the signal: the queue, the thread
/// table, the woken thread's delivered word and memory.
#[test]
fn a_seek_into_a_blocked_cvwait_matches_a_cold_seek() {
    let (_, trace) = records_and_replays_as_native(&["pingpong"]);
    let w = calls(&trace, retrace_arch::SYS_PSYNCH_CVWAIT).into_iter().find(|w| w.1 == 1)
        .expect("the ponger's first wait").0;
    let s = calls(&trace, retrace_arch::SYS_PSYNCH_CVSIGNAL).into_iter().find(|s| s.0 > w)
        .expect("main's signal after it").0;
    let at = retrace_core::seek(&trace, w + 1, 0).unwrap();
    assert!(at.dbg_internal_state().contains("InWait"), "the checkpoint holds the queued waiter:\n{}", at.dbg_internal_state());
    let cp = at.checkpoint();
    drop(at); // one VM per process: from_checkpoint below creates the next
    let warm = {
        let mut r = retrace_core::ReplaySession::from_checkpoint(&trace, &cp).unwrap();
        r.advance_to_landmark(s + 1).unwrap_or_else(|d| panic!("warm: diverged at {}: {}", d.landmark, d.detail));
        (r.current_thread(), r.dbg_regs(), r.dbg_regs_of(1), r.dbg_fp_regs(), r.dbg_internal_state(), r.snapshot().1)
    };
    let cold = retrace_core::seek(&trace, s + 1, 0).unwrap();
    assert!(!cold.dbg_internal_state().contains("InWait"), "past the signal nobody waits:\n{}", cold.dbg_internal_state());
    assert_eq!(warm.0, cold.current_thread(), "thread: checkpointed vs cold");
    assert_eq!(warm.1, cold.dbg_regs(), "registers: checkpointed vs cold");
    assert_eq!(warm.2, cold.dbg_regs_of(1), "the woken ponger's saved context, its delivered word included");
    assert_eq!(warm.3, cold.dbg_fp_regs(), "FP/SIMD: checkpointed vs cold");
    assert_eq!(warm.4, cold.dbg_internal_state(), "the psynch queue and the clock: checkpointed vs cold");
    assert!(cold.diff_memory(&warm.5).is_none(), "memory: checkpointed vs cold");
}

/// Review Focus 5. A `cvwait` the recording accepted but replay refuses is reachable only after an
/// earlier silent divergence, so replay must name it as a `Divergence`, never panic. The trace
/// cannot carry that divergence, because the arguments are compared before the mirror runs, so the
/// test plants its effect in the box (T5-f): a stale waiter at the ponger's own sequence, which
/// `_psynch_cvwait` answers EBUSY and the port refuses.
#[test]
fn a_cvwait_refused_on_replay_is_a_divergence_naming_it_not_a_panic() {
    let (_, trace) = records_and_replays_as_native(&["pingpong"]);
    let (i, _, args, _) = calls(&trace, retrace_arch::SYS_PSYNCH_CVWAIT).into_iter().find(|w| w.1 == 1)
        .expect("the ponger's first wait");
    let lockseq = (args[1] as u32) & 0xffff_ff00;
    let mut s = retrace_core::seek(&trace, i, 0).unwrap();
    s.dbg_psynch_mut().dbg_plant_waiter(args[0], lockseq, 7);
    let d = match s.advance() { Err(d) => d, Ok(_) => panic!("the cvwait at landmark {i} replayed over a planted waiter") };
    assert_eq!(d.landmark, i);
    assert!(d.detail.starts_with("psynch_cvwait refused on replay, though the recording accepted it")
            && d.detail.contains(&format!("M48: psynch psynch_cvwait on cv {:#x} by thread 1", args[0]))
            && d.detail.contains(&format!("thread 7 already waits at sequence {lockseq:#x}")), "{}", d.detail);
}

/// Spec §1 part 5 for the port. The mutex pair is out of scope (plan F3), so a contended mutex,
/// which libpthread's firstfit lock takes to `psynch_mutexwait` (301), stops the recorder by value
/// before anything reaches the host. Natively the same program completes.
#[test]
fn the_mutex_pair_is_refused_by_value_and_never_forwarded() {
    let (code, out) = native(&["mutex"]);
    assert_eq!((code, String::from_utf8_lossy(&out).into_owned()), (0, "mutex ok flag=1\n".to_string()), "native completes");
    let (rec, trace) = util::record_dynamic_args(retrace_guest::CONDVAR_DYN, &["mutex"]);
    assert_eq!(rec.code, 101, "the recorder must stop at the refusal. stderr:\n{}", rec.stderr);
    assert!(rec.stderr.contains("M48: psynch psynch_mutexwait (301) is not modelled"), "stderr:\n{}", rec.stderr);
    assert!(!String::from_utf8_lossy(&rec.stdout).contains("mutex ok"), "the guest must not run past the refusal");
    // The refused call appends no landmark; main's timed wait, which let the locker run, did.
    assert!(calls(&trace, 301).is_empty(), "a refused call is never recorded");
    let waits = calls(&trace, retrace_arch::SYS_PSYNCH_CVWAIT);
    assert!(waits.len() == 1 && waits[0].1 == 0, "main's timed wait preceded the refusal: {waits:x?}");
}
