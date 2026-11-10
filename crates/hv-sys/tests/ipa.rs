// M57 §3c: the guest IPA width. Needs the hypervisor entitlement for the VM test (the cargo runner signs it).
use hv_sys::{max_ipa_bits, MemFlags, Vm};
use std::alloc::{alloc_zeroed, dealloc, Layout};

#[test]
fn this_mac_allows_a_40_bit_guest_ipa() {
    assert!(max_ipa_bits().unwrap() >= 40);
}

#[test]
fn a_40_bit_vm_maps_above_64_gib_where_the_default_vm_cannot() {
    let layout = Layout::from_size_align(0x4000, 0x4000).unwrap();
    let host = unsafe { alloc_zeroed(layout) };
    assert!(!host.is_null());
    {
        let vm = Vm::create().unwrap();
        assert!(vm.map(host, 0x90_0000_0000, 0x4000, MemFlags::RWX).is_err(), "the default 36-bit VM cannot");
    }
    {
        let vm = Vm::create_with_ipa(40).unwrap();
        vm.map(host, 0x90_0000_0000, 0x4000, MemFlags::RWX).expect("a 40-bit VM can");
        vm.unmap(0x90_0000_0000, 0x4000).unwrap();
    }
    unsafe { dealloc(host, layout) };
}
