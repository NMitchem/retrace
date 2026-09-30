// M46 gate (spec §3f, §4; plan R7). libdispatch's timers, end to end, on the synthetic clock.
// Every assertion is on the trace, on the guest's own output or on the recorder's own words, never
// on an exit code alone: a guest whose timer path read the host's clock still exits 0 once real
// time catches up with it.
mod util;

use retrace_trace::Event;
use std::path::{Path, PathBuf};

/// Record `argv` of `guest`, assert exit 0, and replay twice byte-identically.
fn records_and_replays(guest: &str, argv: &[&str]) -> (util::RunOut, PathBuf) {
    let (rec, trace) = util::record_dynamic_args(guest, argv);
    assert_eq!(rec.code, 0, "{argv:?}: record: {}", rec.stderr);
    for n in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 0, "{argv:?}: replay {n}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "{argv:?}: replay {n} stdout");
    }
    (rec, trace)
}

/// Every event of `trace` with its landmark index (the replay session's `idx`).
fn events(trace: &Path) -> Vec<(usize, Event)> {
    retrace_trace::Reader::open(trace).unwrap().into_iter().enumerate().collect()
}

/// R7: the guest's `mach_absolute_time` has one source. `mach_get_times` falls back to
/// `gettimeofday` (116) whenever the commpage stamp is a second or more from `mach_absolute_time`
/// (xnu `libsyscall/wrappers/mach_get_times.c`). retrace's commpage is frozen while its timebase is
/// synthetic, so it always falls back, and before R7 it handed the guest the HOST's mach time,
/// which is libdispatch's timer "now" (t0 M1).
#[test]
fn the_guest_clock_has_one_source() {
    let (rec, trace) = records_and_replays(retrace_guest::AFTER_DYN, &["clock"]);
    assert_eq!(rec.stdout, b"clock ok\n",
        "mach_get_times must agree with mach_absolute_time: got {:?}", String::from_utf8_lossy(&rec.stdout));
    let fallbacks = events(&trace).into_iter()
        .filter(|(_, e)| matches!(e, Event::Syscall { num, args, err: false, .. }
            if *num == retrace_arch::SYS_GETTIMEOFDAY && args[2] != 0))
        .count();
    assert!(fallbacks >= 1,
        "the fixture must reach gettimeofday's mach-time fallback, or this test proves nothing about R7");
}
