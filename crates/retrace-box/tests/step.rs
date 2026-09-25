// Box_::step(): one instruction per call — a hardware single-step, or one below-the-trace
// emulation (the steppy MRS), each exactly one step; the window-ending svc is returned as
// Stop::Syscall, unconsumed. HVF allows one VM per process, so --test-threads=1 is mandatory.
use retrace_box::{Box_, SetBy, Stop};

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

/// M42 §3a/§3b, on (a), at the box:
/// - stepping the `ldxr` sets the shadow from the step exit's ISS.EX;
/// - the `cbnz`, a debug exit, leaves it;
/// - the `stxr` is emulated: the store lands, the shadow clears, and pc moves one instruction.
///
/// Before M42 the stepped `stxr` failed and the cell stayed 0 (t0 M2).
#[test]
fn stepping_a_load_exclusive_sets_the_shadow_and_its_store_lands() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::LLSC).unwrap());
    let mut b = Box_::load(&loaded);
    for i in 1..=3 { assert!(matches!(b.step(), Stop::Step), "step {i}"); } // adrp, add, movz
    assert_eq!(b.dbg_excl(), None, "nothing exclusive has retired yet");
    assert!(matches!(b.step(), Stop::Step));                                 // ldxr w10, [x9]
    let ex = b.dbg_excl().expect("the ldxr's retire sets the shadow");
    assert_eq!((ex.size, ex.pair, ex.loaded.as_slice(), ex.by), (4, false, &[0u8; 4][..], SetBy::Stepped));
    assert!(matches!(b.step(), Stop::Step));                                 // cbnz w10 (not taken)
    assert_eq!(b.dbg_excl().map(|e| e.va), Some(ex.va), "a debug exit leaves the shadow standing");
    let stx = b.pc();
    assert!(matches!(b.step(), Stop::Step));                                 // stxr wzr, w0, [x9]
    assert_eq!(b.dbg_excl(), None, "the emulated store clears the shadow");
    assert_eq!(b.pc(), stx + 4);
    assert_eq!(b.read_guest(b.va_to_ipa(ex.va).unwrap(), 4), vec![0x42, 0x42, 0, 0]);
}
