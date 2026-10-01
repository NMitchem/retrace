//! M47 gate (spec §3f): the repo-owned fixtures for git's mechanisms, one test per fixture mode.
//! Each test records, replays twice byte-identically, and asserts the difference M47 makes — never
//! an exit code a weaker failure would also produce. Every fixture is C, built by
//! `crates/retrace-guest/build.rs`, and was run natively by t0 for its reference output
//! (`docs/sweep-evidence/2026-09-30-m47-t0/native-*.out`).
mod util;
use retrace_trace::{Event, Region};
use std::path::{Path, PathBuf};

/// A fresh, empty directory for a fixture that writes to disk.
fn scratch_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("retrace-m47-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Record `guest` with `args`, replay it twice, and hold both replays to the recording.
fn records_and_replays_twice(guest: &str, args: &[&str]) -> (util::RunOut, PathBuf) {
    let (rec, trace) = util::record_dynamic_args(guest, args);
    assert_eq!(rec.code, 0, "record {guest} {args:?}: {}", rec.stderr);
    for i in 0..2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 0, "replay {i} of {guest} {args:?}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "replay {i} of {guest} {args:?}: stdout differs from the recording");
    }
    (rec, trace)
}

/// RED at `427fa0a`: the recorder stops at the M33 panic for mkdir (136), the first of the new rows
/// the fixture reaches. The on-disk assertions are the difference: only a FORWARDED link and
/// utimes can leave a second link and that mtime, and replay forwards nothing.
#[test]
fn fsops_mkdir_chdir_link_and_utimes_are_forwarded_and_land_on_disk() {
    use std::os::unix::fs::MetadataExt;
    let dir = scratch_dir("fsops");
    let (rec, _) = records_and_replays_twice(retrace_guest::FSOPS_DYN, &[dir.to_str().unwrap()]);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), "mkdir chdir link rename utimes ok\nh mtime=1234567890 nlink=2\n");
    let st = std::fs::metadata(dir.join("d/h")).unwrap();
    assert_eq!((st.mtime(), st.nlink()), (1_234_567_890, 2), "utimes and link must have reached the host");
    assert_eq!(std::fs::read(dir.join("d/f")).unwrap(), b"fsops\n", "the relative open after chdir must land in <dir>/d");
}

/// One madvise landmark, as the tests read it.
#[derive(Debug)]
struct Madv { len: u64, advice: u32, ret: u64, ret1: u64, err: bool, writes: usize }

/// Every madvise landmark in `trace`.
fn madvise_events(trace: &Path) -> Vec<Madv> {
    retrace_trace::Reader::open(trace).unwrap().into_iter().filter_map(|e| match e {
        Event::Syscall { num, args, ret, ret1, err, writes, .. } if num == retrace_arch::SYS_MADVISE =>
            Some(Madv { len: args[1], advice: args[2] as u32, ret, ret1, err, writes: writes.len() }),
        _ => None,
    }).collect()
}

/// A copy of `trace` with the FIRST event `edit` accepts rewritten in place (`edit` returns true
/// when it edited one). `Writer` re-frames every record with a fresh CRC, so only the content lies —
/// `util::tamper_last_write`'s method.
fn tamper(trace: &Path, tag: &str, mut edit: impl FnMut(&mut Event) -> bool) -> PathBuf {
    let mut ev = retrace_trace::Reader::open(trace).unwrap();
    assert!(ev.iter_mut().any(&mut edit), "no event in {} to tamper", trace.display());
    let out = trace.with_extension(format!("{tag}.tampered.bin"));
    let mut w = retrace_trace::Writer::create(&out).unwrap();
    for e in &ev { w.append(e).unwrap(); }
    out
}

/// RED at `427fa0a`: the forwarded MADV_ZERO writes 512 KiB past the diff window and the guard
/// band stops the recorder. The difference M47 makes is the zeros landing AND the landmark carrying
/// none of them (R3: recomputed on both sides, never recorded).
#[test]
fn madv_zero_zeroes_the_range_by_recompute_and_records_no_bytes() {
    let (rec, trace) = records_and_replays_twice(retrace_guest::MADV_DYN, &["zero"]);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), "zero low=zeros high=kept\n");
    let zeros: Vec<_> = madvise_events(&trace).into_iter().filter(|e| e.advice == retrace_arch::MADV_ZERO && e.len == 0x8_0000).collect();
    assert_eq!(zeros.len(), 1, "the fixture's one 512 KiB MADV_ZERO must be a landmark");
    let z = &zeros[0];
    assert_eq!((z.ret, z.ret1, z.err, z.writes), (0, 0, false, 0),
        "ret 0, no ret1, no error, and NO writes: the zeros are recomputed, not recorded (R3)");
}

/// No RED at `427fa0a`, and t0 M1(c) found no repo-owned trigger: `madv reuse` printed `reuse kept`
/// / `reuse ok` 5/5 on the base binary. It guards the no-op semantics. Every madvise in the run —
/// the fixture's two and libmalloc's own — is a landmark with no writes.
#[test]
fn madv_free_reusable_and_reuse_are_no_ops_that_keep_the_bytes() {
    let (rec, trace) = records_and_replays_twice(retrace_guest::MADV_DYN, &["reuse"]);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), "reuse kept\nreuse ok\n");
    let ev = madvise_events(&trace);
    for adv in [retrace_arch::MADV_FREE_REUSABLE, retrace_arch::MADV_FREE_REUSE] {
        assert!(ev.iter().any(|e| e.advice == adv && e.len == 1 << 20), "the fixture's advice {adv} over 1 MiB must be a landmark");
    }
    assert!(ev.iter().all(|e| (e.ret, e.ret1, e.err, e.writes) == (0, 0, false, 0)), "{ev:x?}");
}

/// No RED at `427fa0a`: the value was forwarded. After M47 it stops the recorder, by value.
#[test]
fn an_unmeasured_advice_stops_the_recorder_naming_the_value() {
    let (rec, _) = util::record_dynamic_args(retrace_guest::MADV_DYN, &["bad"]);
    assert_ne!(rec.code, 0, "an unmeasured advice must not record: stdout {:?}", String::from_utf8_lossy(&rec.stdout));
    assert!(rec.stderr.contains("M47: unmeasured madvise advice 9."), "{}", rec.stderr);
}

/// Review Focus 4. A recording whose madvise landmark carries writes is not one this build wrote;
/// replay refuses it by name rather than applying bytes the model never made.
#[test]
fn replay_refuses_a_madvise_landmark_that_carries_writes() {
    let (_, trace) = records_and_replays_twice(retrace_guest::MADV_DYN, &["zero"]);
    let t = tamper(&trace, "madv", |e| match e {
        Event::Syscall { num, args, writes, .. } if *num == retrace_arch::SYS_MADVISE && args[2] as u32 == retrace_arch::MADV_ZERO => {
            writes.push(Region { ipa: args[0], bytes: vec![0; 16] });
            true
        }
        _ => false,
    });
    let rp = util::replay(&t);
    assert_eq!(rp.code, 3, "a tampered madvise landmark must be a divergence: {}", rp.stderr);
    assert!(rp.stderr.contains("madvise recorded ret=0x0 ret1=0x0 err=false with 1 write(s)"), "{}", rp.stderr);
}
