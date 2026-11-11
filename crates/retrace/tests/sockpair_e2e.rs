// M55 t1 gate (spec 2b, 3b-3e). A Unix stream socket pair is a guest object: `socketpair` hands out
// guest-numbered descriptors, a data model counts bytes from each call's own return, a socket end's
// kqueue filters are level-triggered by that model, and `poll` and `select` answer from it. Each mode of
// `crates/retrace-guest/c/sockpair_dyn.c` is run natively, recorded and replayed twice, and the guest's own
// lines are the assertion; every literal below is native's output, measured (t0, p0). Before M55 the
// first `socketpair` was a wall. Every run is bounded so that a regression fails rather than stalls.
mod util;

use retrace_core::ReplaySession;
use retrace_trace::Event;
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
    let mut argv = vec!["record", "-o", trace.to_str().unwrap(), retrace_guest::SOCKPAIR_DYN];
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

/// M55 t4 (spec §3f): the same with SIGPIPE at its default disposition: the last write ends the process, as
/// native, with 141 and the output flushed before it. The recording ends at a terminal `Event::Signal`.
#[test]
fn the_default_sigpipe_on_a_socket_ends_the_process_with_141() {
    let (ncode, nout) = native(&["eofdfl"]);
    assert_eq!(ncode, Some(141), "native dies of SIGPIPE");
    let (rec, trace) = record(&["eofdfl"], BOUND);
    assert_eq!((rec.code, String::from_utf8_lossy(&rec.stdout).into_owned()), (Some(141), nout.clone()), "stderr:\n{}", rec.stderr);
    assert_eq!(nout, "write 2\nread 2\nread 0\n");
    for k in 1..=2 {
        let rp = util::run_bounded(util::bin(), &["replay", trace.to_str().unwrap()], &[], BOUND);
        assert_eq!((rp.code, rp.stdout.as_slice()), (Some(141), rec.stdout.as_slice()), "replay {k}: {}", rp.stderr);
    }
    let ev = retrace_trace::Reader::open(&trace).unwrap();
    let last = ev.iter().rev().find(|e| !matches!(e, Event::Snapshot { .. })).unwrap();
    assert!(matches!(last, Event::Signal { sig: 13, .. }), "the terminal is SIGPIPE: {last:?}");
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
    assert!(rec.stderr.contains("retrace: unsupported: socketpair of SOCK_DGRAM"), "{}", rec.stderr);
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
        assert!(lines[0].contains("") && lines[0].contains(call) && lines[0].contains("is not modelled"), "{which}: {}", lines[0]);
        assert_eq!(String::from_utf8_lossy(&rec.stdout), "pair ok\n", "{which}: native's output up to the call");
    }
}

/// F-1: `MSG_NOSIGNAL` on `send` and `sendto` answers EPIPE and raises nothing, as native does (the review's
/// probe: before the fix record and replay ended with "killed by SIGPIPE", exit 141).
#[test]
fn msg_nosignal_suppresses_sigpipe_as_native() {
    let (out, _) = records_as_native(&["nosignal"]);
    assert_eq!(out, "send MSG_NOSIGNAL rc=-1 errno=32\nsendto MSG_NOSIGNAL rc=-1 errno=32\n");
}

/// F-5: a plain `sendmsg` on a guest socket end is a wall, never the EFAULT its untranslated iovec gave
/// (native sends 5 bytes).
#[test]
fn a_plain_sendmsg_on_a_socket_is_refused_by_value() {
    let (rec, _) = record(&["refusals", "plainmsg"], BOUND);
    assert_eq!(rec.code, Some(4), "stderr:\n{}", rec.stderr);
    let lines: Vec<&str> = rec.stderr.lines().filter(|l| l.starts_with("retrace: unsupported: ")).collect();
    assert_eq!(lines.len(), 1, "{}", rec.stderr);
    assert!(lines[0].contains("") && lines[0].contains("sendmsg") && lines[0].contains("is not modelled"), "{}", lines[0]);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), "pair ok\n");
}

/// A pair made on a worker thread is tagged with the worker's id (the retag lives in `thread_oracle`).
#[test]
fn a_pair_made_on_a_worker_thread_records_and_replays() {
    let (out, _) = records_as_native(&["worker"]);
    assert_eq!(out, "worker pair d=1 lowest=1\nworker read 2 hi\nmain joined\n");
}

// ---- M55 t2: EV_CLEAR, EV_RECEIPT and error events (spec 2b, 2c, 3d; t0 T0-receipt, T0-clear-oneshot) ----

/// Spec 2b: an `EV_CLEAR` knote on an end is queued by the peer's write (read), the peer's read (write) and
/// the peer's last close (both, `EV_EOF`, write first), and never by the end's own calls or a non-last close.
#[test]
fn an_edge_triggered_socket_knote_fires_on_the_peers_calls_only() {
    let (out, _) = records_as_native(&["clearsock"]);
    assert_eq!(out, concat!(
        "registered: n=1 [+0 write flags=0x21 data=8192 u=0]\n",
        "A writes 1 byte: n=0\n",
        "B writes 1 byte to A: n=1 [+0 read flags=0x21 data=1 u=0]\n",
        "A reads it: n=0\n",
        "B reads A's byte: n=1 [+0 write flags=0x21 data=8192 u=0]\n",
        "dup+close of B (not the last): n=0\n",
        "last close of B: n=2 [+0 write flags=0x8021 data=8192 u=0] [+0 read flags=0x8021 data=0 u=0]\n",
    ));
}

/// `p0/sockprobe.txt`: a read edge per write whatever is unread, no edge from a read or a drain, and a write
/// edge only when a peer read leaves at least the 2048-byte low-water mark free, again with each later read.
#[test]
fn edge_triggered_read_and_write_follow_natives_edges() {
    let (out, _) = records_as_native(&["clearedges"]);
    assert_eq!(out, concat!(
        "registered: n=1 [+0 write flags=0x21 data=8192 u=0]\n",
        "after 2 bytes: n=1 [+0 read flags=0x21 data=2 u=0]\n",
        "again, 2 unread: n=0\n",
        "after 1 more byte: n=1 [+0 read flags=0x21 data=3 u=0]\n",
        "after reading 1: n=0\n",
        "after draining: n=0\n",
        "after fill: n=0\n",
        "peer read 1: n=0\n",
        "peer read 1023 more: n=0\n",
        "peer read 2048 more: n=1 [+0 write flags=0x21 data=3072 u=0]\n",
        "peer read the rest: n=1 [+0 write flags=0x21 data=8192 u=0]\n",
        "peer drained: n=0\n",
    ));
}

/// Spec 2c: any read or write of a pipe activates both ends' knotes, each only while its filter holds (a full
/// pipe fires read alone, a drained one write alone), write first; the last write-end close fires read, `EV_EOF`.
#[test]
fn an_edge_triggered_pipe_knote_fires_on_any_read_or_write() {
    let (out, _) = records_as_native(&["clearpipe"]);
    assert_eq!(out, concat!(
        "add read: n=0\n",
        "add write: n=1 [+1 write flags=0x21 data=16384 u=0]\n",
        "empty: n=0\n",
        "again: n=0\n",
        "after 2 bytes: n=2 [+1 write flags=0x21 data=16382 u=0] [+0 read flags=0x21 data=2 u=0]\n",
        "again: n=0\n",
        "after 1 more: n=2 [+1 write flags=0x21 data=16381 u=0] [+0 read flags=0x21 data=3 u=0]\n",
        "after reading 1: n=2 [+1 write flags=0x21 data=16382 u=0] [+0 read flags=0x21 data=2 u=0]\n",
        "after fill: n=1 [+0 read flags=0x21 data=65536 u=0]\n",
        "after reading 512: n=2 [+1 write flags=0x21 data=512 u=0] [+0 read flags=0x21 data=65024 u=0]\n",
        "drained: n=1 [+1 write flags=0x21 data=65536 u=0]\n",
        "write end closed: n=1 [+0 read flags=0x8021 data=0 u=0]\n",
    ));
}

/// Spec 2c: each change carrying `EV_RECEIPT` is one event (`EV_ERROR` set, `data` 0 or the errno); a call that
/// made any returns them without scanning, the same array serving as both lists; `nevents` 0 drops them.
#[test]
fn receipts_answer_each_change_and_return_without_scanning() {
    let (out, _) = records_as_native(&["receipt"]);
    assert_eq!(out, concat!(
        "register read+write, one array, NULL timeout: n=2 [+0 read flags=0x4061 data=0 u=7] [+0 write flags=0x4061 data=0 u=8]\n",
        "poll after the receipts: n=1 [+0 write flags=0x61 data=8192 u=8]\n",
        "poll again: n=0\n",
        "delete registered, delete 999, add 998: n=3 [+0 read flags=0x4042 data=0 u=0] [#999 read flags=0x4042 data=2 u=0] [#998 read flags=0x4041 data=9 u=0]\n",
        "re-add read: n=1 [+0 read flags=0x4061 data=0 u=0]\n",
        "receipt with nevents 0: n=0\n",
        "poll after it: n=0\n",
    ));
}

/// T0-receipt: changes apply in order; a receipt with no room is dropped, a failing change with no room ends the
/// call with -1 and its errno (the earlier changes applied, the later ones not), and with room it stores its
/// `EV_ERROR` event and the call goes on. Also T0-clear-oneshot: `EV_CLEAR|EV_ONESHOT` on a socket read is a one-shot.
#[test]
fn receipt_overflow_is_natives() {
    let (out, _) = records_as_native(&["receiptcorner"]);
    assert_eq!(out, concat!(
        "receipt bad third nevents=1: n=-1 errno=9 [+0 read flags=0x4041 data=0 u=1]\n",
        "  later trigger: n=2 [+0 read flags=0x41 data=1 u=1] [+2 read flags=0x41 data=1 u=2]\n",
        "receipt bad third nevents=2: n=-1 errno=9 [+0 read flags=0x4041 data=0 u=1] [+2 read flags=0x4041 data=0 u=2]\n",
        "  later trigger: n=2 [+0 read flags=0x41 data=1 u=1] [+2 read flags=0x41 data=1 u=2]\n",
        "receipt bad third nevents=3: n=3 [+0 read flags=0x4041 data=0 u=1] [+2 read flags=0x4041 data=0 u=2] [#999 read flags=0x4041 data=9 u=3]\n",
        "  later trigger: n=2 [+0 read flags=0x41 data=1 u=1] [+2 read flags=0x41 data=1 u=2]\n",
        "receipt bad first nevents=1: n=1 [#999 read flags=0x4041 data=9 u=1]\n",
        "  later trigger: n=2 [+0 read flags=0x41 data=1 u=2] [+2 read flags=0x41 data=1 u=3]\n",
        "receipt bad first nevents=2: n=2 [#999 read flags=0x4041 data=9 u=1] [+0 read flags=0x4041 data=0 u=2]\n",
        "  later trigger: n=2 [+0 read flags=0x41 data=1 u=2] [+2 read flags=0x41 data=1 u=3]\n",
        "receipt bad first nevents=3: n=3 [#999 read flags=0x4041 data=9 u=1] [+0 read flags=0x4041 data=0 u=2] [+2 read flags=0x4041 data=0 u=3]\n",
        "  later trigger: n=2 [+0 read flags=0x41 data=1 u=2] [+2 read flags=0x41 data=1 u=3]\n",
        "plain bad first nevents=0: n=-1 errno=9\n",
        "  later trigger: n=0\n",
        "plain delete bad first nevents=0: n=-1 errno=2\n",
        "  later trigger: n=0\n",
        "plain bad first nevents=1: n=1 [#999 read flags=0x4001 data=9 u=1]\n",
        "  later trigger: n=2 [+0 read flags=0x1 data=1 u=2] [+2 read flags=0x1 data=1 u=3]\n",
        "plain delete bad first nevents=1: n=1 [#999 read flags=0x4002 data=2 u=1]\n",
        "  later trigger: n=2 [+0 read flags=0x1 data=1 u=2] [+2 read flags=0x1 data=1 u=3]\n",
        "receipt bad first nevents=0: n=-1 errno=9\n",
        "  later trigger: n=0\n",
        "receipt delete bad first nevents=0: n=-1 errno=2\n",
        "  later trigger: n=0\n",
        "receipt bad first nevents=1: n=1 [#999 read flags=0x4041 data=9 u=1]\n",
        "  later trigger: n=2 [+0 read flags=0x41 data=1 u=2] [+2 read flags=0x41 data=1 u=3]\n",
        "receipt delete bad first nevents=1: n=1 [#999 read flags=0x4042 data=2 u=1]\n",
        "  later trigger: n=2 [+0 read flags=0x41 data=1 u=2] [+2 read flags=0x41 data=1 u=3]\n",
        "oneshot register: n=1 [+0 read flags=0x4071 data=0 u=0]\n",
        "after the peer's write: n=1 [+0 read flags=0x71 data=1 u=0]\n",
        "after a second write: n=0\n",
        "delete after delivery: n=1 [+0 read flags=0x4042 data=2 u=0]\n",
        "register when readable: n=1 [+0 read flags=0x31 data=2 u=0]\n",
        "then poll: n=0\n",
    ));
}

/// Spec 2c: without `EV_RECEIPT` an error is still an `EV_ERROR` event, `ENOENT` for a delete of a knote that is
/// not registered even on a descriptor not open, and -1 with the errno when `nevents` is 0; a receipt beside a
/// plain add returns alone, unscanned.
#[test]
fn errors_without_receipt_are_error_events() {
    let (out, _) = records_as_native(&["errnoreceipt"]);
    assert_eq!(out, concat!(
        "3 receipts, nevents 1: n=-1 errno=9 [+0 read flags=0x4041 data=0 u=11]\n",
        "then: n=1 [+0 write flags=0x41 data=8192 u=12]\n",
        "no receipt: delete registered, delete 996, nevents 3: n=1 [#996 read flags=0x4002 data=2 u=0]\n",
        "no receipt: delete 996, nevents 0: n=-1 errno=2\n",
        "mixed: plain add read (data waiting), receipt delete write: n=1 [+0 write flags=0x4042 data=0 u=0]\n",
    ));
}

/// Review Focus 5: closing the descriptor a knote was registered on drops the knote even with a dup keeping the
/// end open, so the peer's write queues nothing; the dup's own knote, added after the peer's last close, sees
/// `EV_EOF`.
#[test]
fn a_knote_on_a_closed_socket_end_is_dropped() {
    let (out, _) = records_as_native(&["knoteclose"]);
    assert_eq!(out, concat!(
        "closed A (a dup kept), B writes: n=0\n",
        "B closed, the dup's knote added: n=1 [+2 read flags=0x8001 data=1 u=0]\n",
    ));
}

fn events(trace: &PathBuf) -> Vec<Event> { retrace_trace::Reader::open(trace).unwrap() }

/// Review Focus 3: a checkpoint taken in the middle of `clearedges`'s socket traffic, continued to the last
/// landmark, must equal a cold seek there: the socket model and the knotes (box state), registers, saved
/// contexts and memory. A model `BoxState` did not carry would put the first edge after the checkpoint in the
/// wrong place, or panic on a socket it had never heard of.
#[test]
fn a_seek_into_socket_traffic_continues_as_recorded() {
    let (_, trace) = records_as_native(&["clearedges"]);
    let evs = events(&trace);
    let kevents: Vec<usize> = evs.iter().enumerate()
        .filter_map(|(i, e)| matches!(e, Event::Syscall { num: 363, .. }).then_some(i)).collect();
    assert!(kevents.len() >= 10, "clearedges polls many times: {}", kevents.len());
    let mid = kevents[kevents.len() / 2] + 1;
    let last = evs.iter().rposition(|e| matches!(e, Event::Syscall { .. })).expect("a last syscall") + 1;
    assert!(mid < last);
    // One VM per process: each session ends before the next is built.
    let cp = {
        let from = retrace_core::seek(&trace, mid, 0).unwrap();
        let state = from.dbg_internal_state();
        assert!(state.contains("socks"), "the checkpoint carries the socket model:\n{state}");
        from.checkpoint()
    };
    let warm = {
        let mut s = ReplaySession::from_checkpoint(&trace, &cp).unwrap();
        s.advance_to_landmark(last).unwrap_or_else(|d| panic!("warm: diverged at {}: {}", d.landmark, d.detail));
        (s.current_thread(), s.dbg_regs(), s.dbg_internal_state(), s.snapshot().1)
    };
    let cold = retrace_core::seek(&trace, last, 0).unwrap();
    assert_eq!(warm.0, cold.current_thread(), "thread: checkpointed vs cold");
    assert_eq!(warm.1, cold.dbg_regs(), "registers: checkpointed vs cold");
    assert_eq!(warm.2, cold.dbg_internal_state(), "box state: checkpointed vs cold");
    assert!(cold.diff_memory(&warm.3).is_none(), "memory: checkpointed vs cold");
}
