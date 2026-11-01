// M55 t4 gate (spec §3g; t0 `p0/blockread.c`, T0-blockwrite, T0-nonblock). A blocking read or write on a guest
// pipe or socket end that would wait for another thread cannot be modelled (the forwarded call blocks the
// recorder's one thread, so the worker that could end the wait never runs). Before M55 it hung the
// recorder; now it is a wall, with the output before it printed. A call that completes at once, or that is
// nonblocking, records as native. Each mode of `crates/retrace-guest/c/blockio_dyn.c` is bounded, so that a
// regression fails by its timeout rather than stalling the gate.
mod util;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// The bound on a wall test: a hang must fail it. A recording that walls takes about a second.
const WALL_BOUND: u64 = 30;
const BOUND: u64 = 120;

fn native(mode: &str) -> (Option<i32>, String) {
    let o = util::run_bounded(retrace_guest::BLOCKIO_DYN, &[mode], &[], BOUND);
    (o.code, String::from_utf8_lossy(&o.stdout).into_owned())
}

fn trace_path() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!("retrace-blockio-{}-{}.bin", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
}

fn record(mode: &str, bound: u64) -> (util::Bounded, PathBuf) {
    let trace = trace_path();
    let rec = util::run_bounded(util::bin(),
        &["record-dyn", retrace_guest::BLOCKIO_DYN, "-o", trace.to_str().unwrap(), "--", mode], &[], bound);
    (rec, trace)
}

/// Native, then record and two replays: the exit code and the whole of stdout agree in all four and nothing
/// stops at a wall. Returns the output.
fn records_as_native(mode: &str) -> String {
    let (ncode, nout) = native(mode);
    let ncode = ncode.unwrap_or_else(|| panic!("{mode}: native hung"));
    let (rec, trace) = record(mode, BOUND);
    assert!(!rec.stderr.contains("retrace: unsupported:"), "{mode}: no wall. stderr:\n{}", rec.stderr);
    let out = String::from_utf8_lossy(&rec.stdout).into_owned();
    assert_eq!((rec.code, out.as_str()), (Some(ncode), nout.as_str()), "{mode}: record against native. stderr:\n{}", rec.stderr);
    for k in 1..=2 {
        let rp = util::run_bounded(util::bin(), &["replay", trace.to_str().unwrap()], &[], BOUND);
        assert_eq!((rp.code, rp.stdout.as_slice()), (rec.code, rec.stdout.as_slice()), "{mode}: replay {k}: {}", rp.stderr);
    }
    out
}

/// A mode that ends at a wall: bounded at 30 s, exit 4, the wall line naming `says`, and the output the
/// program printed before it.
fn is_a_wall(mode: &str, says: &str, printed: &str) {
    let (rec, _) = record(mode, WALL_BOUND);
    assert!(rec.code.is_some(), "{mode}: the recording hung (it must stop at a wall). stderr:\n{}", rec.stderr);
    assert!(rec.stderr.contains(&format!("retrace: unsupported: {says}")), "{mode}: the wall names the call. stderr:\n{}", rec.stderr);
    assert_eq!(rec.code, Some(4), "{mode}: a wall exits 4. stderr:\n{}", rec.stderr);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), printed, "{mode}: the output before the wall is printed");
}

#[test]
fn a_blocking_read_on_an_empty_pipe_is_a_wall_not_a_hang() {
    is_a_wall("pipe", "M55: read on guest pipe fd", "start\n");
}

#[test]
fn a_blocking_read_on_an_empty_socket_is_a_wall_not_a_hang() {
    is_a_wall("sock", "M55: read on guest socket fd", "start\n");
}

#[test]
fn a_blocking_write_that_cannot_complete_is_a_wall() {
    // T0-blockwrite: an empty pipe completes 65536 at once, an empty socket direction 8192.
    is_a_wall("pipewrite", "M55: write of 70000 bytes on guest pipe fd", "start\n");
    is_a_wall("sockwrite", "M55: write of 10000 bytes on guest socket fd", "start\n");
}

#[test]
fn clearing_o_nonblock_makes_the_read_a_wall_again() {
    is_a_wall("cleared", "M55: read on guest pipe fd", "start\n");
}

#[test]
fn a_nonblocking_read_answers_eagain_and_records() {
    // F_SETFL(O_NONBLOCK) answers EAGAIN (35) and does not wall; FIONBIO clears and sets the bit again.
    let out = records_as_native("nonblock");
    assert_eq!(out, "nonblock read r=-1 errno=35\nafter a write r=1 c=k\ncleared getfl nonblock=0\nfionbio read r=-1 errno=35\n");
}

#[test]
fn msg_dontwait_answers_eagain_and_records() {
    let out = records_as_native("dontwait");
    assert_eq!(out, "dontwait r=-1 errno=35\n");
}

#[test]
fn a_blocking_write_that_fits_records_as_native() {
    // Exactly the capacities T0-blockwrite measured: 65536 into an empty pipe, 8192 into an empty socket. A
    // model that used the kqueue's 16384 for the pipe would wall the first.
    let out = records_as_native("fitswrite");
    assert_eq!(out, "pipe write 65536\nsock write 8192\n");
}

/// The would-block test's capacities are not the kqueue model's: a pipe reports 16384 free to `EVFILT_WRITE`
/// yet completes 65536, so the two constants disagree on purpose (T0-blockwrite).
#[test]
fn the_would_block_capacity_is_not_the_kqueue_models() {
    assert_eq!(retrace_box::gkq::PIPE_SIZE, 16384);
    assert_eq!(retrace_box::gkq::PIPE_WRITE_CAPACITY, 65536);
    assert_eq!(retrace_box::gkq::SOCK_CAPACITY, 8192);
}
