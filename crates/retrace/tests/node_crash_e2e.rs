//! M48 rung 10 (spec §1 part 3, §3g, D1): the real node runs a repo-owned script whose optimized
//! JavaScript stores a computed bad pointer, and an N-API addon loads through it. The run records,
//! replays bit-for-bit, and a scripted debug session reverse-continues from the crash into V8's
//! JIT code, to the store.
//!
//! cpython_crash_e2e's structure and its four assertions, each on the difference rung 10 makes
//! (CLAUDE.md honest-gate rule 1), plus the fifth that is this rung's own:
//!   1. the script RAN: its marker is in the recorded stdout, `UNREACHED` is not, and the marker's
//!      `opt=` equals native's, so TurboFan compiled `store` exactly as it does natively;
//!   2. the crash IS the deref: node's own SIGSEGV handler is delivered the fault first (it
//!      re-raises), then the terminal Event::Crash has far == the computed target, a level-1
//!      translation DFSC, and the marker's thread;
//!   3. replay agrees, twice;
//!   4. `watch <cell>; reverse-continue` from the crash lands on the store, by its effect: the cell
//!      holds the warm-up value 2 before it and the target one stepi later;
//!   5. the store's pc lies inside a MAP_JIT mapping of the recording. An interpreter store passes
//!      1–4 too (spec §4).
//!
//! The cell and the MAP_JIT ranges are DISCOVERED from the recording (the M6 marker convention).
//!
//! Neither node nor its headers are repo artifacts, so the test announces a skip naming what is
//! missing. kq_e2e, condvar_e2e, jitwp_e2e, simd_e2e and trim_e2e guard node's mechanisms without
//! it. The crash demo's trace is 1.2 GB (walls.md §2), so the test deletes it after its last
//! assertion; a failing run keeps it for diagnosis.
mod util;
use retrace_trace::Event;

const NODE: &str = "/opt/homebrew/bin/node";
const TARGET: u64 = 0x4000_DEAD_0000; // crash.json: 0x400000000000 + 0xdead0000
/// The debug session's bound, so a hang fails the gate rather than stalling it (M42). t0 M7
/// measured the session at 14 s on a release build; Task 8 measured the debug build's, and this
/// is at least twice that.
const DEBUG_SECS: u64 = 1200;

fn marker(stdout: &str) -> &str {
    stdout.lines().find(|l| l.starts_with("CRASHJS cell=0x"))
        .unwrap_or_else(|| panic!("missing the `CRASHJS cell=0x` marker in stdout:\n{stdout}"))
}

/// The value of `key` (`opt=`, `cell=0x`, …) in the marker line, up to the next space.
fn field<'a>(marker: &'a str, key: &str) -> &'a str {
    marker.split(' ').find_map(|w| w.strip_prefix(key))
        .unwrap_or_else(|| panic!("no `{key}` in the marker `{marker}`"))
}

#[test]
fn node_crashes_in_a_real_script_and_reverse_debugs_into_its_jit_code() {
    if !std::path::Path::new(NODE).exists() {
        util::announce(&format!("SKIPPED node_crashes_in_a_real_script…: {NODE} not installed \
            (`brew install node`). This gate did NOT run — it is not evidence of anything."));
        return;
    }
    let Some(addon) = retrace_guest::NODE_CRASH_ADDON else {
        util::announce("SKIPPED node_crashes_in_a_real_script…: the crash addon was not built \
            (/opt/homebrew/include/node/node_api.h not found at build time). This gate did NOT run \
            — it is not evidence of anything.");
        return;
    };
    let exe = std::fs::canonicalize(NODE).unwrap();
    let exe = exe.to_str().unwrap();
    let args = ["--allow-natives-syntax", retrace_guest::CRASH_JS, addon];

    let native = std::process::Command::new(exe).args(args).output().unwrap();
    let native_out = String::from_utf8_lossy(&native.stdout).into_owned();
    assert!(!native_out.contains("UNREACHED"), "natively the deref must fault:\n{native_out}");
    let native_opt = field(marker(&native_out), "opt=").to_owned();

    let (rec, trace) = util::record_dynamic_args(exe, &args);
    let stdout = String::from_utf8_lossy(&rec.stdout).into_owned();

    // 1. The script ran, and TurboFan compiled `store` as it does natively.
    assert!(stdout.contains("CRASHJS cell=0x"),
        "marker line missing (record exit {}). stdout:\n{stdout}\nstderr:\n{}", rec.code, rec.stderr);
    let m = marker(&stdout);
    assert_eq!(field(m, "target="), format!("{TARGET:#x}"), "the target is computed from crash.json");
    assert_eq!(field(m, "rows="), "2");
    assert_eq!(field(m, "opt="), native_opt, "TurboFan must compile `store` under retrace as natively");
    assert!(!stdout.contains("UNREACHED"), "the deref must not return. stdout:\n{stdout}");
    let cell = u64::from_str_radix(field(m, "cell=0x"), 16).expect("cell hex");

    // 2. The crash is the deref, after node's own handler saw it; and 5's ranges, from one decode.
    let evs = retrace_trace::Reader::open(&trace).unwrap();
    let jit = util::map_jit_ranges(&evs);
    let (mut marker_thread, mut delivery, mut crash) = (None, None, None);
    for (i, e) in evs.iter().enumerate() {
        match e {
            Event::Syscall { num, args, thread, .. }
                if (*num == retrace_arch::SYS_WRITE || *num == retrace_arch::SYS_WRITE_NOCANCEL) && args[0] == 1 =>
                marker_thread = Some(*thread),
            Event::SignalDelivery { sig: 11, si_addr, thread, .. } => delivery = Some((i, *si_addr, *thread)),
            Event::Crash { esr, far, thread, .. } => crash = Some((i, *esr, *far, *thread)),
            _ => {}
        }
    }
    drop(evs);
    let mthread = marker_thread.expect("a write to stdout (the marker) before the crash");
    let (ci, esr, far, cthread) = crash.expect("the trace ends in an Event::Crash");
    assert_eq!(far, TARGET, "the crash FAR must be the computed target (esr={esr:#x})");
    assert_eq!(esr & 0x3f, 0x05, "DFSC must be a level-1 translation fault (esr={esr:#x})");
    assert_eq!(cthread, mthread, "the deref must run on the marker's thread");
    let (di, si_addr, dthread) = delivery
        .expect("node's own SIGSEGV handler must be delivered the fault before the terminal crash (walls.md §2)");
    assert!(di < ci && si_addr == TARGET && dthread == cthread,
        "node's handler must see this fault first: delivery #{di} si_addr {si_addr:#x} thread \
         {dthread}, crash #{ci} thread {cthread}");
    assert_eq!(rec.code, 139, "a recorded crash exits 139 (M6). stderr:\n{}", rec.stderr);

    // 3. Replay agrees, twice.
    for i in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 139, "replay {i} must reproduce the crash. stderr:\n{}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "replay {i} stdout must be byte-identical");
        assert!(!rp.stderr.contains("DIVERGENCE"), "replay {i} diverged:\n{}", rp.stderr);
    }

    // 4. THE demo: run to the crash, watch the cell the addon read the pointer from, run BACKWARD
    //    to its last writer, and prove it by effect.
    let ts = trace.to_str().unwrap();
    let script = format!("continue; watch 0x{cell:x} 8; reverse-continue; x 0x{cell:x} 8; stepi; x 0x{cell:x} 8");
    let (code, out, err) = util::debug_bounded(ts, &script, DEBUG_SECS);
    assert_eq!(code, Some(0), "the debug session failed or hit its {DEBUG_SECS} s bound. stderr:\n{err}\nstdout:\n{out}");
    assert!(out.contains("guest crashed: pc="), "continue must park at the crash:\n{out}");
    let hit = out.lines().find(|l| l.starts_with(&format!("hit watch 0x{cell:x} (write at 0x")))
        .unwrap_or_else(|| panic!("reverse-continue must find a writer:\n{out}"));
    let pc_hex = hit.split("(write at 0x").nth(1).and_then(|r| r.split(')').next()).expect("the writer's pc");
    let pc = u64::from_str_radix(pc_hex, 16).expect("the writer's pc is hex");
    let hex = |v: u64| v.to_le_bytes().iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
    let xs: Vec<&str> = out.lines().filter(|l| l.starts_with(&format!("0x{cell:x}:"))).collect();
    assert_eq!(xs.len(), 2, "two x dumps expected:\n{out}");
    assert!(xs[0].contains(&hex(2)), "before the store the cell holds the warm-up value 2:\n{out}");
    assert!(xs[1].contains(&hex(TARGET)), "after one stepi the store of the target retired:\n{out}");

    // 5. The store is JIT code.
    assert!(jit.iter().any(|&(s, e)| (s..e).contains(&pc)),
        "the store's pc {pc:#x} must lie inside a MAP_JIT mapping {jit:x?}: an interpreter or \
         builtin store passes 1–4 too (spec §4)");
    let _ = std::fs::remove_file(&trace);
}
