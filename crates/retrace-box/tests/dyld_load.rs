use retrace_box::{Box_, DYLD_BASE};
use retrace_guest::{parse_macho, slice_arm64e, HELLO_DYN, DYLD_PATH};

// Loading exe + dyld must set PC to dyld's SLID entry (dyld is PIE at vmaddr 0, slid by DYLD_BASE)
// and lay down dyld4's KernelArgs start frame: SP[0] = mainExecutable (main exe's mach-header
// address), SP[8] = argc == 1.
#[test]
fn dynamic_load_sets_dyld_entry_and_stack() {
    let exe = parse_macho(&std::fs::read(HELLO_DYN).unwrap());
    assert_eq!(exe.dylinker.as_deref(), Some("/usr/lib/dyld"));
    let dyld = parse_macho(slice_arm64e(&std::fs::read(DYLD_PATH).unwrap()));
    let b = Box_::load_dynamic(&exe, &dyld, &["hello_dyn".to_string()]);
    assert_eq!(b.pc(), dyld.entry + DYLD_BASE, "PC must be dyld's slid entry");
    // dyld4 KernelArgs: SP[0] is the main executable's mach-header address (the exe segment whose
    // bytes begin with MH_MAGIC_64), SP[8] is argc.
    let main_hdr = exe.segments.iter()
        .find(|s| s.data.len() >= 4 && s.data[0..4] == [0xcf, 0xfa, 0xed, 0xfe])
        .map(|s| s.vaddr).expect("exe carries a mach header");
    let sp0 = u64::from_le_bytes(b.read_guest(b.sp(), 8).try_into().unwrap());
    assert_eq!(sp0, main_hdr, "SP[0] must be the main executable's mach-header address");
    let argc = u64::from_le_bytes(b.read_guest(b.sp() + 8, 8).try_into().unwrap());
    assert_eq!(argc, 1, "argc at SP+8 must be 1");
}
