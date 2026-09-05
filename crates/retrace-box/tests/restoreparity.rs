// M24-restoreaudit. `load`/`load_dynamic` run on the RECORD path only; replay builds its box through
// `Box_::restore`. Anything a load establishes that `restore` does not re-establish is a record/replay
// asymmetry whose signature is a PASSING RECORD followed by a REPLAY DIVERGENCE — invisible to every
// record-side test, which is how two milestones shipped one each (M21's believed-stack reservation,
// which broke replay outright, and M23's vector table, which was right only by luck).
//
// Not every difference is a defect: `mmap_next` and the guest's own reservations reset deliberately,
// because replay rebuilds those by re-executing recorded landmarks through mirrored dispatch arms.
// These tests pin the cases where no such mechanism exists.
use retrace_box::{Box_, TRAMPOLINE_IPA};
use retrace_guest::{parse_macho, slice_arm64e, HELLO, HELLO_DYN, DYLD_PATH};
use retrace_trace::Event;

/// G1. The STATIC load never sets `TPIDRRO_EL0` (a fresh vCPU leaves it 0), but `restore` set it to
/// `TSD_IPA` unconditionally under a comment claiming to "match load" — which matched only
/// `load_dynamic`. A static guest therefore ran with a different thread pointer on each side, and
/// `TSD_IPA` is not even mapped in a static box, so a deref would fault on REPLAY ONLY. Nothing
/// exercises it today; that is exactly why it needs pinning rather than a comment.
#[test]
fn a_restored_static_box_has_the_same_thread_pointer_as_the_static_load() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let b = Box_::load(&loaded);
    let recorded_tp = b.tpidrro_el0();
    let Event::Snapshot { regs, mem } = b.snapshot() else { panic!("snapshot") };
    drop(b);
    let r = Box_::restore(&mem, &regs);
    assert_eq!(r.tpidrro_el0(), recorded_tp,
        "record and replay of a STATIC guest must agree on TPIDRRO_EL0; record had {recorded_tp:#x}, \
         restore produced {:#x}", r.tpidrro_el0());
}

/// L2. `load_dynamic` folds the real startup register state into thread 0's saved context; `restore`
/// left it `ThreadCtx::zeroed()`. Benign only because every consumer today either refreshes the
/// current thread's entry first or reads the table for NON-current threads — so a future reader of
/// `ctx_of(current)` without a prior `save_ctx` would get real state on record and zeros on replay.
#[test]
fn a_restored_dynamic_box_has_thread_zeros_context_populated() {
    let exe = parse_macho(&std::fs::read(HELLO_DYN).unwrap());
    let dyld = parse_macho(slice_arm64e(&std::fs::read(DYLD_PATH).unwrap()));
    let b = Box_::load_dynamic(&exe, &dyld, &["hello_dyn".to_string()]);
    assert_ne!(b.threads().ctx_of(0).regs.pc, 0, "precondition: load_dynamic populates thread 0");
    let Event::Snapshot { regs, mem } = b.snapshot() else { panic!("snapshot") };
    drop(b);
    let r = Box_::restore(&mem, &regs);
    assert_eq!(r.threads().ctx_of(0).regs.pc, r.save_ctx().regs.pc,
        "restore must seed thread 0's saved context from the live vCPU, as load_dynamic does; \
         a zeroed entry differs from record for any reader that does not refresh it first");
}

/// L1 (M23 review finding F5). `build_vector_table()` runs in both load paths and never in `restore`,
/// so the trapping vector padding reaches replay ONLY because the trampoline happens to be a
/// snapshot backing. Correct today, pinned by nothing.
///
/// This also closes M23's F4 as a SILENT failure. `TRACE_MAGIC` was not bumped when t1 changed that
/// padding from `UDF #0` to `hvc #1`, so a pre-M23 recording still opens and restores its OLD zero
/// padding while the current code assumes trapping padding — a fall-through on that replay destroys
/// ESR_EL1 and reproduces the very `pc=0x4204` misattribution M23 removed. With this assert the
/// stale trace is REFUSED LOUDLY instead of replayed wrongly, which is the difference between a
/// format break nobody declared and a diagnosable error.
#[test]
#[should_panic(expected = "vector table")]
fn restore_refuses_a_snapshot_whose_vector_table_is_not_the_one_this_build_makes() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let b = Box_::load(&loaded);
    let Event::Snapshot { regs, mut mem } = b.snapshot() else { panic!("snapshot") };
    drop(b);
    // Exactly what a pre-M23 recording carries: zero padding (`UDF #0`) behind each slot head.
    let t = mem.iter_mut().find(|r| r.ipa == TRAMPOLINE_IPA).expect("trampoline must be snapshotted");
    for slot in 0..16usize {
        for w in 1..0x20usize { t.bytes[slot * 0x80 + w * 4..slot * 0x80 + w * 4 + 4].fill(0); }
    }
    let _ = Box_::restore(&mem, &regs);
}

/// Normalise the one field that differs BY DESIGN at landmark 0, so the diff below is about
/// everything else. `load_dynamic` installs the shared-cache pager eagerly; `restore` starts without
/// it and the mirrored `#294`/`#536` dispatch arms install it on replay. That mirror is real
/// (retrace-core: record 350/369 ↔ replay 2257/2265) and it is the reason this one is allowed to
/// differ — safe because dyld provably touches no cache VA before its own mapping call.
fn normalise(s: &str) -> String { s.replace("cache_installed=true", "cache_installed=false") }

/// The entries that differ between two SORTED `(ipa, len)` backing lists, each tagged with the side
/// it appears on and rendered in hex — empty when the two maps are identical.
///
/// Worth the fifteen lines because the alternative was measured, not guessed: a plain `assert_eq!`
/// on the dynamic fixture prints both 136-entry lists in decimal, and the reader then has to find
/// the one tuple that moved. A parity guard whose failure output cannot be read is a guard that gets
/// dismissed as noise the first time it fires for a real reason.
///
/// A two-pointer merge over sorted input rather than a set difference, so it is multiplicity-exact:
/// `[a, a, b]` against `[a, b, b]` has an empty set difference and is still not the same map.
fn diff_backings(load: &[(u64, usize)], restore: &[(u64, usize)]) -> String {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    let show = |side, e: &(u64, usize)| format!("{side}-only (ipa {:#x}, len {:#x})", e.0, e.1);
    while i < load.len() || j < restore.len() {
        match (load.get(i), restore.get(j)) {
            (Some(l), Some(r)) if l == r => { i += 1; j += 1; }
            (Some(l), Some(r)) if l < r => { out.push(show("load", l)); i += 1; }
            (Some(_), Some(r)) => { out.push(show("restore", r)); j += 1; }
            (Some(l), None) => { out.push(show("load", l)); i += 1; }
            (None, Some(r)) => { out.push(show("restore", r)); j += 1; }
            (None, None) => unreachable!("loop condition guarantees one side still has entries"),
        }
    }
    out.join(", ")
}

/// The structural guard. Fixing four asymmetries does not keep the class closed — nothing stops the
/// NEXT field or load-time write from reopening it, and the failure signature (passing record,
/// diverging replay) is invisible to every record-side test. So this diffs a LOAD box against a
/// RESTORE box built from that same box's own snapshot, and a new asymmetry has to get past a test
/// rather than past a reviewer.
///
/// **OBLIGATION when you add a field to `Box_`, or any load-time write or sysreg set:** it must be
/// either (a) covered here and EQUAL, or (b) named in `normalise` above with the mirrored mechanism
/// that re-establishes it on replay, cited by file and line. There is no third option that is safe.
/// M21 shipped record-only and M23 shipped correct-by-luck precisely because no such test existed.
///
/// ---
///
/// **What this guard deliberately does NOT compare, and why — read this before adding to it.**
///
/// `Box_` carries 27 state fields. Eleven of them — `noaccess`, `bps_armed`, `wps_armed`,
/// `watch_ranges`, `syscall_watch_hit`, `tlbi_stub_ready`, `fds`, `sigtable`, `thread_start_pc`,
/// `wq_thread_pc`, `pthread_size` — sit at their DEFAULT on both sides at landmark 0, because both
/// constructors independently choose the same default. Asserting `Default == Default` passes for a
/// reason unrelated to the assertion's name: it would grow this file's apparent authority without
/// growing its reach by a single field, and the reader who trusts that appearance is exactly the
/// one this milestone exists to protect.
///
/// They are omitted BY DECISION, not by oversight. The moment a load path starts setting one of them
/// before the first landmark it stops being a default and starts being an asymmetry — and the
/// OBLIGATION above already requires it to be added here at that point. Watch for that trigger; the
/// current absence is not one.
///
/// `l2_host` is absent too, for a different reason: it is a HOST pointer, so two boxes differ there
/// by construction and no useful equality exists. What it points AT is pinned instead, through
/// `dbg_backings` (the L2 table's `(ipa, len)`) and `dbg_next_l3` (the cursor derived from those
/// same tables).
///
/// And the largest gap, stated so it is not mistaken for coverage: `from_checkpoint` has NO parity
/// guard at all. It is the path this class has bitten five times (M9 t3, M10, M11, M14, M18 — the
/// `BoxState` field comments enumerate them by name), it carries far more state than `restore`, and
/// it runs mid-run where nothing is at a default. That guard needs a mid-run fixture and a judgement
/// about what *should* legitimately differ at a mid-run landmark; it is the successor milestone, not
/// something this file quietly covers.
fn assert_load_restore_parity(b: Box_, label: &str) {
    let load_state = normalise(&b.dbg_internal_state());
    let (top, size) = (b.stack_top(), b.stack_size());
    let (tp, tpro) = (b.tpidr_el0(), b.tpidrro_el0());
    // M24 t2: the WHOLE context, not just `regs.pc`. `ThreadCtx` also carries the FP bank, FPCR/FPSR,
    // `tpidrro_el0`, ELR_EL1 and SPSR_EL1 — every one of which a restored box establishes by a
    // different route than a load does, and `regs.pc` alone would agree while any of them differed.
    let ctx0 = b.threads().ctx_of(0).clone();
    let nthreads = b.threads().len();
    let fts = b.fall_throughs();
    let vectors = b.read_guest(TRAMPOLINE_IPA, 0x800);
    // M24 t2: `load` derives the region list from the Mach-O, `restore` from the snapshot's regions.
    // Two derivations of one map, and until now nothing compared them. Sorted, because equality of
    // the MAP is the contract — no lookup here is order-sensitive (they go through `find`/`any` on a
    // non-overlapping set), so pinning the emission order would pin something the code does not rely
    // on and would fail on a harmless reordering.
    let mut load_backings = b.dbg_backings();
    load_backings.sort_unstable();
    let next_l3 = b.dbg_next_l3();

    let Event::Snapshot { regs, mem } = b.snapshot() else { panic!("snapshot") };
    drop(b); // one VM per process (HVF)
    let r = Box_::restore(&mem, &regs);

    assert_eq!(normalise(&r.dbg_internal_state()), load_state, "{label}: internal bookkeeping");
    assert_eq!((r.stack_top(), r.stack_size()), (top, size), "{label}: stack geometry");
    assert_eq!((r.tpidr_el0(), r.tpidrro_el0()), (tp, tpro), "{label}: thread-pointer sysregs");
    assert_eq!(*r.threads().ctx_of(0), ctx0, "{label}: thread 0 saved context");
    assert_eq!(r.threads().len(), nthreads, "{label}: thread count");
    assert_eq!(r.fall_throughs(), fts, "{label}: fall-through counter");
    assert_eq!(r.read_guest(TRAMPOLINE_IPA, 0x800), vectors, "{label}: EL1 vector table");
    // Count first, then the map. The count is implied by the map comparison, but it states a dropped
    // or duplicated backing as one number beside another, which is the crispest form that failure has.
    let mut restore_backings = r.dbg_backings();
    restore_backings.sort_unstable();
    assert_eq!(restore_backings.len(), load_backings.len(), "{label}: backing count");
    let diff = diff_backings(&load_backings, &restore_backings);
    assert!(diff.is_empty(),
        "{label}: load and restore disagree on the guest memory map: {diff}");
    // Spelled out in hex as well as `assert_eq!`'s decimal: every IPA in this codebase is written in
    // hex, and a one-granule drift reads as 0x4000 at a glance and as 16384 only after arithmetic.
    assert_eq!(r.dbg_next_l3(), next_l3,
        "{label}: next free L3 table IPA — restore derived {:#x} from the snapshot's L3-window \
         backings, load had bumped it to {next_l3:#x}", r.dbg_next_l3());
}

#[test]
fn a_restored_static_box_matches_the_load_it_came_from() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    assert_load_restore_parity(Box_::load(&loaded), "static");
}

#[test]
fn a_restored_dynamic_box_matches_the_load_it_came_from() {
    let exe = parse_macho(&std::fs::read(HELLO_DYN).unwrap());
    let dyld = parse_macho(slice_arm64e(&std::fs::read(DYLD_PATH).unwrap()));
    assert_load_restore_parity(
        Box_::load_dynamic(&exe, &dyld, &["hello_dyn".to_string()]), "dynamic");
}
