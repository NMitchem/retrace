// M2-cpuid Task 1: macOS 26 derives the guest's current CPU number from TPIDR_EL0 & 0xFFF and the
// cluster number from TPIDR_EL0 >> 12 (`_os_cpu_number` / `_os_cpu_cluster_number`). A single-vCPU
// guest is always cpu 0 / cluster 0, so TPIDR_EL0 must be 0 -- NOT the TSD pointer (TSD_IPA =
// 0x30000), which mis-reads as cluster 48 and blows libmalloc xzone's per-cluster segment-group
// index out of bounds. TPIDRRO_EL0 is the real TSD base (dyld/libSystem read errno + pthread-self
// through it) and must stay TSD_IPA. Covers both constructor sites: load_dynamic and restore
// (round-tripped through a real snapshot, the same path record/replay use).
use retrace_box::{Box_, TSD_IPA};
use retrace_guest::{parse_macho, slice_arm64e, HELLO_DYN, DYLD_PATH};
use retrace_trace::Event;

#[test]
fn dynamic_load_and_restore_set_cpu_identity_zero_and_preserve_tsd_base() {
    let exe = parse_macho(&std::fs::read(HELLO_DYN).unwrap());
    let dyld = parse_macho(slice_arm64e(&std::fs::read(DYLD_PATH).unwrap()));
    let b = Box_::load_dynamic(&exe, &dyld, &["hello_dyn".to_string()]);
    assert_eq!(b.tpidr_el0(), 0, "load_dynamic: TPIDR_EL0 must be 0 (cpu 0 / cluster 0)");
    assert_eq!(b.tpidrro_el0(), TSD_IPA, "load_dynamic: TPIDRRO_EL0 must remain the TSD base");

    let Event::Snapshot { regs, mem } = b.snapshot() else { panic!("snapshot must be Event::Snapshot") };
    drop(b); // one VM per process (HVF); must tear down before restore's Box_::restore creates a new one
    let r = Box_::restore(&mem, &regs);
    assert_eq!(r.tpidr_el0(), 0, "restore: TPIDR_EL0 must be 0 (cpu 0 / cluster 0)");
    assert_eq!(r.tpidrro_el0(), TSD_IPA, "restore: TPIDRRO_EL0 must remain the TSD base");
}
