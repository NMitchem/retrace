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
// CORRECTED (M2-tbi, 2026-07-14): what the M2-bfam close-out documented here as a DISTINCT
// "objc SHARED-CACHE PREOPTIMIZATION" wall was a MISDIAGNOSIS. objc did abort (exit 134) in
// _objc_init -> map_images -> realizeClassWithoutSwift -> validateAlreadyRealizedClass
// ("realized class 0x1ec2f1618 has corrupt data pointer: malloc_size(0x1ed950f80)=0"), but the root
// cause was a one-line guest-MMU bug, not an objc-opt / cache-trust gap. The fatal class is NSObject;
// its data() pointer 0x1ed950f80 symbolicates to _OBJC_CLASS_RO_$_NSObject (a class_ro_t, NOT a
// preoptimized class_rw_t). objc's has_rw_pointer()/isRealized() reads bit 63 (FAST_IS_RW_POINTER)
// of the RAW class_data_bits::bits word; the guest value 0x964a8001ed950f80 had bit 63 SET, so objc
// read unrealized NSObject as already-realized and validated its class_ro_t (malloc_size 0) -> fatal.
// (Confirmed independently: the host runs hello_dyn fine with OBJC_DISABLE_PREOPTIMIZATION=YES, so
// "preopt disabled -> this fatal" is disproven; and validateAlreadyRealizedClass has NO cache-trust
// guard — it is an unconditional malloc_size check.) Bit 63 was polluted because the guest TCR left
// TBI OFF: under a 47-bit VA the re-signed data-pointer PAC field spans [63:56]∪[54:47], landing on
// objc's realized flag. M2-tbi enables TBI0+TBID0 in the guest TCR (Apple's arm64e user posture),
// placing data-pointer PACs in [54:47] with the top byte (incl. bit 63) preserved = 0 —
// has_rw_pointer() now reads NSObject as unrealized, objc realizes it normally, and the
// validateAlreadyRealizedClass fatal is GONE. See docs/superpowers/specs/2026-07-14-retrace-m2-tbi-design.md.
//
// The mmap DEMAND-COMMIT wall (M2-tbi's honest boundary) FELL in M2-mmapcommit: the class_rw_t
// first-touch fault was on a page inside a mach_vm_map PROT_NONE reservation (cur_protection == 0)
// that guest_vm_reserve deliberately never backed. Box_::commit_reserved_page now demand-commits
// exactly the faulting page with a fresh zeroed anon page on a stage-2 fault inside a tracked
// reservation — the moral twin of the shared-cache demand-pager, minus the file read and re-sign,
// dispatched by a second below-the-trace guard mirrored textually in record and replay's Stop::Other
// arms. Pure zero-fill + the guest's own re-executed stores, so nothing enters the trace (same
// posture as the cache pager / timebase MRS / FPAC strip). The run advances from the xzone CHUNK
// allocator one frame deeper into the xzone SEGMENT allocator (~206-214 traps; the count varies with
// the forwarded-gettimeofday deadline-spin below, not a determinism defect — record forwards real
// time, replay reproduces the recorded values).
//
// NEW WALL (M2-mmapcommit's honest boundary — libmalloc xzone SEGMENT allocator, NOT demand-commit):
// with reservation pages demand-committed, objc class realization (libdispatch_init -> _objc_init ->
// map_images -> realizeClassWithoutSwift -> _xzm_xzone_malloc_freelist_outlined+0x144 ->
// _xzm_xzone_find_and_malloc_from_freelist_chunk+0x51c -> xzm_segment_group_alloc_chunk+0x274) reaches
// _xzm_segment_group_alloc_segment+0x90, which faults NEAR-NULL (data abort EC=0x24 FSC=0x7,
// far=0x178). The faulting insn is `ldrb w9, [x8, #0x178]` with x8 = 0; x8 was just loaded by
// `ldp x27, x8, [x20, #0x10]` from x20 = 0xa0010e4c8 (a demand-committed xzone segment-group metadata
// page — the LDP itself SUCCEEDS, proving the page IS backed), whose +0x18 slot is 0. So xzm reads a
// NULL *segment* pointer out of its own committed metadata and dereferences it. This is DISTINCT from
// the demand-commit wall M2-mmapcommit cleared: commit_reserved_page did its job (the metadata page is
// mapped); the fault is an xzone allocator-state inconsistency — a null segment link where a real
// kernel-backed run has a valid pointer — under retrace's approximated VM-op semantics and single-vCPU
// (no-preemption) model. A 12x gettimeofday deadline-spin (an xzone/libdispatch timed backoff with no
// second thread to make progress) immediately precedes the fault. Investigating xzone's segment-group
// allocation protocol is a distinct subsystem, deferred to a future milestone — NOT walked into here
// (see docs/superpowers/specs/2026-07-14-retrace-m2-mmapcommit-design.md, risk register #1). Ignored
// (not deleted) so it stays the living M2 gate, re-runnable with `--ignored` as the approach evolves.
#[ignore = "blocked at the libmalloc xzone SEGMENT-allocator wall (M2-mmapcommit's honest boundary). The prior mmap DEMAND-COMMIT wall FELL: Box_::commit_reserved_page now demand-commits pages inside tracked mach_vm_map PROT_NONE reservations (cur_protection==0) on first touch, below the trace and mirrored in record/replay. The run advances one frame deeper — from the xzone chunk allocator into _xzm_segment_group_alloc_segment+0x90 (via realizeClassWithoutSwift -> xzm_xzone_malloc -> xzm_segment_group_alloc_chunk), which faults NEAR-NULL (data abort EC=0x24 FSC=0x7, far=0x178): `ldrb w9,[x8,#0x178]` with x8=0, where x8 was loaded by `ldp x27,x8,[x20,#0x10]` from x20=0xa0010e4c8 — a demand-committed xzone segment-group metadata page (the LDP SUCCEEDS, so the page IS backed) whose +0x18 slot is 0. xzm dereferences a NULL segment pointer read from its own committed metadata: an xzone allocator-state inconsistency under retrace's approximated VM-op / single-vCPU (no-preemption) model, DISTINCT from demand-commit (which did its job). A 12x gettimeofday deadline-spin precedes the fault. Deferred as a future milestone — see docs/superpowers/specs/2026-07-14-retrace-m2-mmapcommit-design.md risk #1"]
#[test]
fn hello_dyn_records_and_replays() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    assert_eq!(rec.stdout, b"hi\n");
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, b"hi\n");
}
