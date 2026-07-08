// The headline M2 gate: a normal dynamically-linked C program records and replays with zero
// divergence, dyld having mapped the shared cache itself.
mod util;
// BLOCKED (tracked). Two prior walls have FALLEN and the run now advances deep into libSystem's
// image initializers:
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
// NEW WALL (a DISTINCT subsystem — guest MMU VA-size vs the arm64e cache's PAC layout): objc's
// class realization faults in _map_images_nolock -> addClassTableEntry, dereferencing a mis-stripped
// class/isa pointer. hello_dyn is a plain arm64 (NOT arm64e) process, so libobjc STRIPS the arm64e
// shared-cache isa pointers with a compile-time ISA_MASK (47-bit) instead of authenticating them.
// retrace's guest runs with TCR_EL1.T0SZ=28 (a 36-bit VA), so the guest's PACDA places the isa
// signature in bits [54:36]; the ISA_MASK strip (bits [46:0]) leaves the signature bits in [46:36]
// -> a poisoned pointer -> data abort. retrace's re-signing is itself correct (in-guest sign+AUTDA
// round-trips exactly); the mismatch is purely that real macOS uses a 47-bit user VA (PAC above bit
// 47, cleanly masked away) while retrace uses 36-bit. Clearing it needs a 47-bit guest VA (T0SZ=17,
// a 3-level 16 KiB page-table walk instead of today's 2-level) or an arm64e guest — core MMU work,
// distinct from mach_msg2 servicing. Full anatomy in task-m2mach-7-report.md. Ignored (not deleted)
// so it stays the living M2 gate, re-runnable with `--ignored` as the approach evolves.
#[ignore = "blocked BEYOND mach_msg2 on a guest-VA/arm64e-PAC boundary: arm64 objc ISA_MASK-strips cache isas but retrace's 36-bit guest VA (T0SZ=28) puts the PAC below bit 47 -> poisoned deref in objc class realization; needs a 47-bit guest VA (T0SZ=17) or arm64e guest — see task-m2mach-7-report.md"]
#[test]
fn hello_dyn_records_and_replays() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    assert_eq!(rec.stdout, b"hi\n");
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, b"hi\n");
}
