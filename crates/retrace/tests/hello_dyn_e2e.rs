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
// The xzone SEGMENT-GROUP indexing wall (M2-carveout's honest boundary) FELL in M2-cpuid, and it was
// a ONE-VALUE bug — NOT the "per-CPU/cluster commpage-topology subsystem" the M2-carveout walk guessed.
// libmalloc's xzone computes sg_index = segment_group_front_count*clusterid + sg_front_index, where
// clusterid = _os_cpu_cluster_number() = (uint32_t)TPIDR_EL0 >> 12 on macOS 26 arm64e (verified by
// live lldb disassembly of _xzm_xzone_find_and_malloc_from_freelist_chunk; _os_cpu_number() reads
// TPIDR_EL0 & 0xFFF). retrace had set the guest TPIDR_EL0 = TSD_IPA = 0x30000, conflating it with the
// thread-self pointer — but TPIDRRO_EL0 (unchanged) is the real TSD base; TPIDR_EL0 carries the
// cpu/cluster id. So cpu = 0x30000 & 0xFFF = 0 (accidentally right) but cluster = 0x30000 >> 12 = 48
// (garbage — there is no cluster 48), and sg_index overshot ~253 slots (main + ~0x4e4c8), past the
// 0x3e000 main-zone block, faulting UNMAPPED on the segment-group lock. M2-cpuid sets guest
// TPIDR_EL0 = 0 (single vCPU is always cpu 0 / cluster 0) at both constructor sites (load_dynamic /
// restore), below the trace and identical on record and replay (a fixed constant, like the PAC keys
// and synthetic timebase — nothing enters the trace). The xzone segment-group fault is GONE; the run
// advances from ~205 to ~218 traps, past _xzm_segment_group_alloc_chunk, with no earlier fault
// (confirming nothing dereferences TPIDR_EL0 as a TSD base). The "per-run variance" the M2-carveout
// walk saw (0x4e4c8 vs 0x4e740, delta = one sizeof(xzm_segment_group_s)) was forwarded-entropy drift
// in the pre-fault gettimeofday spin, not the index — the register-derived overshoot itself was fixed.
// Known debt (deferred, not fatal, not a determinism bug): retrace still memcpy's the whole HOST
// commpage into the guest, so the per-CPU/cluster COUNT arrays carry the host's 12-CPU/2-cluster
// values; harmless once the INDEX is pinned to 0 by this fix (the bytes are frozen identically on both
// runs). A principled single-vCPU commpage synthesis is the hygiene follow-up.
//
// NEW WALL (M2-cpuid's honest boundary — an unhandled Mach task-port MIG message, mach-IPC lineage,
// DISTINCT from the now-fixed CPU-identity bug): at ~218 traps the run hits
// `RECORD ERROR: unsupported mach_msg2 at pc 0x1804abc34: msgh_id 3409 dest 0x203 (guest task port
// Some(515)) send_size 36` — a mach_msg2 (trap -47) to the GUEST TASK PORT. msgh_id 3409 is the Mach
// `task` subsystem (MIG base 3400), routine index 9 = task_get_special_port (task.defs slot 9, macOS
// 26.4 SDK): the 36-byte request is header(24) + NDR(8) + which_port:int(4), and which_port = 4 =
// TASK_BOOTSTRAP_PORT — libSystem fetching its bootstrap port. It is Unsupported because retrace's MIG
// router (retrace-core::machmsg::route) has NO handler for this task-subsystem id: it services 4811
// (_kernelrpc_mach_vm_map), stubs 4822 (vm_reclaim) / 8000-8001 (task_restartable), forwards the
// read-only allowlist {200,206,3418}, and fails LOUD on every other id to the task port. Servicing one
// more MIG id is M2-mach-lineage work, NOT in scope for M2-cpuid (a CPU-identity milestone) beyond
// re-parking the gate here. Ignored (not deleted) so it stays the living M2 gate, re-runnable with
// `--ignored` as the approach evolves. (Trap count varies run-to-run because getentropy/PID are
// forwarded and recorded per-trace; that is normal record/replay, not a determinism defect.)
//
// The Mach task-port MIG wall (M2-cpuid's honest boundary) FELL in M2-bootstrap: retrace's MIG router
// (machmsg::route) now services task_get_special_port(TASK_BOOTSTRAP_PORT) (msgh_id 3409) by
// synthesizing a COMPLEX MIG reply carrying a FIXED synthetic bootstrap-port name
// (SYNTHETIC_BOOTSTRAP_PORT = 0x0BAD_0B03) — a pure function of (reply_port, name), mirrored textually
// in record and replay (the divergence oracle byte-compares the recomputed reply). libxpc's image
// initializer ACCEPTS the reply: __MIG_check__Reply__task_get_special_port passes, libxpc extracts the
// synthetic name and flows it into _xpc_mach_port_retain_send (three deterministically-forwarded
// _kernelrpc_mach_port_mod_refs_trap send-right retains) and then xpc_pipe_create_from_port. The run
// advances from ~218 to ~228 traps.
//
// NEW WALL (M2-bootstrap's honest boundary — the XPC bootstrap-PIPE subsystem, DISTINCT from the
// now-serviced task_get_special_port MIG): the design spec's "fetch-and-cache, dormant" hypothesis is
// empirically FALSIFIED for this hello_dyn — libxpc's initializer is NOT lazy. At ~228 traps the run
// aborts in `libxpc.dylib`_xpc_create_bootstrap_pipe.cold.1` with `brk #0x1` (EC=0x3c) at guest
// pc 0x180201190, crash string "Bug in libxpc: Could not create pipe to bootstrap server!", called from
// _libxpc_initializer+0x42c <- libSystem_initializer+0x100 (all symbolicated live against the arm64e
// shared cache, runtime slide backed out). Hot path: after the send-right retain,
// `xpc_pipe_create_from_port(bootstrap_port = 0x0BAD_0B03, flags = 4)` returns NULL — a real Mach
// dispatch channel to launchd cannot be stood up over the synthetic token — so `cbz x0` takes the cold
// __builtin_trap. This is NOT a Task-1 reply-format bug: the BRK is DOWNSTREAM of __MIG_check__Reply and
// 0x0BAD_0B03 flows through cleanly (retain + pipe-create), proving the complex reply decoded correctly.
// It is also NOT the predicted eager bootstrap_look_up: NO mach_msg2 ever targets 0x0BAD_0B03
// (grep-confirmed) — the abort precedes any send, so the blast radius is one message, not an unbounded
// look-up chain. Risk-check 2 (name collision): 0x0BAD_0B03 is collision-free — it appears ONLY as the
// `name` argument of the three bootstrap-caching mach_port_mod_refs traps, never as a differently-sourced
// forwarded port name. Servicing this wall means standing up the XPC pipe / dispatch-mach channel
// subsystem against a real bootstrap port — explicitly DEFERRED (do NOT pre-stub launchd/XPC). Ignored
// (not deleted) so it stays the living M2 gate, re-runnable with `--ignored`.
//
// The XPC bootstrap-PIPE wall FELL in M2-xpcport, and M2-bootstrap's guess just above (that clearing it
// meant standing up a real XPC pipe / dispatch-mach subsystem) was WRONG: the pipe never needed a live
// channel to launchd — only a genuinely valid send right. task_get_special_port(BOOTSTRAP) now hands
// back a REAL kernel-minted send right (Box_::mint_bootstrap_port via mach_port_construct with
// MPO_INSERT_SEND_RIGHT in retrace's OWN IPC space, which is the guest's; name observed = 0x1003)
// instead of the synthetic 0x0BAD_0B03. Because the minted name is nondeterministic (the kernel picks
// it, like task_self's name), the ServiceGetSpecialPort handler moved from M2-bootstrap's
// synthesize-and-byte-compare posture to the forward-and-record posture: record mints + records the
// reply bytes; replay applies the recorded reply VERBATIM (no recompute/byte-compare — the name can't
// be regenerated), exactly as task_self's port name is already replayed. libxpc's three
// __xpc_mach_port_retain_send sites (mach_port_mod_refs(SEND,+1), trap -19, name 0x1003) now return
// KERN_SUCCESS instead of KERN_INVALID_NAME, so xpc_pipe_create_from_port returns non-NULL, the
// `brk #0x1` in `libxpc.dylib`_xpc_create_bootstrap_pipe.cold.1` (pc 0x180201190) is GONE, and
// _libxpc_initializer completes. The run advances ~228 -> ~242 traps.
//
// NEW WALL (M2-xpcport's honest boundary — a SECOND task-subsystem MIG id, DISTINCT from the now-fixed
// pipe and NOT the deferred XPC send): at ~242 traps the run fail-louds on
// `RECORD ERROR: unsupported mach_msg2 at pc 0x1804abc34: msgh_id 3410 dest 0x203 (guest task port
// Some(515)) send_size 52`. This is NOT a CPU fault (no ESR/EC, unlike the prior brk) — it is retrace's
// MIG router (machmsg::route) fail-louding on an unhandled id. msgh_id 3410 is the Mach `task`
// subsystem (base 3400) routine 10 = task_set_special_port (mach/task.h, macOS 26 SDK): a COMPLEX
// message (msgh_bits 0x80001513) carrying one COPY_SEND mach_msg_port_descriptor (port name 0x1103)
// followed by which_port = 10 = TASK_DEBUG_CONTROL_PORT (mach/task_special_ports.h), with reply port
// 0x1603 (MAKE_SEND_ONCE — it awaits a reply). Symbolicated LIVE against the arm64e shared cache (box
// loads at slide 0, so trace pcs are unslid VAs; ASLR slide backed out via lldb): the caller is NOT
// libxpc — the 0x1802xxxxx range is shared with libsystem_trace, and the real stack is
// `libsystem_trace.dylib`_os_trace_create_debug_control_port+0x60` <- `_libtrace_init+0xfc` <-
// `libSystem.B.dylib`libSystem_initializer+0x10c` <- dyld's findAndRunAllInitializers. So this is the
// os_log/os_trace image initializer installing its task debug-control port — a SIBLING libSystem
// sub-initializer that runs just AFTER _libxpc_initializer (libSystem_initializer+0x100 called libxpc;
// +0x10c calls libtrace), which is exactly why widening past the pipe brk surfaced it. It is a SMALL
// next-init MIG step (same task-subsystem lineage as the SERVICED 3409 task_get_special_port and the
// stubbed vm_reclaim / task_restartable): servicing it means accepting the complex request, handling
// the inbound debug-control-port descriptor, and synthesizing a __Reply__task_set_special_port_t that
// returns KERN_SUCCESS, mirrored textually in record and replay. It is NOT the deferred XPC send /
// dispatch-mach subsystem — no mach_msg2 targets the minted bootstrap port (0x1003), and no
// bootstrap_look_up has appeared. Deferred to the next milestone; do NOT pre-stub. Ignored (not
// deleted) so it stays the living M2 gate, re-runnable with `--ignored`.
//
// The libsystem_trace task_set_special_port wall FELL in M2-setport: retrace's MIG router
// (machmsg::route) now services task_set_special_port(TASK_DEBUG_CONTROL_PORT) (msgh_id 3410) with a
// deterministic mig_reply_error KERN_SUCCESS reply (reply id 3510). The complex request's inbound
// COPY_SEND port descriptor (name 0x1103) is decoded but deliberately DROPPED — never forwarded, since
// forwarding would set retrace's OWN debug-control port. This is the STANDARD symmetric posture (unlike
// M2-xpcport's forward-and-record special case for the nondeterministic minted bootstrap name): the reply
// is a pure function of (msgh_id, reply_port, KERN_SUCCESS), so replay recomputes it and byte-compares
// against the recording (that comparison IS the divergence oracle). libsystem_trace's
// _os_trace_create_debug_control_port ACCEPTS the reply and _libtrace_init completes; the run advances one
// MIG call further (~241-242 traps; the count is within forwarded-entropy noise, not a determinism defect).
//
// NEW WALL (M2-setport's honest boundary — a THIRD task-subsystem MIG id, DISTINCT from the serviced
// 3409/3410, and NOT the deferred XPC send): at ~241 traps the run fail-louds
// `RECORD ERROR: unsupported mach_msg2 at pc 0x1804abc34: msgh_id 3405 dest 0x203 (guest task port
// Some(515)) send_size 40` — no ESR/EC (retrace's MIG router rejecting an unhandled id, not a CPU fault).
// msgh_id 3405 is the Mach `task` subsystem (base 3400) routine 5 = task_info (mach/task.h): a SIMPLE
// message (bits 0x1513, non-complex), 40 bytes = header(24) + NDR(8) + flavor:int(4) + task_info_outCnt:int(4),
// with flavor = 15 = TASK_AUDIT_TOKEN and count = 8 = TASK_AUDIT_TOKEN_COUNT (audit_token_t = 8 words),
// reply port 0x1603 (MAKE_SEND_ONCE — it awaits a reply). Symbolicated live against the arm64e shared
// cache (box loads at slide 0, so trace pcs are unslid VAs; resolved statically in lldb): the caller is NOT
// libsystem_trace (that was the fallen 3410) but libsystem_secinit's app-sandbox check —
// `libsystem_kernel.dylib`task_info+224` <- `libxpc.dylib`_fetch_self_token+60` <- (dispatch_once)
// `libxpc.dylib`_xpc_get_self_audit_token+144` <- `libxpc.dylib`xpc_copy_entitlements_for_self+20` <-
// `libsystem_secinit.dylib`_libsecinit_appsandbox_check+72` <- `libsystem_secinit.dylib`_libsecinit_initializer+160`
// <- `libSystem.B.dylib`libSystem_initializer+0x118` <- dyld's findAndRunAllInitializers. So this is the
// SANDBOX-INIT image initializer fetching the process's OWN audit token (process identity) — the sibling
// libSystem sub-initializer that runs right AFTER libtrace (libSystem_initializer+0x10c ran libtrace / 3410;
// +0x118 runs libsecinit), which is exactly why widening past the 3410 wall surfaced it. It is a SMALL
// next-init MIG step (same task-subsystem lineage as the serviced 3409 task_get_special_port and 3410
// task_set_special_port): service it by synthesizing a __Reply__task_info_t carrying an audit_token_t (8
// words). Because the audit token holds host process identity (pid/asid/pidversion vary run-to-run), the
// reply is NONDETERMINISTIC — so this likely wants the forward-and-record posture (record forwards the real
// task_info + records the reply; replay applies it verbatim), like task_self's port name and getentropy,
// NOT synthesize-and-byte-compare. NOTE the caller is libsecinit's SANDBOX check (via
// xpc_copy_entitlements_for_self), so servicing task_info may surface a FURTHER libsecinit step (an
// entitlement / sandbox query) once the audit token flows — to be discovered, not pre-stubbed. It is NOT
// the deferred XPC send / dispatch-mach subsystem — dest is the guest task port (0x203), no mach_msg2
// targets the minted bootstrap port (0x1003), and no bootstrap_look_up has appeared. Deferred to the next
// milestone; do NOT pre-stub. Ignored (not deleted) so it stays the living M2 gate, re-runnable with
// `--ignored`.
#[ignore = "blocked at the libsystem_secinit task_info(TASK_AUDIT_TOKEN) wall (M2-setport's honest boundary). The libsystem_trace task_set_special_port(TASK_DEBUG_CONTROL_PORT) wall FELL in M2-setport: machmsg::route now services msgh_id 3410 with a deterministic mig_reply_error KERN_SUCCESS reply (id 3510); the complex request's inbound COPY_SEND port descriptor (name 0x1103) is decoded but DROPPED (never forwarded — forwarding would set retrace's own debug-control port). STANDARD symmetric posture (unlike M2-xpcport's forward-and-record for the nondeterministic minted bootstrap name): the reply is a pure function of (msgh_id, reply_port, KERN_SUCCESS), so replay recomputes + byte-compares (that comparison IS the divergence oracle). _os_trace_create_debug_control_port accepts it, _libtrace_init completes, and the run advances one MIG call further (~241-242 traps, count within forwarded-entropy noise). NEW wall (a THIRD task-subsystem MIG, NOT the deferred XPC send): at ~241 traps machmsg::route fail-louds `unsupported mach_msg2 at pc 0x1804abc34: msgh_id 3405 dest 0x203 (guest task port Some(515)) send_size 40` (no ESR/EC — retrace's router rejecting an unhandled id, not a CPU fault). msgh_id 3405 = task_info (Mach task subsystem base 3400, routine 5; mach/task.h): a SIMPLE message (bits 0x1513), 40 bytes = header(24)+NDR(8)+flavor:int(4)+count:int(4), flavor = 15 = TASK_AUDIT_TOKEN, count = 8 = TASK_AUDIT_TOKEN_COUNT (audit_token_t = 8 words), reply port 0x1603 (MAKE_SEND_ONCE). Symbolicated live against the arm64e shared cache (slide backed out; box loads at slide 0): the caller is libsystem_secinit's app-sandbox check, NOT libsystem_trace — `libsystem_kernel.dylib`task_info+224` <- `libxpc.dylib`_fetch_self_token+60` <- (dispatch_once) `libxpc.dylib`_xpc_get_self_audit_token+144` <- `libxpc.dylib`xpc_copy_entitlements_for_self+20` <- `libsystem_secinit.dylib`_libsecinit_appsandbox_check+72` <- `_libsecinit_initializer+160` <- `libSystem.B.dylib`libSystem_initializer+0x118` <- dyld's findAndRunAllInitializers — the sandbox-init image initializer fetching the process's OWN audit token, the sibling libSystem sub-initializer right after libtrace (libSystem_initializer+0x10c ran libtrace/3410; +0x118 runs libsecinit), which is why widening past the 3410 wall surfaced it. SMALL next-init MIG step (same lineage as the serviced 3409/3410): synthesize a __Reply__task_info_t carrying an audit_token_t (8 words). The audit token holds host process identity (pid/asid/pidversion), so the reply is nondeterministic — likely the forward-and-record posture (record forwards + records; replay applies verbatim), like task_self's name / getentropy, NOT synthesize-and-byte-compare. Its caller is libsecinit's sandbox check (via xpc_copy_entitlements_for_self), so servicing task_info may surface a FURTHER libsecinit step (an entitlement/sandbox query) once the token flows. NOT the deferred XPC send / dispatch-mach subsystem (dest is the guest task port 0x203; no mach_msg2 targets the minted 0x1003; no bootstrap_look_up yet). Deferred to the next milestone; do NOT pre-stub. (Predecessor walls, all fallen: the libsystem_trace task_set_special_port MIG (this milestone), the XPC bootstrap-pipe brk + real bootstrap send right (M2-xpcport), the task_get_special_port MIG (M2-bootstrap), the xzone segment-group index (M2-cpuid, TPIDR_EL0 = 0), the reservation carveout/demand-commit (M2-carveout/M2-mmapcommit), and arm64e data-pointer PAC TBI (M2-tbi). Deferred debt unchanged: the frozen HOST commpage still carries 12-CPU/2-cluster COUNTS — harmless once the cpu/cluster index is pinned to 0; a single-vCPU commpage synthesis is the hygiene follow-up.)"]
#[test]
fn hello_dyn_records_and_replays() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    assert_eq!(rec.stdout, b"hi\n");
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, b"hi\n");
}
