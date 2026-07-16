//! The M3 headline gate: a scripted reverse-debug session over a `hello_dyn` recording is
//! deterministic and its coordinates round-trip. Two independent `retrace debug --script` sessions
//! on the SAME fresh recording emit a byte-identical transcript (the primary oracle); the scripted
//! coordinates behave — `continue` catches the breakpoint at the write-return landmark boundary, a
//! `reverse-stepi` backs into the write's window, a second `reverse-stepi` steps one instruction
//! within it, and `stepi` round-trips forward to the exact same (N, K).
//!
//! Honest-gate discipline: written `#[ignore]`d, walked, and un-ignored ONLY on a genuine double
//! pass. No assertion is ever loosened to reach green — a §2-vs-reality mismatch is a finding.

mod util;

use retrace_core::{seek, Advance, ReplaySession};
use std::path::Path;
use std::process::Command;

// The single breakpoint is discovered in-process (never a hardcoded address): it is the RETURN pc
// of `write(1, "hi\n", …)`. That pc is the first instruction of the write's next (exit) window, so
// `continue` catches it as a landmark-boundary hit — the executor's `Advance::Event` path compares
// `pc()`, which at a K=0 boundary equals ELR/`position()` (`set_x0_err_and_return` set PC = ELR).
//
// The adapted script (two `reverse-stepi` then a `stepi`, per the controller's Task-5-semantics
// resolution): a single `reverse-stepi; stepi` would leave the `stepi` at the window-end position
// (W−1, L), where stepping errors with 0 remaining. Backing up twice puts the round-trip's `stepi`
// at (W−1, L−1) → (W−1, L), which is a clean in-window step.
#[test]
fn reverse_debug_transcript_is_deterministic() {
    // Fresh recording of the dynamically-linked hello, through real /usr/lib/dyld.
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "record-dyn failed: {}", rec.stderr);

    // --- In-process landmark discovery. Sequential VMs only: each session is dropped before the
    //     next one opens (one VM per process), and ALL of it precedes the CLI spawns. ---

    // 1. Drive a discovery session to the write(1, …) trap, advance PAST it, and capture the
    //    return-address pc + the landmark index reached. `peek_syscall` recognizes the write BEFORE
    //    consuming it; `advance()` then consumes it, leaving the guest at the K=0 boundary of the
    //    next window. `position()` (ELR) and `pc()` (reg PC) coincide there — asserted, then we take
    //    `pc()`, the exact value the `continue` boundary check compares.
    let (bp_pc, wr_landmark) = {
        let mut s = ReplaySession::open(Path::new(&trace)).expect("open trace");
        loop {
            match s.peek_syscall() {
                Some((4, args)) if args[0] == 1 => {
                    s.advance().expect("advance past write");
                    assert_eq!(s.position(), s.pc(), "position/pc must coincide at the K=0 boundary");
                    break (s.pc(), s.landmark());
                }
                _ => {
                    if let Advance::Exited(_) = s.advance().expect("advance during discovery") {
                        panic!("hello_dyn never issued write(1, …) before exit");
                    }
                }
            }
        }
    };

    // 2. The write's window is the landmark just before the captured boundary. Probe its length L
    //    — the reverse-stepi/stepi round-trip coordinate. Fresh session: the discovery one is
    //    already dropped. (Only L is taken: `window_len_here` steps THROUGH the window-ending SVC,
    //    so it parks the guest at the EL1 trampoline — its `pc()` is NOT the (write_window, L)
    //    coordinate's pc. The coordinate's exact pc is instead proven by the transcript byte-compare
    //    and the coordinate anchors below.)
    let write_window = wr_landmark - 1;
    let win_len = {
        let mut s = seek(Path::new(&trace), write_window, 0).expect("seek write window");
        s.window_len_here().expect("window length")
    };
    assert!(win_len >= 1, "the write window must hold at least one instruction before the SVC");
    let prev_k = win_len - 1;

    // --- The scripted reverse-debug session, run twice via the spawned CLI ---
    let script = format!(
        "break 0x{bp_pc:x}; continue; where; regs; \
         reverse-stepi; where; reverse-stepi; where; stepi; where; \
         reverse-continue; where"
    );

    let run = || {
        let out = Command::new(util::bin())
            .args(["debug", trace.to_str().unwrap(), "--script", &script])
            .output()
            .expect("spawn retrace debug");
        assert_eq!(
            out.status.code(),
            Some(0),
            "debug exited non-zero: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    };

    // PRIMARY oracle: two independent CLI sessions on the SAME recording are byte-identical.
    let t1 = run();
    let t2 = run();
    assert_eq!(t1, t2, "transcript is not byte-identical across two sessions");

    // Secondary coordinate anchors (each proven in the task report).
    let text = String::from_utf8(t1).expect("utf8 transcript");

    // `continue` catches the breakpoint at the write-return landmark boundary (wr_landmark, 0).
    let hit = format!("hit 0x{bp_pc:x} at ({wr_landmark}, 0)");
    assert!(text.contains(&hit), "continue must hit the write-return boundary\nwant: {hit}\n{text}");
    // `where` at that boundary prints the same pc.
    let where_hit = format!("at ({wr_landmark}, 0) pc=0x{bp_pc:x}");
    assert!(text.contains(&where_hit), "where after continue\nwant: {where_hit}\n{text}");
    // Round-trip: the first `reverse-stepi` lands at (write_window, win_len); `stepi` returns there.
    // That `where` coordinate therefore appears more than once — the coordinate round-trip proof.
    let round = format!("at ({write_window}, {win_len}) pc=");
    let seen = text.matches(&round).count();
    assert!(seen >= 2, "stepi must round-trip to the reverse-stepi position ({round}); saw {seen}×\n{text}");
    // The second `reverse-stepi` actually stepped one instruction back inside the window.
    let mid = format!("at ({write_window}, {prev_k}) pc=");
    assert!(text.contains(&mid), "second reverse-stepi must step back one insn ({mid})\n{text}");
    // `reverse-continue` from (write_window, win_len): the sole hit (wr_landmark, 0) is LATER, so none earlier.
    assert!(
        text.contains("no earlier hit"),
        "reverse-continue must report no earlier hit (the sole breakpoint hit is later)\n{text}"
    );
}
