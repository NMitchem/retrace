// M55 t1 gate (spec 2b, 3b-3e). A Unix stream socket pair is a guest object: `socketpair` hands out
// guest-numbered descriptors, a data model counts bytes from each call's own return, a socket end's
// kqueue filters are level-triggered by that model, and `poll` and `select` answer from it. Each mode of
// `crates/retrace-guest/c/sockpair_dyn.c` is run natively, recorded and replayed twice, and the guest's own
// lines are the assertion; every literal below is native's output, measured (t0, p0). Before M55 the
// first `socketpair` was a wall. Every run is bounded so that a regression fails rather than stalls.
mod util;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const BOUND: u64 = 120;

fn native(args: &[&str]) -> (Option<i32>, String) {
    let o = util::run_bounded(retrace_guest::SOCKPAIR_DYN, args, &[], BOUND);
    (o.code, String::from_utf8_lossy(&o.stdout).into_owned())
}

fn trace_path() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!("retrace-sockpair-{}-{}.bin", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
}

fn record(args: &[&str], bound: u64) -> (util::Bounded, PathBuf) {
    let trace = trace_path();
    let mut argv = vec!["record-dyn", retrace_guest::SOCKPAIR_DYN, "-o", trace.to_str().unwrap(), "--"];
    argv.extend_from_slice(args);
    let rec = util::run_bounded(util::bin(), &argv, &[], bound);
    (rec, trace)
}

/// Native, then record and two replays: the exit code and the whole of stdout agree in all four, and
/// nothing stops at a wall. Returns the output and the recording's path.
fn records_as_native(args: &[&str]) -> (String, PathBuf) {
    let mode = args.join(" ");
    let (ncode, nout) = native(args);
    let ncode = ncode.unwrap_or_else(|| panic!("{mode}: native hung"));
    let (rec, trace) = record(args, BOUND);
    assert!(!rec.stderr.contains("retrace: unsupported:"), "{mode}: no wall. stderr:\n{}", rec.stderr);
    let out = String::from_utf8_lossy(&rec.stdout).into_owned();
    assert_eq!((rec.code, out.as_str()), (Some(ncode), nout.as_str()), "{mode}: record against native. stderr:\n{}", rec.stderr);
    for k in 1..=2 {
        let rp = util::run_bounded(util::bin(), &["replay", trace.to_str().unwrap()], &[], BOUND);
        assert_eq!((rp.code, rp.stdout.as_slice()), (rec.code, rec.stdout.as_slice()), "{mode}: replay {k}: {}", rp.stderr);
    }
    (out, trace)
}

#[test]
fn a_socket_pair_is_numbered_like_native() {
    let (out, _) = records_as_native(&["pair"]);
    assert_eq!(out, "rc=0 d=1 lowest=1\ngetfl=0x2 getfd=0\ngetfl=0x2 getfd=0\n");
}

#[test]
fn bytes_flow_both_ways() {
    let (out, _) = records_as_native(&["flow"]);
    assert_eq!(out, "write A 3\nwrite B 3\nfionread A 3\nfionread B 3\nread B 3 abc\nread A 3 xyz\nfionread A 0\n");
}

/// Review Focus 4: the partial return (8192) must keep the counts in step, or the later reads go
/// past the model's count.
#[test]
fn a_write_past_the_buffer_returns_8192_and_reads_stay_in_step() {
    let (out, _) = records_as_native(&["big"]);
    assert_eq!(out, "write 8192\nwrite again -1 errno=35\nsum=8192 reads=9 last=-1 errno=35\nwrite after drain 100\n");
}

#[test]
fn the_peer_closing_gives_eof_then_epipe() {
    let (out, _) = records_as_native(&["eof"]);
    assert_eq!(out, "write 2\nread 2\nread 0\nwrite -1 errno=32\n");
}

#[test]
fn a_level_triggered_knote_follows_the_bytes() {
    let (out, _) = records_as_native(&["level"]);
    assert_eq!(out, "add read: n=0\n\
        add write: n=1 [A write flags=0x1 data=8192]\n\
        poll empty: n=1 [A write flags=0x1 data=8192]\n\
        poll after 1 byte: n=2 [A write flags=0x1 data=8192] [A read flags=0x1 data=1]\n\
        poll again (level): n=2 [A write flags=0x1 data=8192] [A read flags=0x1 data=1]\n\
        poll after reading it: n=1 [A write flags=0x1 data=8192]\n\
        del read: n=1 [A write flags=0x1 data=8192]\n\
        del write: n=0\n\
        peer closed: n=2 [A write flags=0x8001 data=8192] [A read flags=0x8001 data=2]\n");
}

/// T0-poll: the table, bit by bit. A closed peer reports POLLHUP and every requested read-ish bit and
/// no write bit; `events` 0 reports nothing.
#[test]
fn poll_reports_each_socket_state_as_native() {
    let (out, _) = records_as_native(&["poll"]);
    assert_eq!(out, "\
empty: 0x1=0/0 0x2=0/0 0x4=1/0x4 0x40=0/0 0x4=1/0x4 0x80=0/0 0x100=1/0x100 0=0/0 0x1c7=1/0x104\n\
readable: 0x1=1/0x1 0x2=0/0 0x4=1/0x4 0x40=1/0x40 0x4=1/0x4 0x80=0/0 0x100=1/0x100 0=0/0 0x1c7=1/0x145\n\
outfull: 0x1=0/0 0x2=0/0 0x4=0/0 0x40=0/0 0x4=0/0 0x80=0/0 0x100=0/0 0=0/0 0x1c7=0/0\n\
readable+outfull: 0x1=1/0x1 0x2=0/0 0x4=0/0 0x40=1/0x40 0x4=0/0 0x80=0/0 0x100=0/0 0=0/0 0x1c7=1/0x41\n\
closed+bytes: 0x1=1/0x11 0x2=1/0x12 0x4=1/0x10 0x40=1/0x50 0x4=1/0x10 0x80=1/0x90 0x100=1/0x10 0=0/0 0x1c7=1/0xd3\n\
closed+drained: 0x1=1/0x11 0x2=1/0x12 0x4=1/0x10 0x40=1/0x50 0x4=1/0x10 0x80=1/0x90 0x100=1/0x10 0=0/0 0x1c7=1/0xd3\n");
}

/// T0-select: a closed peer makes the write set ready, where `poll` has no write bit.
#[test]
fn select_reports_each_socket_state_as_native() {
    let (out, _) = records_as_native(&["select"]);
    assert_eq!(out, "\
empty: read=0 write=1 except=0 all=1\n\
readable: read=1 write=1 except=0 all=2\n\
outfull: read=0 write=0 except=0 all=0\n\
readable+outfull: read=1 write=0 except=0 all=1\n\
closed+bytes: read=1 write=1 except=0 all=2\n\
closed+drained: read=1 write=1 except=0 all=2\n");
}

/// Review Focus 1: a socket end moved onto fd 1 and written must be counted and reach the peer, not be
/// taken as a console write (which would print the byte on the console and never reach the peer).
#[test]
fn a_socket_moved_onto_stdout_is_not_a_console_write() {
    let (out, _) = records_as_native(&["dup2console"]);
    assert_eq!(out, "wrote 1 read 1 byte=x\nwrote 2 fionread 2\n");
}

/// Review Focus 2: exec's close-on-exec closes the end, and the peer sees EOF as natively.
#[test]
fn an_exec_closing_a_socket_end_gives_the_peer_eof() {
    let (out, _) = records_as_native(&["execeof"]);
    assert_eq!(out, "exec\nafter exec: poll n=1 hup=1 in=1\n");
}

#[test]
fn refused_shapes_answer_natives_errno() {
    let (out, _) = records_as_native(&["shapes"]);
    assert_eq!(out, "\
domain=1 type=5 proto=0: rc=-1 errno=43\n\
domain=1 type=3 proto=0: rc=-1 errno=43\n\
domain=1 type=1 proto=6: rc=-1 errno=43\n\
domain=2 type=1 proto=0: rc=-1 errno=102\n\
domain=30 type=1 proto=0: rc=-1 errno=102\n\
domain=99 type=1 proto=0: rc=-1 errno=47\n\
unwritable sv: rc=-1 errno=14 lowest_unchanged=1\n");
}

#[test]
fn a_datagram_pair_is_refused_by_value() {
    let (rec, _) = record(&["dgram"], BOUND);
    assert_eq!(rec.code, Some(4), "the recorder stops at the wall. stderr:\n{}", rec.stderr);
    assert!(rec.stderr.contains("retrace: unsupported: M55: socketpair of SOCK_DGRAM"), "{}", rec.stderr);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), "", "nothing ran past the call");
}

/// Spec 3c: each call outside the model, after a good pair, ends at one wall line naming the call, with
/// native's output up to it printed first.
#[test]
fn calls_outside_the_model_are_refused_by_value() {
    for (which, call) in [("shutdown", "shutdown"), ("rights", "sendmsg"), ("sndbuf", "setsockopt"), ("msgx", "sendmsg_x")] {
        let (rec, _) = record(&["refusals", which], BOUND);
        assert_eq!(rec.code, Some(4), "{which}: the recorder stops at the wall. stderr:\n{}", rec.stderr);
        let lines: Vec<&str> = rec.stderr.lines().filter(|l| l.starts_with("retrace: unsupported: ")).collect();
        assert_eq!(lines.len(), 1, "{which}: {}", rec.stderr);
        assert!(lines[0].contains("M55: ") && lines[0].contains(call) && lines[0].contains("is not modelled"), "{which}: {}", lines[0]);
        assert_eq!(String::from_utf8_lossy(&rec.stdout), "pair ok\n", "{which}: native's output up to the call");
    }
}

/// A pair made on a worker thread is tagged with the worker's id (the retag lives in `thread_oracle`).
#[test]
fn a_pair_made_on_a_worker_thread_records_and_replays() {
    let (out, _) = records_as_native(&["worker"]);
    assert_eq!(out, "worker pair d=1 lowest=1\nworker read 2 hi\nmain joined\n");
}
