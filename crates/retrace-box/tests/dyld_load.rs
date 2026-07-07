use retrace_box::{Box_, DYLD_BASE};
use retrace_guest::{parse_macho, slice_arm64e, HELLO_DYN, DYLD_PATH};

// Loading exe + dyld must set PC to dyld's SLID entry (dyld is PIE at vmaddr 0, slid by DYLD_BASE)
// and lay down a start stack whose first word at SP is argc==1.
#[test]
fn dynamic_load_sets_dyld_entry_and_stack() {
    let exe = parse_macho(&std::fs::read(HELLO_DYN).unwrap());
    assert_eq!(exe.dylinker.as_deref(), Some("/usr/lib/dyld"));
    let dyld = parse_macho(slice_arm64e(&std::fs::read(DYLD_PATH).unwrap()));
    let b = Box_::load_dynamic(&exe, &dyld, "hello_dyn");
    assert_eq!(b.pc(), dyld.entry + DYLD_BASE, "PC must be dyld's slid entry");
    let argc = u64::from_le_bytes(b.read_guest(b.sp(), 8).try_into().unwrap());
    assert_eq!(argc, 1, "argc at SP must be 1");
}
