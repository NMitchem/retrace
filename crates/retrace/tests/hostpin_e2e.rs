// M57 §3e: a recording carries the host that made it, and replays only on that macOS build's shared cache.
mod util;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const BOUND: u64 = 120;

fn scratch(tag: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!("retrace-hostpin-{}-{}-{tag}.bin", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
}

fn sw_vers(flag: &str) -> String {
    let o = std::process::Command::new("sw_vers").arg(flag).output().unwrap();
    String::from_utf8(o.stdout).unwrap().trim().to_string()
}

fn record(guest: &str, tag: &str) -> PathBuf {
    let t = scratch(tag);
    let r = util::run_bounded(util::bin(), &["record", "-o", t.to_str().unwrap(), guest], &[], BOUND);
    assert_eq!(r.code, Some(0), "record {guest}: {}", r.stderr);
    t
}

/// A copy of `src` with the cache UUID's first byte flipped, built through the writer.
fn another_cache(src: &Path) -> PathBuf {
    let mut host = retrace_trace::Reader::host(src).unwrap().expect("a host record");
    host.cache_uuid.as_mut().expect("a cache uuid")[0] ^= 0xff;
    let events = retrace_trace::Reader::open(src).unwrap();
    let copy = scratch("other");
    let mut w = retrace_trace::Writer::create_with_host(&copy, &host).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);
    copy
}

#[test]
fn a_dynamic_recording_carries_this_hosts_version_build_and_cache_uuid() {
    let t = record(retrace_guest::HELLO_DYN, "dyn");
    let h = retrace_trace::Reader::host(&t).unwrap().expect("a host record");
    assert_eq!(h.product, sw_vers("-productVersion"));
    assert_eq!(h.build, sw_vers("-buildVersion"));
    let cache = std::fs::File::open("/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld/dyld_shared_cache_arm64e")
        .or_else(|_| std::fs::File::open("/System/Cryptexes/OS/System/Library/dyld/dyld_shared_cache_arm64e")).unwrap();
    let mut hdr = [0u8; 0x68];
    std::os::unix::fs::FileExt::read_exact_at(&cache, &mut hdr, 0).unwrap();
    assert_eq!(h.cache_uuid, Some(<[u8; 16]>::try_from(&hdr[0x58..0x68]).unwrap()));
    let _ = std::fs::remove_file(&t);
}

#[test]
fn a_recording_with_another_cache_uuid_is_refused_by_name_with_the_bad_input_exit() {
    let t = record(retrace_guest::HELLO_DYN, "pin");
    let copy = another_cache(&t);
    let h = retrace_trace::Reader::host(&copy).unwrap().unwrap();
    let want = format!("retrace: {} was recorded on macOS {} ({}) with a different shared cache; a recording replays only on \
        the macOS build that made it", copy.display(), h.product, h.build);
    let rep = util::run_bounded(util::bin(), &["replay", copy.to_str().unwrap()], &[], BOUND);
    assert_eq!(rep.code, Some(2), "{}", rep.stderr);
    assert_eq!(rep.stderr.trim_end(), want);
    let dbg = util::run_bounded(util::bin(), &["debug", copy.to_str().unwrap(), "--script", "where"], &[], BOUND);
    assert_eq!(dbg.code, Some(2), "{}", dbg.stderr);
    assert_eq!(dbg.stderr.trim_end(), want);
    let _ = std::fs::remove_file(&t);
    let _ = std::fs::remove_file(&copy);
}

#[test]
fn the_library_refuses_another_cache_before_restoring() {
    let t = record(retrace_guest::HELLO_DYN, "lib");
    let copy = another_cache(&t);
    match retrace_core::ReplaySession::open(&copy) {
        Ok(_) => panic!("a foreign cache must not open"),
        Err(e) => assert!(e.contains("with a different shared cache"), "{e}"),
    }
    let _ = std::fs::remove_file(&t);
    let _ = std::fs::remove_file(&copy);
}

#[test]
fn a_static_recording_carries_no_cache_uuid_and_replays() {
    let t = record(retrace_guest::HELLO, "static");
    let h = retrace_trace::Reader::host(&t).unwrap().expect("a host record");
    assert_eq!(h.cache_uuid, None);
    assert_eq!(h.product, sw_vers("-productVersion"));
    let rep = util::run_bounded(util::bin(), &["replay", t.to_str().unwrap()], &[], BOUND);
    assert_eq!(rep.code, Some(0), "{}", rep.stderr);
    let _ = std::fs::remove_file(&t);
}

#[test]
fn a_pre_m57_recording_is_refused_as_another_format() {
    let t = scratch("old");
    std::fs::write(&t, b"RT\x00\x11rest-of-a-pre-M57-trace").unwrap();
    let rep = util::run_bounded(util::bin(), &["replay", t.to_str().unwrap()], &[], BOUND);
    assert!(rep.stderr.contains("recorded by a different retrace trace format"), "{}", rep.stderr);
    assert!(rep.stderr.contains(r"RT\x00\x11") && rep.stderr.contains(r"RT\x00\x12"), "{}", rep.stderr);
    assert!(!rep.stderr.contains("panicked at") && !rep.stderr.contains("internal error"), "{}", rep.stderr);
    let _ = std::fs::remove_file(&t);
}
