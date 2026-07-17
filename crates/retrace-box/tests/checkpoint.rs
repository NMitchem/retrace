// Box_::checkpoint()/from_checkpoint() round-trip: a MID-RUN capture (not landmark-0, where
// Box_::restore's defaults would coincidentally look correct) must restore byte-identical state
// with zero further execution — registers (incl. FP/SIMD), all memory, and the internal bookkeeping
// (reservations/mmap_next/bootstrap_port/cache_installed/...) that Box_::restore only gets right at
// landmark 0.
use retrace_box::{Box_, Stop};
use retrace_guest::{parse_macho, STEPPY};

#[test]
fn checkpoint_round_trip_is_lossless_mid_run() {
    let loaded = parse_macho(&std::fs::read(STEPPY).unwrap());
    let mut b = Box_::load(&loaded);

    // Exercise non-default internal state via Box_'s own public bump-allocator/cache/port methods
    // (no real syscall forwarding needed — these are exactly what record/replay's dispatch calls).
    let _reserved = b.guest_vm_reserve(0x9999_0000, 0x4000, true);
    let _mapped = b.guest_mmap(0x4000);
    b.install_cache_pager();
    let _port = b.mint_bootstrap_port();
    // Single-step a few instructions (crosses steppy's timebase-emulated MRS, advancing
    // synthetic_tsc — the field most likely to be silently reset by a naive restore).
    for i in 1..=5u64 {
        assert!(matches!(b.step(), Stop::Step), "step {i}");
    }

    let original_regs = b.dbg_regs();
    let original_fp = b.dbg_fp_regs();
    let original_internal = b.dbg_internal_state();
    let original_mem = match b.snapshot() {
        retrace_trace::Event::Snapshot { mem, .. } => mem,
        _ => unreachable!(),
    };

    let checkpoint = b.checkpoint();
    drop(b); // one VM per process (HVF); must tear down before from_checkpoint creates a new one
    let restored = Box_::from_checkpoint(&checkpoint);

    assert_eq!(restored.dbg_regs(), original_regs, "registers diverged on restore");
    assert_eq!(restored.dbg_fp_regs(), original_fp, "FP/SIMD state diverged on restore");
    assert_eq!(restored.dbg_internal_state(), original_internal,
        "internal bookkeeping (reservations/mmap_next/bootstrap_port/cache/...) diverged on restore");
    assert!(restored.diff_memory(&original_mem).is_none(), "memory diverged on restore");
}
