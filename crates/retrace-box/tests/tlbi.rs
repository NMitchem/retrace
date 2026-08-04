// M9 t2. The TLBI oracle: flush_guest_tlb must run the EL1 stub to completion on a live box and
// leave the caller's architectural state untouched, so it is safe to call in the middle of a guest
// run (the same contract sign_slots already gives the cache pager). Task 1's spike (spikes/tlbi.c)
// proved the hazard is real: a hand-flipped data->code leaf genuinely stale-faults (EC=0x20,
// IFSC=0x0F) without a flush, and running `tlbi vmalle1` on the guest vCPU itself clears it.
use retrace_box::Box_;
use retrace_guest::{parse_macho, HELLO};

#[test]
fn flush_guest_tlb_preserves_architectural_state() {
    let bytes = std::fs::read(HELLO).expect("read hello");
    let loaded = parse_macho(&bytes);
    let mut b = Box_::load(&loaded);

    let before = b.regs_snapshot();
    let elr_before = b.position(); // ELR_EL1: not in Regs, the register most at risk from a trap
    b.flush_guest_tlb();
    let after = b.regs_snapshot();

    assert_eq!(before, after,
        "flush_guest_tlb must restore every register it saved — a mid-run caller must see no \
         disturbance (same contract as sign_slots)");
    assert_eq!(b.position(), elr_before,
        "flush_guest_tlb clobbered ELR_EL1 (the EL1 stub's terminating hvc traps straight to EL2, \
         so this should never move)");
}

#[test]
fn flush_guest_tlb_is_repeatable() {
    let bytes = std::fs::read(HELLO).expect("read hello");
    let loaded = parse_macho(&bytes);
    let mut b = Box_::load(&loaded);
    // Two flushes in a row must both reach the stub's terminating hvc (the bounded runner panics
    // otherwise), proving the scratch page and stub survive reuse.
    b.flush_guest_tlb();
    b.flush_guest_tlb();
}

#[test]
fn flush_guest_tlb_survives_checkpoint_restore() {
    // M9 t3 fix. `checkpoint()` captures EVERY backing (Box_::checkpoint), so once a flush has
    // installed the TLBI stub, TLBI_STUB_IPA is part of `state.mem` and `from_checkpoint` re-maps
    // it as a live backing. Before the fix, `from_checkpoint` unconditionally reset
    // `tlbi_stub_ready: false`, so a later `flush_guest_tlb()` re-entered `ensure_tlbi_stub()` and
    // called `hv_vm_map` over an IPA already stage-2 mapped — a panic (`hv_vm_map` rejects an
    // overlapping range). This pins the checkpointed-seek-across-a-flush shape: a checkpoint taken
    // after at least one FIXED-exec-over-live-backing map (this milestone's whole point) followed
    // by a second such map after restore is exactly dyld's multi-segment load strategy.
    let bytes = std::fs::read(HELLO).expect("read hello");
    let loaded = parse_macho(&bytes);
    let mut b = Box_::load(&loaded);

    b.flush_guest_tlb(); // installs the stub; TLBI_STUB_IPA is now one of `b`'s backings

    let checkpoint = b.checkpoint();
    drop(b); // one VM per process (HVF); must tear down before from_checkpoint creates a new one
    let mut restored = Box_::from_checkpoint(&checkpoint);

    let before = restored.regs_snapshot();
    let elr_before = restored.position();
    restored.flush_guest_tlb(); // must not panic — this is the regression
    let after = restored.regs_snapshot();

    assert_eq!(before, after,
        "flush_guest_tlb after a checkpoint restore must restore every register it saved");
    assert_eq!(restored.position(), elr_before,
        "flush_guest_tlb after a checkpoint restore clobbered ELR_EL1");
}
