// M39 headline gate — rung 8: the real CPython interpreter runs `crash.py` (a script file with
// modest stdlib use on a data file) and dies on a ctypes bad-pointer deref; the run records,
// replays bit-for-bit, and a scripted debug session reverse-continues from the crash to the
// store that wrote the bad pointer.
//
// Four assertions, each on the difference rung 8 makes (CLAUDE.md honest-gate rule 1) — never
// on exit 139 alone, which a guest that died inside dyld produces identically:
//   1. the script RAN: its marker line is in the recorded stdout, `UNREACHED` is not;
//   2. the crash IS the deref: the terminal Event::Crash has far == the computed target, a
//      level-1 translation DFSC, and the thread that wrote the marker;
//   3. replay agrees, twice (byte-identical stdout, no divergence, the M6 crash convention);
//   4. `watch <cell>; reverse-continue` from the crash lands on the store of the pointer,
//      proved by its effect (crashy_cli's proof): before the store the cell is not `target`,
//      one stepi later it is.
// `cell` and every coordinate are DISCOVERED from the recording (the M6 marker convention).
//
// Neither the interpreter nor its stdlib is a repo artifact, so the test skips with a loud
// eprintln! naming the missing path rather than passing quietly — a silent skip reads as a green
// it did not earn. That is also why every mechanism the walk fixes gets its own repo-owned guard
// (vmremap_e2e is the first): this gate guards nothing on a machine without Homebrew Python.
mod util;
use std::path::Path;
use retrace_trace::{Event, Reader};

const REAL: &str =
    "/opt/homebrew/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python";
const TARGET: u64 = 0x4000_DEAD_0000; // crash.json: 0x400000000000 + 0xdead0000

fn debug_run(trace: &str, script: &str) -> (i32, String, String) {
    let out = std::process::Command::new(util::bin())
        .args(["debug", trace, "--script", script])
        .output().expect("spawn debug");
    (out.status.code().unwrap_or(-1),
     String::from_utf8(out.stdout).unwrap(),
     String::from_utf8(out.stderr).unwrap())
}

/// `cell=0x…` out of the guest's own marker line.
fn parse_cell(stdout: &str) -> u64 {
    let start = stdout.find("CRASHPY cell=0x")
        .unwrap_or_else(|| panic!("missing `CRASHPY cell=0x` in stdout:\n{stdout}")) + "CRASHPY cell=0x".len();
    let hex: String = stdout[start..].chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    u64::from_str_radix(&hex, 16).expect("cell hex")
}

/// The terminal crash event, and the thread tag of the LAST write to fd 1 before it (the marker:
/// nothing else reaches stdout after it — `print(p[0])` never gets to write).
fn crash_and_marker_thread(trace: &Path) -> ((u64, u64, u64, u32), u32) {
    let mut last_write_thread = None;
    let mut crash = None;
    for e in Reader::open(trace).unwrap().iter() {
        match e {
            Event::Syscall { num, args, thread, .. } if (*num == 4 || *num == 397) && args[0] == 1 => {
                last_write_thread = Some(*thread);
            }
            Event::Crash { pc, esr, far, thread } => { crash = Some((*pc, *esr, *far, *thread)); }
            _ => {}
        }
    }
    (crash.expect("trace has a Crash event"), last_write_thread.expect("a write(1) before the crash"))
}

#[test]
fn cpython_runs_a_real_script_crashes_on_the_computed_pointer_and_reverse_debugs_to_its_store() {
    if !Path::new(REAL).exists() {
        eprintln!(
            "SKIPPED cpython_runs_a_real_script…: {REAL} not found (expected a Homebrew \
             `python@3.14` install). This gate did NOT run — it is not evidence of anything."
        );
        return;
    }
    let (rec, trace) = util::record_dynamic_args(REAL, &[retrace_guest::CRASH_PY]);
    let stdout = String::from_utf8_lossy(&rec.stdout).into_owned();

    // 1. The script ran: the marker is there, the line after the deref is not.
    assert!(stdout.contains(&format!("target={TARGET:#x} rows=3")),
        "marker line missing (record exit {}). stdout:\n{stdout}\nstderr:\n{}", rec.code, rec.stderr);
    assert!(!stdout.contains("UNREACHED"), "the deref must not return. stdout:\n{stdout}");
    let cell = parse_cell(&stdout);

    // 2. The crash is the deref: FAR is the computed target; DFSC 0x05 (level-1 translation —
    //    bit 46 of the VA selects an L1 slot that has never been mapped, exactly crashy's
    //    0x92000005 at the same VA); and it happened on the thread that wrote the marker.
    let ((_pc, esr, far, cthread), mthread) = crash_and_marker_thread(&trace);
    assert_eq!(far, TARGET, "crash FAR must be the computed target (esr={esr:#x})");
    assert_eq!(esr & 0x3f, 0x05, "DFSC must be a level-1 translation fault (esr={esr:#x})");
    assert_eq!(cthread, mthread, "the deref must run on the marker's thread");
    assert_eq!(rec.code, 139, "a recorded crash exits 139 (M6). stderr:\n{}", rec.stderr);

    // 3. Replay agrees, twice.
    for i in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 139, "replay {i} must reproduce the crash. stderr:\n{}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "replay {i} stdout must be byte-identical");
        assert!(!rp.stderr.contains("DIVERGENCE"), "replay {i} diverged:\n{}", rp.stderr);
    }

    // 4. THE demo: run to the crash, watch the cell the faulting load read the pointer from, run
    //    BACKWARD to its last writer, and prove it by effect: pre-retire the cell does not yet
    //    hold `target`; one stepi later it does. Symbol-free (spec R2): cast() is static in
    //    _ctypes.so.
    let ts = trace.to_str().unwrap();
    let script = format!("continue; watch 0x{cell:x}; reverse-continue; x 0x{cell:x} 8; stepi; x 0x{cell:x} 8");
    let (code, out, err) = debug_run(ts, &script);
    assert_eq!(code, 0, "debug session failed. stderr:\n{err}\nstdout:\n{out}");
    assert!(out.contains("guest crashed: pc="), "continue must park at the crash:\n{out}");
    assert!(out.contains(&format!("hit watch 0x{cell:x} (write at ")), "reverse-continue must find a writer:\n{out}");
    let target_hex = TARGET.to_le_bytes().iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
    let xs: Vec<&str> = out.lines().filter(|l| l.starts_with(&format!("0x{cell:x}:"))).collect();
    assert_eq!(xs.len(), 2, "two x dumps expected:\n{out}");
    assert!(!xs[0].contains(&target_hex), "before the store the cell is NOT yet the target:\n{out}");
    assert!(xs[1].contains(&target_hex), "after one stepi the store of the target retired:\n{out}");
}
