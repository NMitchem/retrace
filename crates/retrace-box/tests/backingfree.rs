// M40: a dropped Box_ must release every byte of guest backing it allocated. Before M40 `Box_` had
// no Drop and `alloc_pages` mmaps were never unmapped, so every replay session the debugger opened
// and dropped leaked its whole guest memory: ~55 MB and ~1,580 mappings per session on rung 8
// (t0 M4), 2.3 GB after 41 sessions. The count is `live_backing_bytes()`, which is exact, not RSS.
use retrace_box::{live_backing_bytes, Box_};
use retrace_guest::{parse_macho, STEPPY};

#[test]
fn a_dropped_box_releases_every_backing_byte() {
    let loaded = parse_macho(&std::fs::read(STEPPY).unwrap());
    let base = live_backing_bytes();
    for round in 0..3 {
        let mut b = Box_::load(&loaded);
        let loaded_bytes = live_backing_bytes();
        assert!(loaded_bytes > base, "round {round}: a live box holds backing bytes");
        // A runtime backing too, and the explicit-removal path: guest_munmap gives back exactly it.
        let a = b.guest_mmap(0, 0x8000, 3, 0x1002).expect("anon mmap");
        assert_eq!(live_backing_bytes(), loaded_bytes + 0x8000, "round {round}: the mmap's backing is counted");
        b.guest_munmap(a, 0x8000);
        assert_eq!(live_backing_bytes(), loaded_bytes, "round {round}: guest_munmap releases it");
        let _ = b.guest_mmap(0, 0x4000, 3, 0x1002).expect("anon mmap");
        drop(b);
        assert_eq!(live_backing_bytes(), base, "round {round}: dropping the box must release every byte");
    }
}
