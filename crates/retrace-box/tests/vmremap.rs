// M39 t3: the stage-1 alias behind mach_vm_remap. A static guest is enough: its code page is a
// page-granular ATTR_CODE entry, and `guest_vm_map(anywhere)` is what a vm_allocate becomes.
// The observable is the stage-1 walk itself (`va_to_ipa`): after the alias the TARGET VA
// resolves to the SOURCE's IPA while its neighbours stay identity. Executing through the alias
// is vmremap_e2e's job (it needs a running guest).
use retrace_box::Box_;
use retrace_guest::{parse_macho, HELLO};

#[test]
fn a_remapped_page_resolves_to_the_source_ipa_and_neighbours_stay_identity() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    let text = loaded.entry & !0x3fff;                       // the guest's code page
    let region = b.guest_vm_map(0, 0xc000, true, false);     // 3 anon RW pages, ANYWHERE
    let target = region + 0x4000;
    assert_eq!(b.va_to_ipa(target), Some(target), "identity before the alias");
    assert_eq!(b.va_to_ipa(text), Some(text), "the source is identity-mapped");

    // A kernel-placed image's RX text: cur r-x (5), max r-x (5) — Task 2's SELF measurement.
    assert_eq!(b.guest_vm_remap(target, 0x4000, text), (target, 5, 5));

    assert_eq!(b.va_to_ipa(target), Some(text), "the target now walks to the source's IPA");
    assert_eq!(b.va_to_ipa(region), Some(region), "the page below is untouched");
    assert_eq!(b.va_to_ipa(region + 0x8000), Some(region + 0x8000), "the page above is untouched");
    assert_eq!(b.va_to_ipa(text), Some(text), "the source is untouched");
}

#[test]
fn a_guest_allocated_source_reports_the_kernels_protections_for_its_band() {
    // The other measured shape (Task 2's FFI line, max=7): a source the GUEST allocated lives at
    // or above MMAP_BASE (> NANO_BAND_START) and the kernel gives such mappings max VM_PROT_ALL.
    // Here the source is an anon RW page, so cur is rw- (3) — the attribute-derived half.
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    let src_region = b.guest_vm_map(0, 0x4000, true, false);    // one anon RW page, in the mmap band
    let region = b.guest_vm_map(0, 0xc000, true, false);
    let target = region + 0x4000;
    assert_eq!(b.guest_vm_remap(target, 0x4000, src_region), (target, 3, 7));
    assert_eq!(b.va_to_ipa(target), Some(src_region));
}

#[test]
#[should_panic(expected = "not a page multiple")]
fn a_non_page_multiple_size_is_refused() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    let text = loaded.entry & !0x3fff;
    let region = b.guest_vm_map(0, 0xc000, true, false);
    b.guest_vm_remap(region + 0x4000, 0x100, text);
}
