// Task 4 (M2-cache): the guest signing oracle. `Box_::sign_slots` re-signs a batch of auth
// slots with the GUEST's fixed PAC keys by executing `pacia`/`pacda` INSIDE the guest VM, and
// must do so WITHOUT disturbing the caller's guest state (the cache pager calls it mid-run).
//
// This test signs the spike's worked-example slot (`spikes/pacsign.c` / `cacheprobe.c`:
// .02.dylddata page1 off 0x22d0), proves PAC engaged (signed != target) and that the in-guest
// inverse (`autda`, same modifier) recovers the original target, and proves full save/restore:
// the vCPU's architectural registers are byte-identical after `sign_slots` + `authenticate`.
//
// Run under a bounded external timeout — a W^X / stub mistake can hang `hv_vcpu_run`:
//   perl -e '$p=fork;if(!$p){setpgrp;exec@ARGV or exit 127}$SIG{ALRM}=sub{kill"-KILL",$p;exit 124};alarm 60;wait;exit($?>>8)' \
//     cargo test -p retrace-box --test sign_oracle -- --test-threads=1
use retrace_arch::{SYS_EXIT, SYS_WRITE};
use retrace_box::{AuthSlot, Box_, Stop};
use retrace_guest::{parse_macho, HELLO};
use retrace_trace::{Event, Regs};

// ptrauth ABI blend (mirrors `cache::blend`): low 48 bits of the address, diversity in bits [63:48].
fn blend(addr: u64, diversity: u16) -> u64 {
    (addr & 0x0000_FFFF_FFFF_FFFF) | ((diversity as u64) << 48)
}

fn regs(b: &Box_) -> Regs {
    match b.snapshot() {
        Event::Snapshot { regs, .. } => regs, // x0..x30, PC, SP_EL0, CPSR
        _ => unreachable!("snapshot() always returns Event::Snapshot"),
    }
}

#[test]
fn sign_slots_signs_roundtrips_and_preserves_state() {
    // Load a static guest and run to its first syscall (write): the vCPU is now stopped mid-syscall
    // with real ELR_EL1/SPSR_EL1/PC/SP/GPRs — exactly the state the cache pager calls sign_slots
    // from. `load` already set the fixed PAC keys and SCTLR_EL1.EnIA/EnDA, so signing is live.
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    match b.run() {
        Stop::Syscall { num, .. } => assert_eq!(num, SYS_WRITE, "expected write() first"),
        Stop::Other { esr } => panic!("guest faulted before first syscall: esr={esr:#x}"),
    }

    // Capture the FULL prior architectural state to prove sign_slots restores it byte-for-byte.
    let before = regs(&b); // x0..x30, PC, SP_EL0, CPSR
    let elr_before = b.position(); // ELR_EL1 (not in Regs; the reg most at risk from an svc trap)

    // The spike's worked example (cacheprobe.c: .02.dylddata page1 off 0x22d0): a DA-key auth slot
    // with a blended modifier. target_va = value_add + runtime_offset (slide 0) = 0x1ec2f15c8;
    // modifier = blend(slot_slid_va, diversity) = blend(0x1ec06c2d0, 0x6ae1).
    let target = 0x1_ec2f_15c8u64;
    let slot_slid_va = 0x1_ec06_c2d0u64;
    let modifier = blend(slot_slid_va, 0x6ae1); // == 0x6ae1_0001_ec06_c2d0
    let slot = AuthSlot { offset: 0, target_va: target, key_is_data: true /* DA */, modifier };

    let signed = b.sign_slots(&[slot]);
    assert_eq!(signed.len(), 1);
    assert_ne!(signed[0], target, "PAC not engaged: signed == target (keys off / stub wrong?)");

    // Round-trip: an in-guest `autda` with the SAME (ptr, modifier) recovers the original target.
    let recovered = b.authenticate(&[(signed[0], modifier, true)]);
    assert_eq!(recovered[0], target, "round-trip failed: autda did not recover target_va");

    // Save/restore: the architectural registers are byte-identical after sign_slots + authenticate.
    assert_eq!(regs(&b), before, "sign_slots/authenticate clobbered vCPU registers (x/PC/SP/CPSR)");
    assert_eq!(b.position(), elr_before, "sign_slots/authenticate clobbered ELR_EL1");

    // Behavioral proof ELR_EL1/SPSR_EL1 were restored: resume the guest from where write() trapped
    // (set_x0_and_return reads the just-restored ELR_EL1/SPSR_EL1) and let it run to its next
    // syscall, exit(0). A clobbered ELR/SPSR would resume at the wrong PC/PSTATE and fault.
    b.set_x0_and_return(6); // write() returned 6 bytes
    match b.run() {
        Stop::Syscall { num, args } => {
            assert_eq!(num, SYS_EXIT, "guest did not resume correctly after sign_slots");
            assert_eq!(args[0], 0);
        }
        Stop::Other { esr } => panic!("guest faulted resuming after sign_slots: esr={esr:#x}"),
    }
}
