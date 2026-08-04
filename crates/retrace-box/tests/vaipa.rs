// M6: the read-only stage-1 walker. Today every mapping is identity (VA == IPA) — these tests
// pin that AND the None-on-unmapped soundness the software watch check relies on.
use retrace_box::{Box_, COMMPAGE_IPA, TSD_IPA};
use retrace_guest::{parse_macho, slice_arm64e, CRASH, CRASHY, DYLD_PATH};

// The crash fixtures' wild pointer: bit 46 set, so L1 index 0x400 — the only valid L1 entry is
// index 0 (build_tables), so the walk must terminate at level 1. Same constant as c/crashy.c.
const GARBAGE_VA: u64 = 0x4000_DEAD_0000;

#[test]
fn walker_is_identity_on_mapped_vas_and_none_on_unmapped() {
    // Static guest: MMU is on (load sets SCTLR_MMU_ON) over the identity map.
    let loaded = parse_macho(&std::fs::read(CRASH).unwrap());
    let text = loaded.segments.iter().find(|s| s.exec).expect("crash has an exec segment").vaddr;
    let b = Box_::load(&loaded);
    // 0x1C000 = STACK_TOP_IPA - GRANULE, the static stack backing's base (load maps it there);
    // `text` is the guest's own executable segment. Both live in L2 blocks promoted to L3 tables
    // (block 0 by the trampoline, the text block by the segment itself), so this is the L3 path.
    assert_eq!(b.va_to_ipa(0x1C000), Some(0x1C000), "stack region, identity");
    assert_eq!(b.va_to_ipa(text), Some(text), "guest text, identity");
    // Bit-46 VA: L1 index 0x400 is invalid -> None (the crash fixture's GARBAGE_VA).
    assert_eq!(b.va_to_ipa(GARBAGE_VA), None);
    // Pins the L1 index shift itself — no other assertion here constrains it, since every mapped
    // VA above is < 2^36 and so lands in L1[0] under a correct >>36 OR a buggy >>37 shift alike.
    // VA 1<<36 selects L1[1], which is empty (only L1[0] is a table, via build_tables) -> None.
    // A buggy >>37 would instead select L1[0] -> L2[0] (promoted by the trampoline) -> L3[0]
    // (identity page 0, valid) -> Some(0), failing this assertion.
    assert_eq!(b.va_to_ipa(1u64 << 36), None);
    // Beyond the 47-bit space -> None.
    assert_eq!(b.va_to_ipa(1u64 << 47), None);
}

#[test]
fn walker_is_identity_across_the_dynamic_layout() {
    let exe = parse_macho(&std::fs::read(CRASHY).unwrap());
    let dyld = parse_macho(slice_arm64e(&std::fs::read(DYLD_PATH).unwrap()));
    let main_hdr = exe.segments.iter()
        .find(|s| s.data.len() >= 4 && s.data[0..4] == [0xcf, 0xfa, 0xed, 0xfe])
        .map(|s| s.vaddr).expect("crashy carries a mach header");
    let b = Box_::load_dynamic(&exe, &dyld, &["crashy".to_string()]);
    // TSD_IPA and the exe's mach header sit in blocks promoted to L3 tables (the trampoline resp.
    // the exe's own text) — the L3 path; COMMPAGE_IPA is in block 2047, which nothing promotes —
    // the L2 BLOCK path. Both must translate identity.
    for va in [TSD_IPA, main_hdr, COMMPAGE_IPA] {
        assert_eq!(b.va_to_ipa(va), Some(va), "identity at {va:#x}");
    }
    assert_eq!(b.va_to_ipa(GARBAGE_VA), None);
}
