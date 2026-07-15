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
// The xzone NULL-METADATA wall (M2-mmapcommit's honest boundary) FELL in M2-carveout: it was a
// placement gap, not an allocator-state bug. libmalloc's guarded-metadata protocol reserves a ~5 MiB
// PROT_NONE band, mach_vm_deallocate's a 1 MiB carveout hole inside it, then commits its zone metadata
// with mach_vm_map(VM_FLAGS_ANYWHERE, hint = reservation base) — which the real kernel is FORCED to
// place in the hole (the band around it is occupied). M2-carveout t1 taught the box to punch holes in
// tracked reservations (subtract_reservations) and to first-fit ANYWHERE-with-hint forward past
// reservations (first_fit); the guarded commit now lands in the carveout hole exactly as on hardware.
// The prior NULL deref (`ldrb [x8,#0x178]`, x8 = sg->xzsg_main_ref = 0 read from a demand-zeroed page)
// is GONE: with the metadata block landed at the hole base, the segment group's back-pointers resolve.
//
// NEW WALL (M2-carveout's honest boundary — libmalloc xzone SEGMENT-GROUP indexing, NOT placement):
// objc class realization (map_images -> realizeClassWithoutSwift -> _xzm_xzone_malloc_freelist_outlined
// -> _xzm_xzone_find_and_malloc_from_freelist_chunk -> xzm_segment_group_alloc_chunk+0x1c4) faults
// UNMAPPED (data abort EC=0x24 FSC=0x7) accessing sg+4 (the xzsg_lock) of a segment group
// `sg = &main->xzmz_segment_groups[sg_index]`. VERIFIED anatomy (three traced runs; addresses shift
// per run because libmalloc's carveout offset is entropy-derived): the main-zone metadata block's own
// `xzmz_total_size` field (read from committed memory) is 0x3e000 and the box committed EXACTLY that
// (mach_vm_map size 0x3e000, rlen 0x40000, fully backed) — yet xzm derives sg at main + ~0x4e4c8, i.e.
// ~0x104c8 PAST the block the guest itself sized. The offset VARIES run-to-run (0x4e4c8/0x4e740),
// the fingerprint of an INDEX-derived address, not a fixed struct offset. sg_index =
// segment_group_front_count * clusterid + sg_front_index, where clusterid/front come from
// _os_cpu_number()/_os_cpu_cluster_number() at alloc time while segment_group_count (which sizes
// total_size) is computed at zone-init from the commpage CPU topology. retrace stages a FROZEN copy of
// the HOST commpage (12 logical CPUs / 2 perflevel clusters), so the guest lays out per-CPU/cluster
// segment-group metadata for a 12-CPU host but executes on a single vCPU: the per-CPU segment-group
// index overshoots the block. This is an xzone per-CPU/cluster segment-group indexing subsystem —
// DISTINCT from carveout placement (now correct) and from demand-commit (M2-mmapcommit, which did its
// job) — deferred to a future milestone, NOT walked into here (a documented escape hatch exists:
// _COMM_PAGE_DEV_FIRM + MallocAllowInternalSecurity=1 + MallocSecureAllocator=0 in envp disables xzone
// entirely, see .superpowers/sdd/xzone-research.md §5; a principled single-vCPU commpage-topology model
// is the deeper fix). Determinism note: within a record/replay pair the reads are reproduced from the
// trace, so record and replay stay in lockstep; only the wall's exact fault address varies ACROSS
// record runs. A ~12-17x gettimeofday deadline-spin (a timed backoff with no second thread to make
// progress) precedes the fault. Ignored (not deleted) so it stays the living M2 gate, re-runnable with
// `--ignored` as the approach evolves.
#[ignore = "blocked at the libmalloc xzone SEGMENT-GROUP indexing wall (M2-carveout's honest boundary). The prior xzone NULL-METADATA wall FELL: it was a placement gap. M2-carveout t1 taught the box to punch mach_vm_deallocate holes in tracked PROT_NONE reservations (subtract_reservations) and to first-fit ANYWHERE-with-hint forward past reservations (first_fit), so libmalloc's guarded-metadata commit (mach_vm_map ANYWHERE, hint = reservation base) now lands in the 1 MiB carveout hole exactly as the real kernel forces it — the prior `ldrb [x8,#0x178]` x8=sg->xzsg_main_ref=0 NULL deref is GONE. The run advances into xzm_segment_group_alloc_chunk+0x1c4, which faults UNMAPPED (EC=0x24 FSC=0x7) at sg+4 (xzsg_lock) of sg=&main->xzmz_segment_groups[sg_index]. VERIFIED (3 runs): the main-zone block's own xzmz_total_size = 0x3e000 and the box committed exactly that (fully backed), but xzm indexes sg at main+~0x4e4c8 — ~0x104c8 PAST the block the guest itself sized. The offset VARIES per run (0x4e4c8/0x4e740) => an index-derived address: sg_index = segment_group_front_count*clusterid + sg_front_index (clusterid from _os_cpu_cluster_number() at alloc time) overshoots a segment_group_count sized at zone-init from the FROZEN HOST commpage (12 CPUs / 2 clusters) while executing on a single vCPU. An xzone per-CPU/cluster segment-group indexing subsystem, DISTINCT from carveout placement (now correct) — deferred. Escape hatch: MallocSecureAllocator=0 (+ _COMM_PAGE_DEV_FIRM, MallocAllowInternalSecurity=1) disables xzone; see .superpowers/sdd/xzone-research.md"]
#[test]
fn hello_dyn_records_and_replays() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    assert_eq!(rec.stdout, b"hi\n");
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, b"hi\n");
}
