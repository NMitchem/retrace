// M10 t5. Replay recomputes the guest fd the deterministic allocator would produce and rejects a
// recording that disagrees.
//
// This test exists because a passing replay proves nothing about the mirror on its own: if the
// recompute were vacuous (never reached, or compared against itself) every replay would still pass.
// Tampering the recorded return is what makes the oracle demonstrate it can fail.
use std::path::PathBuf;

fn record_fileio() -> PathBuf {
    let bytes = std::fs::read(retrace_guest::FILEIO).unwrap();
    let loaded = retrace_guest::parse_macho(&bytes);
    let p = std::env::temp_dir().join(format!("retrace-fdreplay-{}.bin", std::process::id()));
    retrace_core::record(&loaded, &p).expect("record");
    p
}

#[test]
fn the_recording_holds_guest_fds_not_host_fds() {
    // The M10 property, stated against the trace format itself: an fd-producing syscall's recorded
    // return must be a GUEST descriptor. Pre-M10 this was whatever the host handed back — 17+ in a
    // process that holds 0-16 open — which made the trace a function of the recorder.
    let trace = record_fileio();
    let events = retrace_trace::Reader::open(&trace).unwrap();
    let mut saw_open = false;
    for e in events.iter() {
        if let retrace_trace::Event::Syscall { num, ret, err, .. } = e {
            if !*err && retrace_arch::allocates_fd(*num) {
                saw_open = true;
                assert!(*ret >= 3 && *ret < 16,
                    "syscall {num} recorded fd {ret}: an fd >= 16 is a HOST descriptor leaking into \
                     the trace, which is exactly what M10's table exists to prevent");
            }
        }
    }
    assert!(saw_open, "the fileio guest must open something, or this test proves nothing");
}

#[test]
fn a_recorded_host_shaped_fd_is_caught_as_divergence() {
    let trace = record_fileio();
    // Rewrite the recorded open() return to 17 — a host-shaped fd, and precisely what a pre-M10
    // recording contained. Replay's allocator yields 3, so the mirror must reject it.
    let mut events = retrace_trace::Reader::open(&trace).unwrap();
    let mut tampered = false;
    for e in events.iter_mut() {
        if let retrace_trace::Event::Syscall { num, ret, err, .. } = e {
            if !*err && retrace_arch::allocates_fd(*num) && !tampered {
                *ret = 17;
                tampered = true;
            }
        }
    }
    assert!(tampered, "no fd-producing syscall found to tamper");
    let mut w = retrace_trace::Writer::create(&trace).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);

    let err = retrace_core::replay(&trace).expect_err("replay must reject a host-shaped fd");
    assert!(err.detail.contains("fd divergence"),
        "divergence should name the fd mismatch, got: {}", err.detail);
}
