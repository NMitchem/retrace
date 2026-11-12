//! M59: xzone malloc's deferred-reclamation ring, which the box now grants (`retrace-box`'s reclaim.rs).
//!
//! Before M59 the box answered 4822 `KERN_NOT_SUPPORTED`. libmalloc survived that, but reported it: a
//! `malloc: xzm: failed to initialize deferred reclamation buffer` line on every recording made on a
//! terminal, and a syslog socket otherwise, which also moved every program's first free descriptor
//! from native's 3 to 4. These tests guard:
//! - malloc gets its ring: `reclaim_dyn`'s first free descriptor, a large-block churn and the
//!   accounting trap (-63) match native through a recording and two replays, and the recording holds
//!   one 4822 landmark whose reply is a ring.
//! - the kernel's half, through libsystem_kernel's reclaim API (`reclaimapi_dyn`, with
//!   `MallocSpaceEfficient=1` so libmalloc leaves the task's one ring to it): a flush and a resize unmap
//!   the DEALLOCATE entries they consume and keep FREE and cancelled ones, the cancel states, a full
//!   ring, a second allocate refused and the trap with no ring, all as native answers them.
//! - the ring's state crosses a checkpoint: a seek replays the flushes and the resize after a restore.
//! - the symptom itself: a recording made on a terminal prints no malloc line.
mod util;

use retrace_guest::{RECLAIMAPI_DYN, RECLAIM_DYN};
use retrace_trace::Event;

const MACH_MSG2: u64 = (-47i64) as u64;
const API_ENV: &[(&str, &str)] = &[("MallocSpaceEfficient", "1")];

fn native(prog: &str, env: &[(&str, &str)]) -> Vec<u8> {
    let n = util::run_bounded(prog, &[], env, 60);
    assert_eq!(n.code, Some(0), "the native run must exit 0: {}", n.stderr);
    n.stdout
}

fn u32_at(b: &[u8], at: usize) -> u32 { u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) }

#[test]
fn malloc_gets_its_ring_and_the_run_matches_native() {
    let want = native(RECLAIM_DYN, &[]);
    let text = String::from_utf8_lossy(&want).into_owned();
    assert!(text.starts_with("first free fd 3\n") && text.ends_with("accounting trap kr 0\n"),
        "the native run this test was written against: {text}");
    let out = util::assert_rung_records_and_replays(RECLAIM_DYN, &[], &want);
    let events = retrace_trace::Reader::open(&out.trace).unwrap();
    // 4822's reply is msgh_id 4922: RetCode @32, the ring's address @36.
    let rings: Vec<(i32, u64)> = events.iter().filter_map(|e| match e {
        Event::Syscall { num, writes, .. } if *num == MACH_MSG2 && writes.len() == 1
            && writes[0].bytes.len() >= 44 && u32_at(&writes[0].bytes, 20) == 4922 => {
            let b = &writes[0].bytes;
            Some((u32_at(b, 32) as i32, u64::from_le_bytes(b[36..44].try_into().unwrap())))
        }
        _ => None,
    }).collect();
    assert_eq!(rings.len(), 1, "one 4822 landmark: {rings:x?}");
    assert!(rings[0].0 == 0 && rings[0].1 != 0, "the ring is granted, not refused: {rings:x?}");
    let acct: Vec<_> = events.iter().filter_map(|e| match e {
        Event::Syscall { num, ret, writes, .. } if *num == retrace_arch::MACH_VM_RECLAIM_ACCOUNTING => Some((*ret, writes.len())),
        _ => None,
    }).collect();
    assert_eq!(acct, vec![(0, 2)], "the fixture's one trap -63, answered 0 with its two out-parameters");
    let _ = std::fs::remove_file(&out.trace);
}

#[test]
fn a_flush_and_a_resize_reclaim_what_they_consume_as_natively() {
    let want = native(RECLAIMAPI_DYN, API_ENV);
    let text = String::from_utf8_lossy(&want).into_owned();
    for line in ["accounting with no ring kr 56", "allocate kr 0, capacity 1016", "second allocate kr 0x20004006",
                 "flush 2 kr 0: mapped A 0 B 1 C 1 D 1", "resize kr 0 capacity 2040: mapped C 0 D 1",
                 "full after 2040 entries", "flush all kr 0, entering again gives id one"] {
        assert!(text.contains(line), "the native run this test was written against lacks {line:?}:\n{text}");
    }
    let (out, _) = util::assert_rung_records_and_replays_env(RECLAIMAPI_DYN, &[], &want, API_ENV);
    let _ = std::fs::remove_file(&out.trace);
}

#[test]
fn the_ring_survives_a_checkpointed_seek() {
    let (rec, trace) = util::record_dynamic_args_env(RECLAIMAPI_DYN, &[], API_ENV);
    assert_eq!(rec.code, 0, "{}", rec.stderr);
    // Stop with the ring made, step once and back: the step back restores the checkpoint taken at that
    // stop, so the run forward to the end replays both flushes and the resize from a restored box. With
    // the ring left out of `BoxState` the first flush answers "no ring" and the reply comparison diverges
    // (measured: "continue diverged at landmark 421 ... reclaim ring (4823) reply mismatch").
    let script = "break _reclaimapi_ring_ready; continue; stepi; reverse-stepi; break _reclaimapi_done; continue";
    let (code, out, err) = util::debug_bounded(trace.to_str().unwrap(), script, 120);
    let all = format!("{out}\n{err}");
    assert_eq!(code, Some(0), "{all}");
    assert!(!all.contains("diverge"), "{all}");
    assert!(out.lines().any(|l| l.starts_with("hit ") && l.ends_with("in _reclaimapi_done")),
        "the run forward reaches the end:\n{all}");
    let _ = std::fs::remove_file(&trace);
}

#[test]
fn a_recording_on_a_terminal_prints_no_malloc_line() {
    let trace = std::env::temp_dir().join(format!("reclaim-tty-{}.retrace", std::process::id()));
    // script(1) gives the recorder a terminal: libmalloc's report reaches stderr only there, so a test
    // reading a pipe would pass on the old refusal too.
    let r = util::run_bounded("/usr/bin/script",
        &["-q", "/dev/null", util::bin(), "record", "-o", trace.to_str().unwrap(), RECLAIM_DYN], &[], 120);
    let all = String::from_utf8_lossy(&r.stdout).into_owned();
    assert_eq!(r.code, Some(0), "{all}\n{}", r.stderr);
    assert!(all.contains("first free fd 3"), "the terminal's output was captured: {all}");
    assert!(!all.contains("xzm") && !all.contains("deferred reclamation"), "{all}");
    let _ = std::fs::remove_file(&trace);
}
