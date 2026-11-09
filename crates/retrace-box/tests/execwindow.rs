// M57 final review I1 (Ruling P18): a static box that execs a dynamic image takes the host cache's
// shared-region window, as a dynamic load does.
use retrace_box::{Box_, ExecImage, DEFAULT_CACHE_PATH};
use retrace_guest::{parse_macho, slice_arm64e, CRASHY, DYLD_PATH, HELLO};
use std::io::{Read, Seek, SeekFrom};

fn want() -> u64 {
    let mut f = std::fs::File::open(DEFAULT_CACHE_PATH).expect("open the host cache");
    let mut w = |off: u64| { let mut b = [0u8; 8]; f.seek(SeekFrom::Start(off)).unwrap(); f.read_exact(&mut b).unwrap(); u64::from_le_bytes(b) };
    let (base, size) = (w(0xE0), w(0xE8));
    0x3_0000_0000u64.max((base + size).next_multiple_of(0x200_0000))
}

#[test]
fn a_static_to_dynamic_exec_takes_the_host_caches_window() {
    let mut b = Box_::load(&parse_macho(&std::fs::read(HELLO).unwrap()));
    assert_eq!(b.shared_region_end(), 0x3_0000_0000, "a static box starts at the constant");
    let img = ExecImage {
        exe: parse_macho(&std::fs::read(CRASHY).unwrap()),
        dylinker: parse_macho(slice_arm64e(&std::fs::read(DYLD_PATH).unwrap())),
        argv: vec![b"crashy".to_vec()], envp: vec![], exec_path: b"crashy".to_vec(),
        closed: vec![], spawn: None, host_new: vec![],
    };
    b.exec_image(&img).unwrap();
    assert_eq!(b.shared_region_end(), want(), "after exec_image");
}
