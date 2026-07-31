// M7 t6: the guest's PAC posture is DERIVED, not assumed.
//
// macOS enables pointer authentication per process, only for arm64e main executables. retrace
// enabled it unconditionally for every guest, so dyld's unconditional `paciza` in TLV setup — a NOP
// in a real plain-arm64 process — really signed, and hello_rust's plain `blr x8` through the
// resulting descriptor branched to a signed pointer (M7's wall).
//
// The dangerous failure mode is NOT a wrong posture (that faults early and loudly). It is a posture
// MISMATCH between the four SCTLR install sites: record PAC-off against replay PAC-on never faults,
// it diverges only at the final full-memory compare, or mis-seeks silently through from_checkpoint.
// These tests exist to make that mismatch fail here instead of there.
//
// One VM per process: --test-threads=1, and every box is dropped before the next is created.
use retrace_arch::SYS_EXIT;
use retrace_box::{Box_, Stop};
use retrace_guest::{parse_macho, HELLO, PACGUEST};
use retrace_trace::Event;

// pacguest.s: signs with `pacia`, authenticates with `autia`, and leaves x0 = 0 iff PAC was ENGAGED
// and the round-trip recovered the pointer; x0 = 1 if signing was a no-op (PAC disabled).
fn pacguest_exit_arg(mut b: Box_) -> u64 {
    match b.run() {
        Stop::Syscall { num, args } if num == SYS_EXIT => args[0],
        Stop::Syscall { .. } => panic!("unexpected syscall"),
        Stop::Other { esr } => panic!("guest faulted esr={esr:#x}"),
        Stop::Fault { pc, esr, far } => panic!("guest crashed pc={pc:#x} esr={esr:#x} far={far:#x}"),
        Stop::Step => unreachable!("run() does not single-step"),
    }
}

#[test]
fn a_plain_arm64_guest_gets_pac_disabled_like_the_real_os() {
    let loaded = parse_macho(&std::fs::read(PACGUEST).unwrap());
    assert_eq!(loaded.cpusubtype & 0x00ff_ffff, 0, "pacguest must be plain arm64 for this test to mean anything");
    // x0 == 1 is pacguest reporting "signing was a NO-OP" — which is exactly what a plain-arm64
    // process gets from the real OS, and what retrace must now give it.
    assert_eq!(pacguest_exit_arg(Box_::load(&loaded)), 1,
               "a plain-arm64 guest must run with PAC DISABLED (pac* behaving as NOPs)");
}

#[test]
fn an_explicitly_pac_on_box_still_signs() {
    // The escape hatch the PAC tests need: every guest this repo can build is plain arm64, so
    // without this the re-signer, the signing oracle and the strip-on-FPAC arm are untestable.
    let loaded = parse_macho(&std::fs::read(PACGUEST).unwrap());
    assert_eq!(pacguest_exit_arg(Box_::load_with_pac(&loaded, true)), 0,
               "an explicitly PAC-on box must sign and authenticate (x0=0)");
}

#[test]
fn restore_rederives_the_same_posture_from_the_snapshot() {
    // restore() gets only (regions, regs) — no sysregs, no cpusubtype. It re-derives the posture
    // from the mach header the snapshot already contains at EXE_BASE. This is the site where a
    // mismatch would fail LATE (a divergence at the final memory compare, not a fault).
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let b = Box_::load(&loaded);
    let recorded = b.dbg_pac_enabled();
    let (mem, regs) = match b.snapshot() {
        Event::Snapshot { mem, regs } => (mem, regs),
        _ => unreachable!("snapshot() always returns Event::Snapshot"),
    };
    drop(b); // one VM per process — tear down before restore() creates the next one
    let r = Box_::restore(&mem, &regs);
    assert_eq!(r.dbg_pac_enabled(), recorded, "restore() must re-derive the RECORD run's posture");
    assert!(!recorded, "hello is plain arm64, so the posture must be PAC-off");
}

#[test]
fn from_checkpoint_carries_the_posture() {
    // The mid-run twin. Its snapshot is taken while the guest is running, so its header is NOT
    // pristine by construction — that is why the posture is stored in BoxState rather than
    // re-derived here.
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load_with_pac(&loaded, true);
    let _ = b.run(); // reach the first syscall so the checkpoint is genuinely mid-run
    let st = b.checkpoint();
    drop(b);
    let r = Box_::from_checkpoint(&st);
    assert!(r.dbg_pac_enabled(),
            "from_checkpoint must restore the captured posture, not re-derive from the header");
}
