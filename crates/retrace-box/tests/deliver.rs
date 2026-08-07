// M12 t4: Box_::deliver_signal builds the frame in guest memory and enters the trampoline.
//
// FIXTURE NOTE. The plan's `boxed()` helper built a box from `Box_::restore` with one synthetic
// region at 0x1_0000 and `sp_el0 = 0x2_0000`. That cannot work, for three independent reasons:
//   1. `restore` calls `stack_geometry_from_memory`, which PANICS unless the regions look like a
//      real static or dynamic guest stack. One region at 0x1_0000 matches neither shape.
//   2. `sp_el0 = 0x2_0000` is outside that region ([0x1_0000, 0x1_4000)), so the frame write would
//      panic "outside any mapped region".
//   3. `restore` never sets ELR_EL1, which is where `deliver_signal` reads the guest's pc from, so
//      `resume_pc` would come back 0 rather than the trapping instruction.
// So this uses the pattern every other box test uses — load the real static guest and run it to its
// first syscall — which gives genuine ELR_EL1/SP_EL0/SPSR_EL1 state instead of a hand-set fiction.
use retrace_box::{Box_, Disposition, SigAction, FRAME_LEN, FRAME_UCONTEXT_OFF, sigreturn_token};
use retrace_guest::{parse_macho, HELLO};

const TRAMP: u64 = 0x1_0100; // opaque here: nothing ERETs in this test, so it need not be mapped

/// The real static guest, run to its first syscall trap so ELR_EL1, SP_EL0 and SPSR_EL1 hold
/// state the guest actually established.
fn boxed() -> Box_ {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load_with_pac(&loaded, false);
    let _ = b.run();
    b
}

fn handler(flags: u32, mask: u32) -> SigAction {
    SigAction { disp: Disposition::Handler(0xabc0), tramp: TRAMP, mask, flags }
}

#[test]
fn deliver_signal_writes_the_frame_and_enters_the_trampoline() {
    let mut b = boxed();
    b.sigtable_mut().set_action(11, handler(retrace_arch::SA_SIGINFO, 0));
    let sp_before = b.regs_snapshot().sp_el0;
    let pc_before = b.position(); // ELR_EL1 — the instruction the guest trapped on
    let (writes, resume_pc) = b.deliver_signal(11, 1, 0xdead_0000, 0x9200_0046, 0xdead_0000);

    assert_eq!(writes.len(), 1, "the frame is one contiguous write");
    assert_eq!(writes[0].bytes.len(), FRAME_LEN);
    let base = writes[0].ipa;
    assert_eq!(base % 16, 0, "arm64 sp must be 16-byte aligned");
    assert_eq!(base, sp_before - 128 - FRAME_LEN as u64);

    let r = b.regs_snapshot();
    assert_eq!(r.sp_el0, base, "sp IS the frame base (measured: sp == x3)");
    // reg::PC, not ELR_EL1: `set_x0_err_and_return` resumes by writing reg::PC from ELR_EL1, so
    // reg::PC is what the vCPU actually resumes at. Setting ELR_EL1 here would be a no-op and the
    // guest would resume at the trampoline it trapped into, never reaching the handler.
    assert_eq!(r.pc, TRAMP, "entered the TRAMPOLINE, not the handler");
    assert_eq!(r.x[0], 0xabc0, "x0 = the catcher");
    assert_eq!(r.x[1], 30, "x1 = infostyle UC_FLAVOR");
    assert_eq!(r.x[2], 11, "x2 = the signal number");
    assert_eq!(r.x[3], base, "x3 = siginfo*, which is the frame base");
    assert_eq!(r.x[4], base + FRAME_UCONTEXT_OFF as u64, "x4 = ucontext*");
    assert_eq!(r.x[5], sigreturn_token(base + FRAME_UCONTEXT_OFF as u64), "x5 = the token");
    assert_eq!(resume_pc, pc_before, "resume_pc is where the guest was — a fault re-executes it");

    // The bytes really landed in guest memory, not just in the returned Vec.
    assert_eq!(b.read_guest(base, FRAME_LEN), writes[0].bytes);
}

// The handler must run at EL0. `set_x0_err_and_return` restores CPSR from SPSR_EL1 for exactly this
// reason, and the plan's entry sequence omitted it — which would have left the handler at EL1.
#[test]
fn deliver_signal_returns_the_guest_to_its_pre_trap_pstate() {
    let mut b = boxed();
    b.sigtable_mut().set_action(11, handler(retrace_arch::SA_SIGINFO, 0));
    let spsr = b.spsr();
    b.deliver_signal(11, 1, 0, 0, 0);
    assert_eq!(b.regs_snapshot().cpsr, spsr,
        "CPSR must come from SPSR_EL1, or the handler runs at EL1 instead of EL0");
}

#[test]
fn deliver_signal_blocks_the_signal_and_its_sa_mask_for_the_handler() {
    let mut b = boxed();
    b.sigtable_mut().set_action(11, handler(retrace_arch::SA_SIGINFO, 1 << 5 /* SIGABRT */));
    b.deliver_signal(11, 1, 0, 0, 0);
    assert!(b.sigtable().is_blocked(11), "the delivered signal blocks itself");
    assert!(b.sigtable().is_blocked(6), "and everything in sa_mask");
}

#[test]
fn deliver_signal_honours_sa_nodefer() {
    let mut b = boxed();
    b.sigtable_mut()
        .set_action(11, handler(retrace_arch::SA_SIGINFO | retrace_arch::SA_NODEFER, 0));
    b.deliver_signal(11, 1, 0, 0, 0);
    assert!(!b.sigtable().is_blocked(11), "SA_NODEFER means do not block the signal itself");
}

#[test]
fn deliver_signal_honours_sa_resethand() {
    let mut b = boxed();
    b.sigtable_mut()
        .set_action(11, handler(retrace_arch::SA_SIGINFO | retrace_arch::SA_RESETHAND, 0));
    b.deliver_signal(11, 1, 0, 0, 0);
    assert_eq!(b.sigtable().action(11).disp, Disposition::Dfl,
        "SA_RESETHAND resets to SIG_DFL as the handler is entered");
}

#[test]
fn the_frame_records_the_pre_signal_mask_not_the_handler_mask() {
    let mut b = boxed();
    b.sigtable_mut().set_mask(retrace_arch::SIG_SETMASK, 0b1010);
    b.sigtable_mut().set_action(11, handler(retrace_arch::SA_SIGINFO, 0));
    let (writes, _) = b.deliver_signal(11, 1, 0, 0, 0);
    let uc = &writes[0].bytes[FRAME_UCONTEXT_OFF..];
    assert_eq!(u32::from_le_bytes(uc[4..8].try_into().unwrap()), 0b1010,
        "uc_sigmask is what sigreturn restores — it must be the mask from BEFORE delivery, or the \
         handler's own blocking becomes permanent");
}

// The case the plan omits entirely, and the one Task 2's `on_alt` field exists for: the frame must
// land INSIDE the alternate stack and the frame must SAY so, or a handler that queries
// sigaltstack(NULL, &old) is told it is on its normal stack while it demonstrably is not.
#[test]
fn deliver_signal_runs_on_the_alternate_stack_and_says_so_in_the_frame() {
    let mut b = boxed();
    // A sub-range of the guest's own mapped stack, so the frame lands in real backing.
    let (ss_sp, ss_size) = (0x1_c000u64, 0x2000u64);
    b.sigtable_mut().set_altstack(Some((ss_sp, ss_size, 0)));
    b.sigtable_mut().set_action(11, handler(retrace_arch::SA_SIGINFO | retrace_arch::SA_ONSTACK, 0));

    let (writes, _) = b.deliver_signal(11, 1, 0, 0, 0);
    let base = writes[0].ipa;
    assert!((ss_sp..ss_sp + ss_size).contains(&base),
        "the frame must sit inside the alt stack [{ss_sp:#x}, {:#x}), got {base:#x}", ss_sp + ss_size);
    assert_eq!(base, (ss_sp + ss_size - FRAME_LEN as u64) & !15);
    assert_eq!(b.regs_snapshot().sp_el0, base);

    let uc = &writes[0].bytes[FRAME_UCONTEXT_OFF..];
    assert_eq!(u32::from_le_bytes(uc[0..4].try_into().unwrap()), 1,
        "uc_onstack must report SS_ONSTACK — choose_frame_base's on_alt has to reach the frame");
}

#[test]
fn on_altstack_is_false_when_the_guest_is_on_its_normal_stack() {
    let mut b = boxed();
    assert!(!b.on_altstack(), "no alt stack installed at all");
    b.sigtable_mut().set_altstack(Some((0x1_c000, 0x100, 0)));
    assert!(!b.on_altstack(), "installed, but sp is not inside it");
}
