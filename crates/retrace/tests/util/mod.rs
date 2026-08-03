// Shared test helper module: each test binary `mod util;`s it but uses only the subset of helpers
// it needs, so some are legitimately unused per-binary.
#![allow(dead_code)]
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

// `.cargo/config.toml`'s `runner` ad-hoc codesigns the binary cargo invokes
// directly (the test harness) with the hypervisor entitlement, but CARGO_BIN_EXE_retrace
// is a separate binary that this test spawns itself — it never passes through
// that runner. Every executable that calls hv_* needs the entitlement (see
// tools/codesign-run.sh), so sign it here the same way before exec'ing it.
pub fn bin() -> &'static str {
    let p = env!("CARGO_BIN_EXE_retrace");
    let ent = concat!(env!("CARGO_MANIFEST_DIR"), "/../../retrace.entitlements");
    let out = Command::new("codesign")
        .args(["-s", "-", "-f", "--entitlements", ent, p])
        .output()
        .expect("codesign");
    assert!(out.status.success(), "codesign -f --entitlements failed for {p}: {}", String::from_utf8_lossy(&out.stderr));
    p
}

pub struct RunOut { pub code: i32, pub stdout: Vec<u8>, pub stderr: String }

fn run(args: &[&str]) -> RunOut {
    let out = Command::new(bin()).args(args).output().unwrap();
    RunOut {
        code: out.status.code().unwrap_or(-1),
        stdout: out.stdout,
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

// Record `guest` into a fresh trace file in the system tempdir; a monotonic counter (plus
// this process's pid) keeps concurrent/repeated calls within one test binary from colliding.
pub fn record(guest: &str) -> (RunOut, std::path::PathBuf) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let trace = std::env::temp_dir().join(format!("retrace-util-{}-{n}.bin", std::process::id()));
    let out = run(&["record", guest, "-o", trace.to_str().unwrap()]);
    (out, trace)
}

pub fn replay(trace: &std::path::Path) -> RunOut {
    run(&["replay", trace.to_str().unwrap()])
}

// Record a dynamically-linked guest through real dyld via the CLI's `record-dyn` path.
pub fn record_dynamic(guest: &str) -> (RunOut, std::path::PathBuf) {
    static NEXT: AtomicU64 = AtomicU64::new(1_000_000);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let trace = std::env::temp_dir().join(format!("retrace-dyn-{}-{n}.bin", std::process::id()));
    let out = run(&["record-dyn", guest, "-o", trace.to_str().unwrap()]);
    (out, trace)
}

// Record a dynamically-linked guest through real dyld, passing `args` as the guest's argv[1..].
// argv[0] is supplied by the CLI (the guest path), so `args` is exactly what the guest sees past it.
pub fn record_dynamic_args(guest: &str, args: &[&str]) -> (RunOut, std::path::PathBuf) {
    static NEXT: AtomicU64 = AtomicU64::new(2_000_000);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let trace = std::env::temp_dir().join(format!("retrace-argv-{}-{n}.bin", std::process::id()));
    let mut argv = vec!["record-dyn", guest, "-o", trace.to_str().unwrap()];
    if !args.is_empty() { argv.push("--"); argv.extend_from_slice(args); }
    (run(&argv), trace)
}

/// (&g.st, &g.ptr) of the crashy.c fixture, discovered from the recorded marker convention —
/// see c/crashy.c's header comment. Shared by crashy_e2e / watch_dyn / crashy_cli.
pub fn discover_crashy_addrs(trace: &std::path::Path) -> (u64, u64) {
    let events = retrace_trace::Reader::open(trace).unwrap();
    let mut it = events.iter().filter_map(|e| match e {
        retrace_trace::Event::Syscall { num: 4, args, .. } if args[0] == 1 => Some(*args),
        _ => None,
    });
    while let Some(a) = it.next() {
        if a[2] == 7 { // the "CRASHY:" marker write
            let st = it.next().expect("&g.st reveal write")[1];
            let ptr = it.next().expect("&g.ptr reveal write")[1];
            return (st, ptr);
        }
    }
    panic!("CRASHY: marker write not found in trace");
}

/// What a breadth-ladder rung guest yielded once it PROVED IT RAN.
pub struct RungOut { pub trace: std::path::PathBuf, pub stdout: Vec<u8> }

/// The breadth-ladder rung assertion: record `guest` through real dyld with `argv` as its
/// arguments (`&[]` for none), then replay it twice.
///
/// Demands a **clean exit 0 with exactly `expect_stdout`** — not merely that record and replay
/// agree. M6's convention makes a recorded crash a successful recording and a verified crash replay
/// a successful replay (both exit 139), so an agreement-only check is satisfied by a guest that died
/// inside dyld having executed none of its own code. `code == 0` is the discriminator: under M6 a
/// crash outcome always exits 139, so only a guest that reached its own `exit(0)` can pass, and the
/// stdout equality proves it got far enough to produce output.
///
/// Panics with a diagnostic on any failure — it is an assertion helper, and `tests/rung.rs` pins
/// both polarities.
pub fn assert_rung_records_and_replays(guest: &str, argv: &[&str], expect_stdout: &[u8]) -> RungOut {
    let (rec, trace) = record_dynamic_args(guest, argv);
    assert_eq!(rec.code, 0,
        "rung guest must reach a clean exit(0); 139 means it CRASHED (M6 records that as a \
         successful recording, which is exactly what this assertion exists to reject). stderr:\n{}",
        rec.stderr);
    assert_eq!(rec.stdout, expect_stdout,
        "rung guest stdout mismatch — did it reach main? got {:?}, want {:?}",
        String::from_utf8_lossy(&rec.stdout), String::from_utf8_lossy(expect_stdout));
    for i in 0..2 {
        let rep = replay(&trace);
        assert_eq!(rep.code, 0, "replay {i} must exit 0. stderr:\n{}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "replay {i} stdout diverged from the recording");
    }
    RungOut { trace, stdout: rec.stdout }
}

/// Record `guest` TWICE and assert the two traces are byte-identical.
///
/// The second oracle. The replay divergence oracle compares a replay against ONE recording, so it is
/// structurally blind to a nondeterministic value entering the trace: the recording captures it once
/// and replay faithfully reproduces it forever. This one compares two RECORDINGS.
///
/// **Only valid for freestanding (`-nostdlib -static`) guests** — no clock, no entropy, no libmalloc,
/// no mach ports. Recordings of dyld/libSystem guests are NOT reproducible run-to-run, because
/// `gettimeofday` and `getentropy` are forwarded to the host and a libSystem polling loop takes a
/// different number of iterations per run (measured: `hello_dyn` traces differ structurally, by a
/// varying number of events, every time). That is accepted per-trace nondeterminism (M2-cpuid) and
/// does not threaten replay determinism, which the divergence oracle enforces per trace — but it does
/// put dyld guests out of this oracle's reach until both syscalls are synthesized.
pub fn assert_trace_reproducible(guest: &str) {
    let (r1, t1) = record(guest);
    assert_eq!(r1.code, 0, "first recording of {guest} failed: {}", r1.stderr);
    let (r2, t2) = record(guest);
    assert_eq!(r2.code, 0, "second recording of {guest} failed: {}", r2.stderr);
    assert_eq!(r1.stdout, r2.stdout, "stdout differed between two recordings of {guest}");
    let (b1, b2) = (std::fs::read(&t1).expect("read trace 1"), std::fs::read(&t2).expect("read trace 2"));
    if b1 != b2 {
        let at = b1.iter().zip(b2.iter()).position(|(x, y)| x != y);
        panic!("two recordings of {guest} produced different traces (lengths {} vs {}, first byte \
                difference at {:?}) — a nondeterministic value is entering the trace",
               b1.len(), b2.len(), at);
    }
}
