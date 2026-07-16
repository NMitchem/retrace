// Box_::step(): one instruction per call — a hardware single-step, or one below-the-trace
// emulation (the steppy MRS), each exactly one step; the window-ending svc is returned as
// Stop::Syscall, unconsumed. HVF allows one VM per process, so --test-threads=1 is mandatory.
use retrace_box::{Box_, Stop};

fn load_steppy() -> Box_ {
    Box_::load(&retrace_guest::parse_macho(&std::fs::read(retrace_guest::STEPPY).unwrap()))
}

#[test]
fn step_advances_one_insn_at_a_time() {
    let mut b = load_steppy();
    let pc0 = b.pc();
    for i in 1..=4u64 {
        assert!(matches!(b.step(), Stop::Step), "step {i}");
        assert_eq!(b.pc(), pc0 + 4 * i, "pc after step {i}");
    }
}

#[test]
fn step_crosses_the_mrs_as_one_step() {
    let mut b = load_steppy();
    for _ in 0..4 { assert!(matches!(b.step(), Stop::Step)); }
    let at_mrs = b.pc();
    assert!(matches!(b.step(), Stop::Step), "the MRS is one step (emulated or native)");
    assert_eq!(b.pc(), at_mrs + 4);
}

#[test]
fn step_reaches_window_end_as_unconsumed_syscall() {
    let mut b = load_steppy();
    let mut steps = 0u64;
    loop {
        match b.step() {
            Stop::Step => { steps += 1; assert!(steps < 64, "runaway"); }
            Stop::Syscall { num, .. } => { assert_eq!(num, 1, "exit(0) svc"); break; }
            other => panic!("unexpected: {other:?}"),
        }
    }
    // steppy.s: nop×4 + mrs + nop×3 + (mov x0 / mov x16, hello.s's exit-sequence setup) = 10 steps,
    // then the exit svc surfaces as an unconsumed Stop::Syscall.
    assert_eq!(steps, 10, "4 nops + mrs + 3 nops + 2 exit-setup movs before the exit svc");
}

// An SS that leaks past Box_::step() (armed but run() drives the vcpu) must fail loud, not
// masquerade as Stop::Other. dbg_leak_ss arms MDSCR_EL1.SS + PSTATE.SS the way step() does;
// run() must hit the fail-loud Ec::SoftStep arm.
#[test]
#[should_panic(expected = "software-step exception outside Box_::step()")]
fn unarmed_step_exception_fails_loud() {
    let mut b = load_steppy();
    b.dbg_leak_ss();
    b.run();
}
