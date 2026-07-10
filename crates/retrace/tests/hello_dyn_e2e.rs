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
// The B-family autdb wall FELL in M2-bfam t1 (strip-on-FPAC arm): addClassTableEntry+0x70's
// `autdb x16, x17` (DATA-B key, disc 0xc93a) FPAC-faults (EC=0x1c) because the A-family-only cache
// re-signer can't reach DB-signed cache pointers; `Box_::try_emulate_fpac_auth` now emulates a
// successful authenticate by stripping Rd to its 47-bit canonical VA and skipping the insn (a pure,
// deterministic, below-the-trace op — nothing enters the trace). This carries the run PAST
// addClassTableEntry (verified: only three autdb strips occur, all recovering well-formed libobjc
// __AUTH_CONST pointers — not garbage; the strip is mathematically correct).
//
// NEW WALL (a DISTINCT subsystem — objc SHARED-CACHE PREOPTIMIZATION, not PAC): the run now reaches
// _objc_init -> map_images -> map_images_nolock -> realizeClassWithoutSwift and aborts (objc _objc_fatal
// "realized class 0x1ec2f1618 has corrupt data pointer: malloc_size(0x1ed950f80) = 0"). This is
// objc's validateAlreadyRealizedClass (realizeClassWithoutSwift+1188): objc is DYNAMICALLY realizing
// a class that lives in the shared cache (0x1ec2f1618 is in libobjc __DATA_DIRTY) whose data() bits
// correctly authenticate/strip to a PREOPTIMIZED, cache-resident class_rw_t in libobjc __AUTH_CONST
// (0x1ED950140..0x1ED950FC8) — legitimately NOT a malloc heap allocation, so malloc_size == 0 and
// objc fatals. A real process never hits this: it takes the objc shared-cache-preoptimization fast
// path (classes are pre-realized in the cache; realizeClassWithoutSwift is never called on them).
// That fast path is disabled in the guest — the re-signed + demand-paged cache no longer presents as
// a valid/trusted objc-optimized cache (the M2-cache re-signer rewrites the very pointers objc's
// preoptimization vouches for), so libobjc falls back to dynamic realization, which is incompatible
// with the preoptimized cache-resident metadata. Clearing this is NOT another aut to strip or syscall
// to forward: it needs the guest cache to present valid objc preoptimization (objc_opt header +
// selector/class/protocol tables + cache-trust), a distinct, larger subsystem than B-family strip.
// Full anatomy in .superpowers/sdd/task-m2bfam-2-report.md. Ignored (not deleted) so it stays the
// living M2 gate, re-runnable with `--ignored` as the approach evolves.
#[ignore = "blocked BEYOND the B-family autdb wall (emulated in M2-bfam t1's strip-on-FPAC arm, which carries past addClassTableEntry+0x70) on objc SHARED-CACHE PREOPTIMIZATION: _objc_init->map_images->realizeClassWithoutSwift dynamically realizes cache-resident classes and aborts in validateAlreadyRealizedClass ('realized class 0x1ec2f1618 has corrupt data pointer: malloc_size(0x1ed950f80)=0') because data() correctly points to a preoptimized class_rw_t in libobjc __AUTH_CONST (not a malloc heap alloc). A real process skips this via objc preoptimization, which is disabled in-guest since the re-signed/demand-paged cache no longer presents as a trusted objc-optimized cache; needs valid guest-side objc preoptimization (objc_opt + hash tables + cache-trust) — see task-m2bfam-2-report.md"]
#[test]
fn hello_dyn_records_and_replays() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    assert_eq!(rec.stdout, b"hi\n");
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, b"hi\n");
}
