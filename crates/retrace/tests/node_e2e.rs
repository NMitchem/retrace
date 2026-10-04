//! M48 rung 9 (spec §1 parts 1–2): Homebrew's node, JIT on, records to exit 0 and replays
//! byte-identically twice: `console.log(1)`, and a 2-second `setTimeout` whose deadline the idle
//! jump reaches on the synthetic clock.
//!
//! M47 took node past AMFI's dyld policy and parked this gate at `kevent` (363) on a guest
//! `kqueue()`, libuv's `EVFILT_USER` probe. M48 models guest kqueues, psynch condition variables,
//! deadline-bounded waits, partial `munmap` and `MAP_JIT` write-protect, and fixes the SIMD
//! restore that made a threaded replay depend on the host (walls.md §1, §4), so node runs to its
//! end. The Cellar path, not the `/opt/homebrew/bin` symlink, is what the probe and the walks
//! measured.
//!
//! Each test asserts the difference M48 makes (CLAUDE.md honest-gate rule 1):
//! - `node_prints_one_and_replays`: the recording holds a `MAP_JIT` mapping and the guest wrote
//!   `S3_6_C15_C1_5`, because a run that never reached V8's code space (`--jitless`) prints 1 too
//!   (spec §4).
//! - `node_timer_replays`: the clock reaches the timer's deadline by the idle jump, exactly, and
//!   main runs the callback only after it, because a `kevent` that returned at once prints 2 too.
//!
//! NOT a repo artifact: without Homebrew's node each test announces its skip. The mechanisms are
//! guarded without node by `kq_e2e`, `condvar_e2e`, `jitwp_e2e`, `simd_e2e` and `trim_e2e`. A node
//! trace is about 526 MB (walls.md §2), so each test deletes its trace after its last assertion;
//! a failing run keeps it for diagnosis.
mod util;
use retrace_trace::Event;

const NODE: &str = "/opt/homebrew/bin/node";
/// 2000 ms, not spec §1 part 2's 10 (§11b item 12, P8): `SYNTH_TSC_STRIDE` carries the clock past a
/// 10 ms deadline before the loop's first poll, so a 10 ms timer never blocks and nothing jumps.
const TIMER_JS: &str = "setTimeout(() => console.log(2), 2000)";
/// One second at the guest's 24 MHz timebase (plan F6).
const ONE_SECOND: u64 = 24_000_000;
/// What one timebase read moves the synthetic clock: `SYNTH_TSC_STRIDE` in `retrace-box`.
const STRIDE: u64 = 0x2400;

/// The Cellar binary behind Homebrew's symlink, or `None` after announcing the skip past libtest's
/// capture (CLAUDE.md: a skipped test must announce itself).
fn node(test: &str) -> Option<String> {
    if !std::path::Path::new(NODE).exists() {
        util::announce(&format!("SKIPPED {test}: {NODE} not installed (`brew install node`). \
            This gate did NOT run — it is not evidence of anything."));
        return None;
    }
    Some(std::fs::canonicalize(NODE).unwrap().to_str().unwrap().to_owned())
}

#[test]
fn node_prints_one_and_replays() {
    let Some(exe) = node("node_prints_one_and_replays") else { return };
    // RETRACE_SPRR on the RECORDER: the register lives below the trace (R3), so `sprr_write`'s line
    // is the only witness of a write, and the one recording carries it (Ruling T7-b).
    let (out, rec_stderr) = util::assert_rung_records_and_replays_env(
        &exe, &["-e", "console.log(1)"], b"1\n", &[("RETRACE_SPRR", "1")]);
    assert_eq!(out.stdout, b"1\n");
    // JIT ran (M48 §1 part 1): V8 reserved its code range MAP_JIT …
    let jit = util::map_jit_ranges(&retrace_trace::Reader::open(&out.trace).unwrap());
    assert!(!jit.is_empty(),
        "the recording holds no MAP_JIT mapping: V8 never reserved its code range, which a \
         --jitless run also prints 1 without (spec §4)");
    // … and toggled its write-protect at least once.
    let writes = rec_stderr.lines().filter(|l| l.starts_with("[M48 SPRR] thread ")).count();
    assert!(writes >= 1,
        "the guest never wrote S3_6_C15_C1_5: V8 never toggled its JIT write-protect (MAP_JIT \
         ranges {jit:x?})");
    let _ = std::fs::remove_file(&out.trace);
}

fn synthetic_tsc(state: &str) -> u64 {
    let v = state.split("synthetic_tsc=0x").nth(1).and_then(|r| r.split_whitespace().next())
        .unwrap_or_else(|| panic!("no synthetic_tsc in {state}"));
    u64::from_str_radix(v, 16).unwrap()
}

/// A relative `struct timespec` (`tv_sec`, `tv_nsec`, 8 bytes each) as 24 MHz ticks, truncating.
fn ticks(ts: &[u8]) -> u64 {
    let sec = u64::from_le_bytes(ts[0..8].try_into().unwrap());
    let nsec = u64::from_le_bytes(ts[8..16].try_into().unwrap());
    sec * ONE_SECOND + nsec * 3 / 125
}

#[test]
fn node_timer_replays() {
    let Some(exe) = node("node_timer_replays") else { return };
    let rung = util::assert_rung_records_and_replays(&exe, &["-e", TIMER_JS], b"2\n");

    // Replay in process, reading the clock at every landmark (gcdtimer_e2e's `the_idle_jump`).
    // K is main's last timed kevent of a second or more before the jump: the wait the timer's
    // deadline ends. J is the landmark across which the clock moves by more than a second, which
    // only the idle jump can do: a window would need 2,604 timebase reads.
    let d = retrace_core::DecodedTrace::load(&rung.trace).unwrap();
    let evs = d.events();
    let mut s = retrace_core::ReplaySession::open_decoded(&d).unwrap();
    let mut k: Option<(usize, u64, u64)> = None; // (landmark, clock before its window, timeout ticks)
    let (j, after_j) = loop {
        let (n, before) = (s.landmark(), synthetic_tsc(&s.dbg_internal_state()));
        match s.advance() {
            Ok(retrace_core::Advance::Exited(_)) => panic!(
                "node exited and no landmark moved the clock by a second: the timer's deadline was \
                 never reached by the idle jump"),
            Ok(_) => {}
            Err(e) => panic!("diverged at landmark {}: {}", e.landmark, e.detail),
        }
        if let Event::Syscall { num, args, thread: 0, .. } = &evs[n] {
            if *num == retrace_arch::SYS_KEVENT && args[4] > 0 && args[5] != 0 {
                let t = ticks(&s.read_mem(args[5], 16).expect("main's kevent timeout is readable"));
                if t >= ONE_SECOND { k = Some((n, before, t)); }
            }
        }
        let after = synthetic_tsc(&s.dbg_internal_state());
        if after - before > ONE_SECOND { break (n, after); }
    };
    let (kn, before_k, t) = k.unwrap_or_else(|| panic!(
        "the clock jumped at landmark {j} with no timed kevent of a second or more on main before it"));
    // The jump lands on K's deadline (M48 §3d: the one idle jump goes to the earliest deadline):
    // the clock at K's call plus K's timeout. The clock at the call is the clock before K's window
    // plus that window's own timebase reads, a whole number of STRIDEs.
    let excess = after_j.checked_sub(before_k + t).unwrap_or_else(|| panic!(
        "the jump at landmark {j} stopped short of main's kevent deadline (landmark {kn}: clock \
         {before_k:#x} before its window, timeout {t} ticks; clock after the jump {after_j:#x})"));
    assert!(excess.is_multiple_of(STRIDE) && excess < ONE_SECOND,
        "the jump at landmark {j} must land exactly on main's kevent deadline (landmark {kn}, \
         timeout {t} ticks): {excess:#x} ticks past it is not K's own window's timebase reads");
    assert_eq!(s.current_thread(), 0, "the jump at landmark {j} must wake main, whose deadline it reached");
    let write = evs.iter().position(|e| matches!(e, Event::Syscall { num, args, .. }
        if (*num == retrace_arch::SYS_WRITE || *num == retrace_arch::SYS_WRITE_NOCANCEL) && args[0] == 1))
        .expect("the callback's write of `2` to stdout");
    assert!(write > j,
        "the callback wrote stdout at landmark {write}, before the clock reached its deadline at {j}");
    let _ = std::fs::remove_file(&rung.trace);
}
