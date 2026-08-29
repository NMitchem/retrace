use retrace_box::*;
#[test]
fn el0_svc_reaches_vmm_via_trampoline() {
    let bytes = std::fs::read(retrace_guest::HELLO).unwrap();
    let loaded = retrace_guest::parse_macho(&bytes);
    let mut b = Box_::load(&loaded);
    // First stop must be the guest's write() syscall issued from EL0.
    match b.run() {
        Stop::Syscall { num, args } => {
            assert_eq!(num, retrace_arch::SYS_WRITE);
            assert_eq!(args[0], 1);          // fd = stdout
            assert_eq!(args[2], 6);          // len = 6
        }
        Stop::Other { esr } => panic!("expected SVC-via-trampoline, got esr=0x{esr:x}"),
        Stop::Fault { pc, esr, far } => panic!("guest crashed pc=0x{pc:x} esr=0x{esr:x} far=0x{far:x}"),
        Stop::Step => unreachable!("run() does not single-step"),
    }
}

/// True iff `insn` is an `HVC #imm16` (`1101 0100 000 imm16 000 10`), i.e. an instruction that traps
/// to EL2 rather than one that raises an undefined-instruction exception at EL1.
fn hvc_imm(insn: u32) -> Option<u32> {
    (insn & 0xffe0_001f == 0xd400_0002).then_some((insn >> 5) & 0xffff)
}

/// Every byte of the EL1 vector table must be a *trapping* instruction, not `UDF #0`.
///
/// M23 t1. Each of the 16 slots is 0x80 bytes but only its first 4 hold `hvc #0`; the remaining 0x7c
/// were left zero, which decodes as `UDF #0`. When execution falls through a slot head (measured:
/// ~0.27% of vector entries, M23 measurements S2) the guest then executes that `UDF` **at EL1**,
/// which overwrites ELR_EL1/SPSR_EL1 with the trampoline's own address and vectors to VBAR+0x200 —
/// destroying the original exception's identity and reporting the notorious `pc=0x4204`, an address
/// inside retrace's trampoline that has nothing to do with the guest's actual fault. That single
/// misattribution accounted for 13 of the 20 Apple-system-binary failures in the M22 breadth sweep.
///
/// The padding's immediate must be NONZERO so that a fall-through is distinguishable at the VM exit
/// (ESR_EL2 ISS) from a genuine vector entry through a slot head — that distinguishability is what
/// makes the fall-through countable, and hence checkable across record and replay.
///
/// Asserted on BOTH construction paths, because there are two vector-table sites (`load_with_pac`
/// and `load_dynamic`) and `record-dyn` uses the second: patching only one is silently ineffective.
fn assert_vectors_all_trap(b: &Box_, path: &str) {
    for slot in 0..16u64 {
        let base = TRAMPOLINE_IPA + slot * 0x80;
        let head = u32::from_le_bytes(b.read_guest(base, 4).try_into().unwrap());
        assert_eq!(hvc_imm(head), Some(0),
            "{path}: slot {slot} head at {base:#x} must be `hvc #0`, got {head:#010x}");
        for w in 1..0x20u64 {
            let a = base + w * 4;
            let insn = u32::from_le_bytes(b.read_guest(a, 4).try_into().unwrap());
            let imm = hvc_imm(insn).unwrap_or_else(|| panic!(
                "{path}: vector padding at {a:#x} is {insn:#010x}, not a trapping HVC \
                 (0x00000000 = `UDF #0` is the M22 pc=0x4204 masking defect)"));
            assert_ne!(imm, 0,
                "{path}: vector padding at {a:#x} is `hvc #0`, indistinguishable from a genuine \
                 vector entry — a fall-through must be countable");
        }
    }
}

#[test]
fn vector_padding_traps_rather_than_undefs_static() {
    let bytes = std::fs::read(retrace_guest::HELLO).unwrap();
    let loaded = retrace_guest::parse_macho(&bytes);
    assert_vectors_all_trap(&Box_::load(&loaded), "load_with_pac");
}

#[test]
fn vector_padding_traps_rather_than_undefs_dynamic() {
    let exe = retrace_guest::parse_macho(&std::fs::read(retrace_guest::HELLO_DYN).unwrap());
    let dyld = retrace_guest::parse_macho(
        retrace_guest::slice_arm64e(&std::fs::read(retrace_guest::DYLD_PATH).unwrap()));
    let b = Box_::load_dynamic(&exe, &dyld, &["hello_dyn".to_string()]);
    assert_vectors_all_trap(&b, "load_dynamic");
}

/// A fall-through onto the trapping padding must be COUNTED, so that record and replay can be
/// compared (M23 t1). Without a count the recovery is silent self-healing — record and replay would
/// agree with each other while the anomaly vanished, which is the one failure a determinism oracle
/// structurally cannot see. Real fall-throughs are rare (~0.27% of vector entries) and not
/// guest-controllable, so this parks the vCPU exactly where one lands and resumes.
#[test]
fn a_fall_through_onto_vector_padding_is_counted() {
    let bytes = std::fs::read(retrace_guest::HELLO).unwrap();
    let loaded = retrace_guest::parse_macho(&bytes);
    let mut b = Box_::load(&loaded);
    assert_eq!(b.fall_throughs(), 0, "a fresh box has taken no fall-throughs");
    // VBAR+0x400 is the lower-EL/AArch64 synchronous slot; +4 is its first padding word — precisely
    // where execution lands when it runs past the slot head. EL1h with DAIF masked is the PSTATE a
    // vector entry leaves behind (the same value the TLBI stub uses to run at EL1).
    let mut ctx = b.save_ctx();
    ctx.regs.pc = TRAMPOLINE_IPA + 0x404;
    ctx.regs.cpsr = 0x3C5;
    b.load_ctx(&ctx);
    let _ = b.run();
    assert_eq!(b.fall_throughs(), 1, "executing the vector padding must count as a fall-through");
}

/// ...and a NORMAL vector entry must not be counted. The guest's `write()` reaches the VMM through
/// the slot head's `hvc #0`; counting that too would make the record/replay comparison in t2 compare
/// syscall counts instead of anomalies, and it would never fail.
#[test]
fn normal_vector_entries_are_not_counted_as_fall_throughs() {
    let bytes = std::fs::read(retrace_guest::HELLO).unwrap();
    let loaded = retrace_guest::parse_macho(&bytes);
    let mut b = Box_::load(&loaded);
    assert!(matches!(b.run(), Stop::Syscall { .. }), "guest reaches its write() via the slot head");
    assert_eq!(b.fall_throughs(), 0, "a genuine vector entry is not a fall-through");
}
