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
use retrace_box::{
    Box_, Disposition, SigAction, FRAME_LEN, FRAME_MCONTEXT_OFF, FRAME_UCONTEXT_OFF,
    PSTATE_USER_MASK, sigreturn_token,
};
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
    assert!(b.threads().is_blocked_for(0, 11), "the delivered signal blocks itself");
    assert!(b.threads().is_blocked_for(0, 6), "and everything in sa_mask");
}

#[test]
fn deliver_signal_honours_sa_nodefer() {
    let mut b = boxed();
    b.sigtable_mut()
        .set_action(11, handler(retrace_arch::SA_SIGINFO | retrace_arch::SA_NODEFER, 0));
    b.deliver_signal(11, 1, 0, 0, 0);
    assert!(!b.threads().is_blocked_for(0, 11), "SA_NODEFER means do not block the signal itself");
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
    b.threads_mut().set_mask_of(0, retrace_arch::SIG_SETMASK, 0b1010);
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
    b.threads_mut().set_altstack_of(0, Some((ss_sp, ss_size, 0)));
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
    b.threads_mut().set_altstack_of(0, Some((0x1_c000, 0x100, 0)));
    assert!(!b.on_altstack(), "installed, but sp is not inside it");
}

// ---- M12 t5: sigreturn ------------------------------------------------------------------------

#[test]
fn sigreturn_restores_the_pre_signal_state_including_vectors() {
    let mut b = boxed();
    b.vcpu_set_x(7, 0xcafe_f00d);
    b.vcpu_set_q(8, 0x1122_3344_5566_7788_99aa_bbcc_ddee_ff00);
    b.threads_mut().set_mask_of(0, retrace_arch::SIG_SETMASK, 0b0110);
    b.sigtable_mut().set_action(11, handler(retrace_arch::SA_SIGINFO, 0));
    let sp_before = b.regs_snapshot().sp_el0;
    let pc_before = b.position();
    let (writes, _) = b.deliver_signal(11, 1, 0, 0, 0);
    let uctx = writes[0].ipa + FRAME_UCONTEXT_OFF as u64;

    // The handler runs and clobbers everything it is allowed to.
    b.vcpu_set_x(7, 0xdead_beef);
    b.vcpu_set_q(8, 0);
    assert!(b.threads().is_blocked_for(0, 11));

    b.sigreturn_restore(uctx, sigreturn_token(uctx));

    let r = b.regs_snapshot();
    assert_eq!(r.x[7], 0xcafe_f00d, "x7 restored");
    assert_eq!(r.sp_el0, sp_before, "sp restored");
    assert_eq!(r.pc, pc_before, "pc restored to the pre-signal instruction");
    assert_eq!(b.vcpu_get_q(8), 0x1122_3344_5566_7788_99aa_bbcc_ddee_ff00,
        "VECTOR state restored — a handler is ordinary compiled code and will use NEON; without \
         this a handler that RETURNS silently corrupts the guest");
    assert_eq!(b.threads().mask_of(0), 0b0110, "the pre-signal mask is restored from uc_sigmask");
}

#[test]
#[should_panic(expected = "sigreturn token mismatch")]
fn sigreturn_rejects_a_bad_token() {
    let mut b = boxed();
    b.sigtable_mut().set_action(11, handler(retrace_arch::SA_SIGINFO, 0));
    let (writes, _) = b.deliver_signal(11, 1, 0, 0, 0);
    let uctx = writes[0].ipa + FRAME_UCONTEXT_OFF as u64;
    b.sigreturn_restore(uctx, 0);
}

// The security-shaped one: the frame is on the GUEST's stack, so the guest can rewrite __cpsr.
#[test]
fn sigreturn_sanitizes_pstate_and_cannot_be_asked_for_el1() {
    let mut b = boxed();
    b.sigtable_mut().set_action(11, handler(retrace_arch::SA_SIGINFO, 0));
    let (writes, _) = b.deliver_signal(11, 1, 0, 0, 0);
    let base = writes[0].ipa;
    let uctx = base + FRAME_UCONTEXT_OFF as u64;

    // Rewrite __ss.__cpsr in guest memory the way a hostile guest would: ask for EL1h with
    // interrupts masked, plus a legitimate NZCV.
    let cpsr_ipa = base + FRAME_MCONTEXT_OFF as u64 + 16 + 264;
    b.poke_guest(cpsr_ipa, &0x8000_03c5u32.to_le_bytes());

    b.sigreturn_restore(uctx, sigreturn_token(uctx));
    let cpsr = b.regs_snapshot().cpsr;
    assert_eq!(cpsr & !PSTATE_USER_MASK, 0,
        "only user-settable bits may survive: the guest must not be able to select an exception \
         level by writing its own signal frame");
    assert_eq!(cpsr & 0x8000_0000, 0x8000_0000, "the legitimate N flag still round-trips");
    // Concretely: mode bits back to EL0t, which is where this guest actually runs (lib.rs:886).
    assert_eq!(cpsr & 0xf, 0, "EL0t — the guest asked for EL1h (0x5) and did not get it");
}

// ---- The frame delivered at a SYSCALL boundary carries that syscall's RESULT.
//
// Measured against the real kernel, not reasoned about (spikes/sigraisex0.c): a process that
// raises a signal on itself with kill() enters its handler with a frame holding x0 = 0 (the
// syscall's RETURN value, not the pid it passed) and PSTATE.C CLEAR. The probe deliberately set
// C=1 and Z=1 immediately before kill(); the frame came back 0x40000000 — Z survived, C did not.
// So the kernel snapshots the context AFTER completing the syscall return, and a frame built from
// the raw trap state is a frame the guest's libc will read as "kill() failed".
//
// set_x0_err_and_return alone cannot fix this: it writes reg::CPSR, and the frame's PSTATE comes
// from SPSR_EL1. Both directions are pinned below so neither assertion can pass vacuously.

fn frame_x0(bytes: &[u8]) -> u64 {
    let o = FRAME_MCONTEXT_OFF + 16; // __ss within mcontext64, then __x[0] at its offset 0
    u64::from_le_bytes(bytes[o..o + 8].try_into().unwrap())
}
fn frame_cpsr(bytes: &[u8]) -> u32 {
    let o = FRAME_MCONTEXT_OFF + 16 + 264; // __ss.__cpsr, a u32 (the same offset sigreturn pokes)
    u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap())
}

#[test]
fn a_syscall_completed_before_delivery_puts_its_success_result_in_the_frame() {
    let mut b = boxed();
    b.sigtable_mut().set_action(11, handler(retrace_arch::SA_SIGINFO, 0));
    // Leave an ERROR in the trap state first, so a frame that merely copied the raw SPSR_EL1
    // would show C set — this assertion cannot pass by accident on a guest whose C was already 0.
    b.complete_syscall_before_delivery(38, true);
    b.complete_syscall_before_delivery(0, false); // the successful kill() that raised the signal
    let (writes, _) = b.deliver_signal(11, retrace_arch::SI_USER, 0, 0, 0);

    assert_eq!(frame_x0(&writes[0].bytes), 0,
        "the frame carries the syscall's RETURN value, not the argument that was in x0");
    assert_eq!(frame_cpsr(&writes[0].bytes) as u64 & retrace_arch::PSTATE_C, 0,
        "a successful syscall clears PSTATE.C, and the frame must carry the cleared flag or the \
         guest resumes reading its own successful raise as a failure");
}

#[test]
fn a_failed_syscall_completed_before_delivery_carries_its_error_flag_into_the_frame() {
    let mut b = boxed();
    b.sigtable_mut().set_action(11, handler(retrace_arch::SA_SIGINFO, 0));
    b.complete_syscall_before_delivery(0, false); // clear C first, for the same anti-vacuity reason
    b.complete_syscall_before_delivery(38, true);
    let (writes, _) = b.deliver_signal(11, retrace_arch::SI_USER, 0, 0, 0);

    assert_eq!(frame_x0(&writes[0].bytes), 38, "the errno the syscall returned");
    assert_eq!(frame_cpsr(&writes[0].bytes) as u64 & retrace_arch::PSTATE_C, retrace_arch::PSTATE_C,
        "an error sets PSTATE.C, and the frame must carry it");
}

// ---- M16 t6: delivery targets a thread, not the vCPU ------------------------------------------

/// M16 Task 6. `deliver_signal_to` sources FP/LR from `ThreadCtx.regs.x[29]`/`[30]`, because a
/// saved context has no separate FP/LR field. That is only correct if HVF aliases the registers.
#[test]
fn x29_and_x30_are_the_frame_pointer_and_link_register() {
    let mut b = boxed();
    b.vcpu_set_x(29, 0xF00D_0000_0000_0001);
    b.vcpu_set_x(30, 0xF00D_0000_0000_0002);
    let r = b.regs_snapshot();
    assert_eq!((r.x[29], r.x[30]), (0xF00D_0000_0000_0001, 0xF00D_0000_0000_0002));
    assert_eq!(b.dbg_fp_lr(), (0xF00D_0000_0000_0001, 0xF00D_0000_0000_0002),
        "HV_REG_FP/HV_REG_LR must alias X29/X30. If they do not, deliver_signal_to must carry them \
         as their own ThreadCtx fields instead of reading regs.x — measure, do not paper over it.");
}

/// M16 Task 6, the headline unit property: a signal delivered to a thread that is NOT running
/// lands on THAT thread's stack and redirects THAT thread, leaving the running one untouched.
///
/// This is the latent defect M16 closes, expressed at the smallest level that can express it.
#[test]
fn delivering_to_a_non_current_thread_leaves_the_running_one_alone() {
    let mut b = boxed();
    b.sigtable_mut().set_action(30, handler(retrace_arch::SA_SIGINFO, 0));

    // A second thread whose context is main's but on a DIFFERENT stack, so the frame's address
    // alone says which thread's stack it landed on.
    let mut ctx = b.save_ctx();
    let other_sp = ctx.regs.sp_el0 - 0x2000;
    ctx.regs.sp_el0 = other_sp;
    let elr_of_other = ctx.elr;
    let tid = b.threads_mut().spawn(ctx, (other_sp, 0));
    assert_eq!(tid, 1);

    let before = b.regs_snapshot();
    let (writes, resume_pc) = b.deliver_signal_to(tid, 30, retrace_arch::SI_USER, 0, 0, 0);

    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].ipa, other_sp - 128 - FRAME_LEN as u64,
        "the frame must land on thread 1's stack; landing on the running thread's stack IS the bug");
    assert_eq!(resume_pc, elr_of_other, "sigreturn returns to the TARGET's own next instruction");

    let after = b.regs_snapshot();
    assert_eq!((after.pc, after.sp_el0, after.x[0]), (before.pc, before.sp_el0, before.x[0]),
        "delivering to another thread must not redirect the running one");

    let t = b.threads().ctx_of(tid);
    assert_eq!(t.regs.pc, TRAMP, "thread 1 enters the trampoline when it is next scheduled");
    assert_eq!(t.regs.sp_el0, writes[0].ipa, "and resumes on the frame it was given");
    assert_eq!(t.regs.x[0], 0xabc0, "x0 = the catcher, in the TARGET's context");

    assert!(b.threads().is_blocked_for(tid, 30),
        "the signal is blocked for the handler's duration on the RECEIVING thread");
    assert!(!b.threads().is_blocked_for(0, 30),
        "and not on the thread that merely raised it");
}

/// M16 Task 6 fix round 1 (review finding 1): the alt stack half of the fix is untested by the
/// headline test above, which installs no alt stack on either thread. Reverting
/// `self.threads.altstack_of(tid)` / `self.on_altstack_of(tid)` to `(cur)` — exactly the "use the
/// running thread, not the target" defect this task exists to remove — left all 16 `deliver.rs`
/// tests green. This closes that gap: the TARGET has an alt stack installed and SA_ONSTACK, the
/// RUNNING thread has none, so only reading the target's altstack can land the frame correctly.
#[test]
fn delivering_to_a_non_current_thread_uses_that_threads_alt_stack() {
    let mut b = boxed();
    b.sigtable_mut().set_action(30, handler(retrace_arch::SA_SIGINFO | retrace_arch::SA_ONSTACK, 0));

    // Thread 1: main's context, moved to a different (unused) stack region, exactly as the
    // headline cross-thread test builds it.
    let mut ctx = b.save_ctx();
    let other_sp = ctx.regs.sp_el0 - 0x2000;
    ctx.regs.sp_el0 = other_sp;
    let tid = b.threads_mut().spawn(ctx, (other_sp, 0));
    assert_eq!(tid, 1);

    // The alt stack is installed on thread 1 ONLY. Thread 0 (the running thread) has none, so if
    // delivery reads the running thread's altstack instead of the target's, `choose_frame_base`
    // sees `None` and falls through to the "no alt stack" branch — a different, wrong base.
    let (ss_sp, ss_size) = (0x1_c000u64, 0x2000u64);
    b.threads_mut().set_altstack_of(tid, Some((ss_sp, ss_size, 0)));
    assert_eq!(b.threads().altstack_of(0), None, "the running thread must have no altstack of its own");

    let (writes, _) = b.deliver_signal_to(tid, 30, retrace_arch::SI_USER, 0, 0, 0);
    let base = writes[0].ipa;
    assert!((ss_sp..ss_sp + ss_size).contains(&base),
        "the frame must sit inside THREAD 1's alt stack [{ss_sp:#x}, {:#x}), got {base:#x} — landing \
         anywhere else means the target's altstack was not consulted", ss_sp + ss_size);
    assert_eq!(base, (ss_sp + ss_size - FRAME_LEN as u64) & !15);
    assert_eq!(b.threads().ctx_of(tid).regs.sp_el0, base, "the TARGET's saved sp is the frame base");

    let uc = &writes[0].bytes[FRAME_UCONTEXT_OFF..];
    assert_eq!(u32::from_le_bytes(uc[0..4].try_into().unwrap()), 1,
        "uc_onstack must report SS_ONSTACK for the TARGET thread's frame, not the running thread's \
         (unset) altstack state");
}

/// M16 Task 6 fix round 1 (review finding 1, second half): the same "which thread's state?"
/// question applies to `FrameInput.mask` — the frame's `uc_sigmask` must be the TARGET's
/// pre-signal mask, not the running thread's, or a handler that inspects/restores its mask via
/// `sigreturn` gets the wrong one back. The headline cross-thread test leaves both threads at
/// mask 0, so `mask_of(cur)` would pass there unnoticed; this test gives the two threads different
/// masks so only reading the target's is correct.
#[test]
fn delivering_to_a_non_current_thread_records_that_threads_pre_signal_mask_in_the_frame() {
    let mut b = boxed();
    b.sigtable_mut().set_action(30, handler(retrace_arch::SA_SIGINFO, 0));

    let mut ctx = b.save_ctx();
    let other_sp = ctx.regs.sp_el0 - 0x2000;
    ctx.regs.sp_el0 = other_sp;
    let tid = b.threads_mut().spawn(ctx, (other_sp, 0));
    assert_eq!(tid, 1);

    b.threads_mut().set_mask_of(0, retrace_arch::SIG_SETMASK, 0b0001);
    b.threads_mut().set_mask_of(tid, retrace_arch::SIG_SETMASK, 0b1010);

    let (writes, _) = b.deliver_signal_to(tid, 30, retrace_arch::SI_USER, 0, 0, 0);
    let uc = &writes[0].bytes[FRAME_UCONTEXT_OFF..];
    assert_eq!(u32::from_le_bytes(uc[4..8].try_into().unwrap()), 0b1010,
        "uc_sigmask is what sigreturn restores on the TARGET; it must be the TARGET's pre-signal \
         mask (0b1010), not the running thread's (0b0001)");
}

/// M16 Task 6's fail-loud boundary: a second signal to a thread that has already been redirected
/// into a handler and has not run since would stack a frame without the kernel's queueing
/// semantics. Nested delivery is unmodelled — this must panic, not silently corrupt the frame.
#[test]
fn a_second_signal_to_an_unrun_redirected_thread_fails_loud() {
    let mut b = boxed();
    b.sigtable_mut().set_action(30, handler(retrace_arch::SA_SIGINFO, 0));
    let mut ctx = b.save_ctx();
    ctx.regs.sp_el0 -= 0x2000;
    let tid = b.threads_mut().spawn(ctx, (0, 0));
    b.deliver_signal_to(tid, 30, retrace_arch::SI_USER, 0, 0, 0);
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        b.deliver_signal_to(tid, 30, retrace_arch::SI_USER, 0, 0, 0);
    })).is_err();
    assert!(panicked, "stacking a frame on a context that never ran the first must not be silent");
}

/// M16 Task 6 fix round 1 (review finding 5): `deliver_signal_to` must refuse a target that is not
/// `Runnable`. Redirecting a Blocked thread would overwrite the very saved context its blocking
/// syscall is waiting to resume through. Not reachable through any product caller at this commit
/// (nothing yet passes `tid != cur`), but the guard must exist before the next task makes it
/// reachable — without it the failure mode is silent corruption, not a panic.
#[test]
#[should_panic(expected = "is Blocked(")]
fn delivering_to_a_blocked_thread_fails_loud() {
    let mut b = boxed();
    b.sigtable_mut().set_action(30, handler(retrace_arch::SA_SIGINFO, 0));
    let mut ctx = b.save_ctx();
    ctx.regs.sp_el0 -= 0x2000;
    let tid = b.threads_mut().spawn(ctx, (0, 0));

    // Block thread `tid` itself (`block` always blocks the CURRENT thread), then switch back to 0
    // so the target is blocked while some OTHER thread is running — the shape the guard exists for.
    b.threads_mut().switch_to(tid);
    b.threads_mut().block(retrace_box::thread::BlockReason::Wait { addr: 0xdead_0000 });
    b.threads_mut().switch_to(0);

    b.deliver_signal_to(tid, 30, retrace_arch::SI_USER, 0, 0, 0);
}

/// M16 Task 6 fix round 1 (review finding 5), the other half: redirecting an Exited thread would
/// mutate a dead table entry that has no saved context left to resume into a handler.
#[test]
#[should_panic(expected = "has Exited(")]
fn delivering_to_an_exited_thread_fails_loud() {
    let mut b = boxed();
    b.sigtable_mut().set_action(30, handler(retrace_arch::SA_SIGINFO, 0));
    let mut ctx = b.save_ctx();
    ctx.regs.sp_el0 -= 0x2000;
    let tid = b.threads_mut().spawn(ctx, (0, 0));

    b.threads_mut().switch_to(tid);
    b.threads_mut().exit_current(0);
    b.threads_mut().switch_to(0);

    b.deliver_signal_to(tid, 30, retrace_arch::SI_USER, 0, 0, 0);
}
