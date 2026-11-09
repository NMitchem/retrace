// M57 §3b: the shared-region window's end is box state taken from the host's cache, identical on
// load, restore and from_checkpoint.
use retrace_box::Box_;
use retrace_guest::{parse_macho, slice_arm64e, CRASHY, DYLD_PATH};
use retrace_trace::Event;
use std::io::{Read, Seek, SeekFrom};

const CACHE: &str = "/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld/dyld_shared_cache_arm64e";

fn announce(line: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{line}");
}

/// The expected end, computed by hand from the main file's header (0xE0 base, 0xE8 size).
fn want() -> u64 {
    let mut f = std::fs::File::open(CACHE).expect("open the host cache");
    let mut w = |off: u64| { let mut b = [0u8; 8]; f.seek(SeekFrom::Start(off)).unwrap(); f.read_exact(&mut b).unwrap(); u64::from_le_bytes(b) };
    let (base, size) = (w(0xE0), w(0xE8));
    0x3_0000_0000u64.max((base + size).next_multiple_of(0x200_0000))
}

fn dyn_box() -> Box_ {
    let exe = parse_macho(&std::fs::read(CRASHY).unwrap());
    let dyld = parse_macho(slice_arm64e(&std::fs::read(DYLD_PATH).unwrap()));
    Box_::load_dynamic(&exe, &dyld, &["crashy".to_string()])
}

#[test]
fn a_dynamic_box_holds_the_host_caches_window_end() {
    let want = want();
    let b = dyn_box();
    assert_eq!(b.shared_region_end(), want, "load_dynamic");
    let Event::Snapshot { regs, mem } = b.snapshot() else { panic!("snapshot") };
    drop(b);
    let r = Box_::restore(&mem, &regs);
    assert_eq!(r.shared_region_end(), want, "restore");
    let st = r.checkpoint();
    drop(r);
    let c = Box_::from_checkpoint(&st);
    assert_eq!(c.shared_region_end(), want, "from_checkpoint");
}

#[test]
fn a_fixed_request_above_the_constant_inside_the_caches_window_answers_kern_no_space() {
    let want = want();
    if want == 0x3_0000_0000 {
        announce("SKIPPED srwindow: this host's cache window ends at or below 0x300000000; nothing above the constant to refuse");
        return;
    }
    let mut b = dyn_box();
    assert_eq!(b.guest_vm_map_masked(0x3_0000_0000, 0x4000, 0, 0, false), Err(retrace_arch::KERN_NO_SPACE));
    // Fidelity limit (spec 3b): natively 27 refuses up to 0x70_0000_0000 (native-fixed-map-27.2.txt);
    // retrace refuses only to the cache's end, so the first address at it is granted.
    assert_eq!(b.guest_vm_map_masked(want, 0x4000, 0, 0, false), Ok(want));
}
