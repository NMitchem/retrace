//! M47 §3d, box level: `Box_::guest_mac_syscall` classifies from guest memory on a static box (MMU
//! off, so VA == IPA), and `host_amfi_dyld_policy` asks the host. These pin what the fixtures may
//! not reach (Review Focus 3 and 5): an `int` call's upper half, an unterminated policy, and
//! unmodelled names. `gitprims_e2e`'s `rpath_dyn` and `sbxpath_dyn` cover the arms end to end.
//! No test reaches `read_guest_cstr`'s shared-cache page-in, kept per spec §11 item 4: t0 M2(c)
//! found every policy and operation string resident at the trap, and M47 t3 measured 0 page-ins
//! recording rpath_dyn, sbxpath_dyn, hello_dyn and madv_dyn.
use retrace_box::{Box_, MacSyscall};

fn tb() -> Box_ {
    Box_::load(&retrace_guest::parse_macho(&std::fs::read(retrace_guest::HELLO).unwrap()))
}

fn args(policy: u64, call: u64, arg: u64) -> [u64; 8] { [policy, call, arg, 0, 0, 0, 0, 0] }

/// Lay out a policy name at `t - 0x400`, a Sandbox struct at `t - 0x300` whose +16 points at an
/// operation name at `t - 0x100`, and return `t`.
fn sandbox(b: &mut Box_, op: &[u8]) -> u64 {
    let t = b.stack_top();
    b.poke_guest(t - 0x400, b"Sandbox\0");
    b.poke_guest(t - 0x300 + 16, &(t - 0x100).to_le_bytes());
    b.poke_guest(t - 0x100, &[op, b"\0"].concat());
    t
}

/// Review Focus 3: `call` is a C `int`; bit 32 is not the kernel's.
#[test]
fn amfi_is_classified_with_its_flags_and_the_ipa_of_out_flags() {
    let mut b = tb();
    let t = b.stack_top();
    b.poke_guest(t - 0x400, b"AMFI\0");
    b.poke_guest(t - 0x300, &[2u64.to_le_bytes(), (t - 0x200).to_le_bytes()].concat());
    for call in [0x5a, 0x5a | 1 << 32] {
        assert_eq!(b.guest_mac_syscall(args(t - 0x400, call, t - 0x300)),
                   Ok(MacSyscall::AmfiDyldPolicy { in_flags: 2, out_ipa: t - 0x200 }), "call {call:#x}");
    }
}

#[test]
fn sandbox_is_answered_by_its_operation() {
    let mut b = tb();
    let t = sandbox(&mut b, b"syscall-unix");
    assert_eq!(b.guest_mac_syscall(args(t - 0x400, 2, t - 0x300)), Ok(MacSyscall::SandboxCheck { errno: 14 }));
    let t = sandbox(&mut b, b"file-write-data");
    assert_eq!(b.guest_mac_syscall(args(t - 0x400, 2, t - 0x300)), Ok(MacSyscall::SandboxCheck { errno: 22 }));
}

/// t0, after H7: Sandbox call 4 (`sandbox_container_path_for_pid`) is answered ENOTSUP and writes
/// nothing through its nested `buf`, whatever the pid.
#[test]
fn sandbox_container_path_is_answered_enotsup() {
    let mut b = tb();
    let t = b.stack_top();
    b.poke_guest(t - 0x400, b"Sandbox\0");
    b.poke_guest(t - 0x300, &[0x14e3eu64.to_le_bytes(), 0u64.to_le_bytes(), (t - 0x200).to_le_bytes(), 0x400u64.to_le_bytes()].concat());
    assert_eq!(b.guest_mac_syscall(args(t - 0x400, 4, t - 0x300)), Ok(MacSyscall::SandboxContainerPath { errno: 45 }));
}

/// Review Focus 5: `copyinstr` into a 32-byte buffer finds no NUL and fails; so does the model.
#[test]
fn an_unterminated_policy_is_refused_as_copyinstr_would() {
    let mut b = tb();
    let t = b.stack_top();
    b.poke_guest(t - 0x400, &[b'A'; 40]);
    let e = b.guest_mac_syscall(args(t - 0x400, 0x5a, t - 0x300)).unwrap_err();
    assert!(e.starts_with("M47: __mac_syscall policy name: no NUL within 32 bytes"), "{e}");
}

/// Review Focus 5: an unmodelled policy, and an unmeasured Sandbox operation, are refused by name.
#[test]
fn an_unmodelled_policy_or_operation_is_refused_by_name() {
    let mut b = tb();
    let t = b.stack_top();
    b.poke_guest(t - 0x400, b"Quarantine\0");
    let e = b.guest_mac_syscall(args(t - 0x400, 2, t - 0x300)).unwrap_err();
    assert!(e.starts_with("M47: unmodelled __mac_syscall policy \"Quarantine\" call 0x2"), "{e}");
    let t = sandbox(&mut b, b"file-read-data");
    let e = b.guest_mac_syscall(args(t - 0x400, 2, t - 0x300)).unwrap_err();
    assert!(e.starts_with("M47: unmeasured Sandbox operation \"file-read-data\""), "{e}");
}

/// R4: the host's answer about THIS process (the test binary, ad-hoc signed by the cargo runner, as
/// the recorder is). Bit 0 is `AMFI_DYLD_OUTPUT_ALLOW_AT_PATH`, the bit an `@rpath` load needs. The
/// whole word is host policy and is not asserted; the probe host answered 0x1df (amfi.out).
#[test]
fn the_host_answers_amfi_for_this_process_with_at_path_allowed() {
    let flags = retrace_box::host_amfi_dyld_policy(0).expect("AMFI's dyld policy for an ad-hoc process");
    assert_eq!(flags & 1, 1, "ALLOW_AT_PATH: {flags:#x}");
}
