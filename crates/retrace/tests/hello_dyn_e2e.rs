// The headline M2 gate: a normal dynamically-linked C program records and replays with zero
// divergence, dyld having mapped the shared cache itself.
mod util;
// BLOCKED (tracked). Three prior walls have FALLEN and the run now advances deep into libSystem's
// image initializers and into objc class realization itself:
//   * The shared-cache wall fell in M2-cache (re-signing demand-pager): dyld maps the re-signed
//     cache and executes thousands of guest-key-re-signed arm64e pointers with zero FPAC faults.
//   * The mach_msg2 wall fell across M2-mach: libmalloc's mandatory "pointer range" reservation
//     and the whole mach_msg2 (trap -47) MIG surface are now serviced against the GUEST address
//     space — _kernelrpc_mach_vm_map (4811) serviced, vm_reclaim (4822) stubbed unavailable, and
//     (M2-mach Task 7) the private task_restartable subsystem (register 8000 / synchronize 8001)
//     stubbed KERN_SUCCESS (a single-vCPU deterministic replay has no preemption). A latent
//     nano-band soundness bug (bump base collided with libmalloc's FIXED 24-GiB reservation) was
//     also fixed (MMAP_BASE moved above [0x4_0000_0000, 0xA_0000_0000)). The run advances from
//     ~177 traps to ~208.
//   * The isa-STRIP wall fell in M2-va47: widening the guest VA to 47 bits (TCR_EL1.T0SZ=17, an
//     added L1 table) moves the hardware PAC signature into bits [54:47], entirely above objc's
//     47-bit ISA_MASK, so libobjc's plain-arm64 isa strip in addClassTableEntry is now lossless —
//     the old poisoned-isa data abort is GONE and execution advances past the isa load. Proven by
//     the strip47 micro-test (an objc-style 47-bit strip of a pacda-signed pointer: RED under the
//     old 36-bit VA, GREEN under 47-bit) and confirmed empirically in the live run.
// NEW WALL (a DISTINCT subsystem — objc B-family PAC re-signing, not VA size): 8 instructions past
// the now-successful isa load, addClassTableEntry+0x70 executes `autdb x16, x17`, authenticating
// objc's class data()/bits pointer (`class_data_bits_t` at `cls+0x20`, a compiler
// `__ptrauth`-qualified field) with the DATA-B key (APDBKey), address-diversified and blended with
// discriminator 0xc93a. This hardware-faults FPAC (EC=0x1c) because retrace's M2-cache re-signer is
// A-family only: the dyld v5 slide-info format cannot express B-family keys at all
// (`cache.rs::decode5` carries a single IA/DA `key_is_data` bit), and the in-guest signing stub
// implements only `pacia`/`pacda`/`autia`/`autda` — no `pacib`/`pacdb`/`autib`/`autdb`. So this
// DB-signed cache pointer keeps its host-key signature and fails to authenticate under the guest's
// DB key. Clearing it needs B-family (DB/IB) PAC re-signing — extending the re-signer and the
// in-guest signing stub, likely objc-structure-aware — a distinct, larger subsystem from widening
// the VA. Full anatomy in task-m2va47-2-report.md. Ignored (not deleted) so it stays the living M2
// gate, re-runnable with `--ignored` as the approach evolves.
#[ignore = "blocked BEYOND the isa-strip wall (fixed in M2-va47's 47-bit guest VA) on objc B-family PAC: addClassTableEntry+0x70 autdb-authenticates the class data() pointer with the DATA-B key (disc 0xc93a) -> FPAC (EC=0x1c), because retrace's M2-cache re-signer is A-family only (v5 slide-info can't express B-family keys; the signing stub has no pacib/pacdb/autib/autdb); needs B-family (DB/IB) PAC re-signing — see task-m2va47-2-report.md"]
#[test]
fn hello_dyn_records_and_replays() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    assert_eq!(rec.stdout, b"hi\n");
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, b"hi\n");
}
