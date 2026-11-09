// M6: the read-only stage-1 walker. Today every mapping is identity (VA == IPA) — these tests
// pin that AND the None-on-unmapped soundness the software watch check relies on.
use retrace_box::{Box_, COMMPAGE_IPA, TSD_IPA};
use retrace_guest::{parse_macho, slice_arm64e, CRASH, CRASHY, DYLD_PATH, HELLO};

// The crash fixtures' wild pointer: bit 46 set, so L1 index 0x400 — the valid L1 entries are
// 0..16 (build_tables, M57), so the walk must terminate at level 1. Same constant as c/crashy.c.
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
    // Pins the L1 index shift itself: every mapped VA above is < 2^36 and so lands in L1[0] under a
    // correct >>36 or a buggy >>37 shift alike. VA 1<<36 is L1[1] -> L2 page 1 -> an identity data block.
    // VA 1<<40 is L1[16], which is empty (L1[0..16] are tables, M57), so it is None; a buggy >>37
    // sends it to L1[8], which is valid, and fails the None.
    assert_eq!(b.va_to_ipa(1u64 << 36), Some(1u64 << 36));
    assert_eq!(b.va_to_ipa(1u64 << 40), None);
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

/// M51 §3h (C3): the pre-forward check answers `EFAULT` where the program cannot reach the pointer,
/// and passes it where it can.
#[test]
fn the_pre_forward_check_answers_efault_where_the_program_cannot_reach_the_pointer() {
    let loaded = parse_macho(&std::fs::read(CRASH).unwrap());
    let text = loaded.segments.iter().find(|s| s.exec).expect("crash has an exec segment").vaddr;
    let mut b = Box_::load(&loaded);
    let buf = |p: u64| { let mut a = [0u64; 8]; a[1] = p; a[2] = 16; a };
    let efault = Err(retrace_arch::EFAULT);
    assert_eq!(b.prepare_forward_pointers(retrace_arch::SYS_READ, &buf(GARBAGE_VA)), efault, "no translation");
    assert_eq!(b.prepare_forward_pointers(retrace_arch::SYS_READ, &buf(0xB_0000_0000)), efault, "translates, nothing backs it");
    assert_eq!(b.prepare_forward_pointers(retrace_arch::SYS_READ, &buf(text)), efault, "code is not writable by the program");
    assert_eq!(b.prepare_forward_pointers(retrace_arch::SYS_WRITE, &buf(text)), Ok(()), "but it is readable");
    assert_eq!(b.prepare_forward_pointers(retrace_arch::SYS_READ, &buf(0x1C000)), Ok(()), "the stack is writable");
    assert_eq!(b.prepare_forward_pointers(retrace_arch::SYS_READ, &buf(0x10)), Ok(()),
        "below the first page the kernel answers, as natively");
}

/// M51 §3h (R12, C3): a reserved page the program never touched is reachable natively, so the check
/// commits it: only the first page the pointer names, and a second call changes nothing.
#[test]
fn the_pre_forward_check_commits_a_reserved_page_and_is_idempotent() {
    let mut b = Box_::load(&parse_macho(&std::fs::read(HELLO).unwrap()));
    let base = b.guest_vm_reserve(0, 0x100000, true);
    let mut args = [0u64; 8];
    args[1] = base + 0x40000;
    args[2] = 0x8000;
    assert_eq!(b.prepare_forward_pointers(retrace_arch::SYS_READ, &args), Ok(()));
    assert!(b.host_span_for_test(base + 0x40000).is_some(), "the page the pointer names is present");
    assert!(b.host_span_for_test(base + 0x44000).is_none(), "the rest of the count is neither checked nor committed");
    assert_eq!(b.prepare_forward_pointers(retrace_arch::SYS_READ, &args), Ok(()), "a second call");
    assert!(!b.commit_reserved_page(base + 0x40000), "the page was already present");
}

/// M51 §3h: with every guest pointer checked before it, `read_guest` on an address nothing backs is
/// retrace's bug, never a wall.
#[test]
fn read_guest_on_an_unbacked_address_is_retraces_bug_not_a_wall() {
    let b = Box_::load(&parse_macho(&std::fs::read(HELLO).unwrap()));
    let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.read_guest(0xB_0000_0000, 8)))
        .expect_err("a read nothing backs must not return");
    let msg = payload.downcast_ref::<String>().cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .expect("a String or &str payload");
    assert!(retrace_arch::Unsupported::from_marked(&msg).is_none(), "not a wall: {msg}");
    assert!(msg.contains("retrace's bug"), "{msg}");
}
