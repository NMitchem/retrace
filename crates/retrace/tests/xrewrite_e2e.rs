//! M49 §3e: the store-exclusive rewrite, end to end (spec §4's `mrspair` and census guards).
//!
//! Every discarded-status exclusive pair the box stages on the dynamic path becomes the plain load
//! and store an uninterrupted run performs (X1–X3). These tests assert the difference that makes at
//! each of the three staging sites:
//! - the executable's own `__TEXT` (`load_dynamic`): mrspair_dyn's pair, whose trapped timebase
//!   read lost the store on every run before M49 (`cell=0`; native prints `cell=4242`);
//! - a dlopen'd dylib's `__TEXT` (`guest_mmap_file`'s exec path): the same pair in mrspair_lib;
//! - the census: the shared cache's two getpid pairs (`page_in_cache`) and the on-disk dyld's
//!   (`load_dynamic`), read back from guest memory in a recorded hello_dyn.
//!
//! A pair that survived unrewritten still exits 0, so no test here rests on an exit code.
mod util;
use retrace_core::{Advance, ReplaySession};
use retrace_trace::{Event, Region};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// `ldxr w10, [x9]; mrs x11, cntvct_el0; cbnz w10, +8; stxr wzr, w0, [x9]`: the pair mrspair_dyn
/// and mrspair_lib pin, and what X2 makes of it.
const EXCL_PAIR: [u32; 4] = [0x885f_7d2a, 0xd53b_e04b, 0x3500_004a, 0x881f_7d20];
const PLAIN_PAIR: [u32; 4] = [0xb940_012a, 0xd53b_e04b, 0x3500_004a, 0xb900_0120];
/// mrspair_dyn's retry-loop control: `ldxr w10, [x9]; mrs x11, cntvct_el0; add w10, w10, #1;
/// stxr w12, w10, [x9]`. It reads its status, so X1 leaves it exclusive.
const RETRY_PAIR: [u32; 4] = [0x885f_7d2a, 0xd53b_e04b, 0x1100_054a, 0x880c_7d2a];
/// Libsyscall's getpid pair after X2: `ldr w10, [x9]; cbnz w10, +8; str w0, [x9]`.
const PLAIN_GETPID: [u32; 3] = [0xb940_012a, 0x3500_004a, 0xb900_0120];
/// Where the shared cache maps (its header's `sharedRegionStart`). The on-disk dyld, slid to
/// `DYLD_BASE`, sits below it.
const SHARED_REGION_START: u64 = 0x1_8000_0000;
/// The main executable's segments, at `EXE_BASE` (4 GiB). dyld is slid to `DYLD_BASE`, 1 GiB above,
/// so this window holds the executable alone.
const EXE_WINDOW: std::ops::Range<u64> = retrace_core::EXE_BASE..retrace_core::EXE_BASE + 0x4000_0000;
const PROT_EXEC: u64 = 0x4;
const MAP_ANON: u64 = 0x1000;

fn bytes_of(words: &[u32]) -> Vec<u8> { words.iter().flat_map(|w| w.to_le_bytes()).collect() }

/// Whether any of `regions` holds `words`, consecutively.
fn holds(regions: &[&Region], words: &[u32]) -> bool {
    let needle = bytes_of(words);
    regions.iter().any(|r| r.bytes.windows(needle.len()).any(|w| w == needle))
}

/// The executable's regions in a recording's opening snapshot: what `load_dynamic` staged.
fn exe_regions(events: &[Event]) -> Vec<&Region> {
    let Some(Event::Snapshot { mem, .. }) = events.first() else { panic!("a recording opens with a Snapshot") };
    mem.iter().filter(|r| EXE_WINDOW.contains(&r.ipa)).collect()
}

fn assert_two_replays(trace: &Path, stdout: &str) {
    for i in 0..2 {
        let rep = util::replay(trace);
        assert_eq!(rep.code, 0, "replay {i}: {}", rep.stderr);
        assert_eq!(String::from_utf8_lossy(&rep.stdout), stdout, "replay {i}'s stdout");
    }
}

struct Rec { trace: PathBuf, stdout: String }

/// mrspair_dyn with no arguments, recorded once per test process.
fn pair_rec() -> &'static Rec {
    static C: OnceLock<Rec> = OnceLock::new();
    C.get_or_init(|| {
        let (rec, trace) = util::record_dynamic(retrace_guest::MRSPAIR_DYN);
        assert_eq!(rec.code, 0, "record mrspair_dyn: {}", rec.stderr);
        Rec { trace, stdout: String::from_utf8(rec.stdout).unwrap() }
    })
}

/// The executable's own pair (`load_dynamic`). Before M49 the trapped read between its halves
/// exited to the box, the ERET back cleared the local monitor, and the store was lost on record
/// and on every replay (`cell=0`, static `excl-report.md` §1). Native prints `cell=4242`.
#[test]
fn the_executables_pair_publishes_its_store_on_record_and_both_replays() {
    let r = pair_rec();
    assert!(r.stdout.starts_with("cell=4242 "), "the store landed on record: {}", r.stdout);
    assert_two_replays(&r.trace, &r.stdout);
    let events = retrace_trace::Reader::open(&r.trace).unwrap();
    let exe = exe_regions(&events);
    assert!(holds(&exe, &PLAIN_PAIR), "load_dynamic staged the executable's pair rewritten");
    assert!(!holds(&exe, &EXCL_PAIR), "the opening snapshot still holds the exclusive pair");
}

/// The retry-loop control and the documented limit (spec §3e, "Left as limits"). A loop that reads
/// its store's status is not a discarded-status pair, so X1 leaves it exclusive, and the trapped
/// read between its halves fails its store on every pass until the fixture's bound stops it at
/// five. Native prints `ctr=1 tries=1`; retrace prints `ctr=0 tries=5`, on record and replay alike.
#[test]
fn the_retry_loop_keeps_its_status_and_its_documented_limit() {
    let r = pair_rec();
    assert_eq!(r.stdout, "cell=4242 ctr=0 tries=5\n");
    assert_two_replays(&r.trace, &r.stdout);
    let events = retrace_trace::Reader::open(&r.trace).unwrap();
    assert!(holds(&exe_regions(&events), &RETRY_PAIR), "X1 left the status-reading pair exclusive");
}

/// The exec-mmap site (`guest_mmap_file`): the same pair in mrspair_lib.dylib, which mrspair_dyn
/// dlopens by absolute path. dyld maps a non-cache dylib's `__TEXT` with a FIXED, executable map of
/// the file (M9's non-cache-dylib strategy; at plan time `mmap(…, 0x4000, PROT_READ|PROT_EXEC,
/// 0x40012, fd, 0)`), and that map's recorded writes are the bytes replay maps. So the trace
/// itself shows which bytes the hook staged.
#[test]
fn a_dlopened_dylibs_pair_is_rewritten_by_the_exec_mmap_hook() {
    let (rec, trace) = util::record_dynamic_args(retrace_guest::MRSPAIR_DYN, &["dlopen", retrace_guest::MRSPAIR_LIB]);
    assert_eq!(rec.code, 0, "record mrspair_dyn dlopen: {}", rec.stderr);
    let stdout = String::from_utf8(rec.stdout).unwrap();
    assert_eq!(stdout, "libcell=4242\n", "the dylib's store landed on record");
    assert_two_replays(&trace, &stdout);
    let events = retrace_trace::Reader::open(&trace).unwrap();
    let exec_maps: Vec<&Region> = events.iter().filter_map(|e| match e {
        Event::Syscall { num, args, err: false, writes, .. }
            if *num == retrace_arch::SYS_MMAP && args[3] & MAP_ANON == 0 && args[2] & PROT_EXEC != 0 => Some(writes),
        _ => None,
    }).flatten().collect();
    assert!(holds(&exec_maps, &PLAIN_PAIR),
        "no executable file mmap recorded the rewritten pair ({} region(s) in such maps)", exec_maps.len());
    assert!(!holds(&exec_maps, &EXCL_PAIR), "an executable file mmap recorded the pair unrewritten");
}

/// Spec §4's census guard: the dynamic path's three discarded-status sites (static
/// `excl-wzr_hits2.json`, t0 M3) read back as plain `ldr`/`str` from guest memory in a recorded
/// hello_dyn. The mrspair tests prove only the executable-segment and exec-mmap hooks; a cache-page
/// or dyld hook that never ran would still pass them.
///
/// Each site is found from the recording, never pinned. Libsyscall's `__getpid` returns from its
/// `svc` into a `b.lo` that is taken (getpid cannot fail) and lands on the pair's first word, so
/// (g+1, 1) after each getpid (20) landmark is that word, as hitorder_e2e measured for dyld's copy.
/// Three images run their own copy, once each: the on-disk dyld, the cache's dyld that dyld4
/// restarts into, and libsystem_kernel (plan-time probe: 0x140004f68, 0x180118f68, 0x1804ae1b8).
#[test]
fn the_census_sites_read_back_as_plain_loads_and_stores() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "record hello_dyn: {}", rec.stderr);
    let mut s = ReplaySession::open(&trace).unwrap();
    let mut sites = Vec::new();
    loop {
        if let Some((20, _)) = s.peek_syscall() {
            assert!(matches!(s.advance().unwrap(), Advance::Event), "the getpid landmark");
            s.step_insns(1).unwrap(); // the b.lo, taken
            let pc = s.pc();
            let w = s.read_mem(pc, 12).unwrap_or_else(|| panic!("the pair at {pc:#x} is staged: it runs next"));
            let words: Vec<u32> = w.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
            sites.push((pc, words));
            continue;
        }
        let adv = s.advance().unwrap_or_else(|d| panic!("diverged at landmark {}: {}", d.landmark, d.detail));
        if let Advance::Exited(r) = adv {
            assert_eq!(r.stdout, rec.stdout, "the replay reached the recorded end");
            break;
        }
    }
    assert_eq!(sites.len(), 3, "one getpid landmark per image that carries Libsyscall: {sites:#x?}");
    assert_eq!(sites.iter().filter(|(pc, _)| *pc < SHARED_REGION_START).count(), 1,
        "one site in the on-disk dyld, the other two in the shared cache: {sites:#x?}");
    for (pc, w) in &sites {
        let what = if *pc < SHARED_REGION_START { "the on-disk dyld's (load_dynamic)" } else { "the shared cache's (page_in_cache)" };
        assert_eq!(*w, PLAIN_GETPID, "{what} getpid pair at {pc:#x} is not plain: {w:#010x?}");
    }
}
