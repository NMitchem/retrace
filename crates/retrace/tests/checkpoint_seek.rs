// The M4 correctness invariant, directly generalizing M3's oracle: a checkpoint-restored-and-
// continued session must produce byte-identical results to a cold seek() to the same (N,K). Plus
// CheckpointCache's byte-budget/LRU eviction, proven via the observable behavior it produces
// (never via wall-clock — this project bans timing-based assertions by policy).
mod util;
use std::path::Path;

#[test]
fn checkpointed_seek_same_and_earlier_window_hits_match_cold() {
    let (rec, trace) = util::record(retrace_guest::SPINLOOP);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let trace = Path::new(&trace);
    let mut cache = retrace_core::CheckpointCache::new(256 * 1024 * 1024, 64);

    // SAME-WINDOW hit: checkpoint deep in window 2 (landmark 2, ~4003 insns), then seek a few
    // steps further — must resume from the cache, not restart from landmark 0.
    let _ = retrace_core::checkpointed_seek(trace, &mut cache, 2, 3990).unwrap();
    assert_eq!(cache.len(), 1, "a >=64-step seek must clear the cost gate and get cached");
    let before = cache.total_single_steps();
    let (regs_a, fp_a, mem_a) = {
        let mut s = retrace_core::checkpointed_seek(trace, &mut cache, 2, 3995).unwrap();
        (s.dbg_regs(), s.dbg_fp_regs(), { let (_, mem) = s.snapshot(); mem })
    };
    let same_window_cost = cache.total_single_steps() - before;
    assert!(same_window_cost <= 10, "same-window hit should need ~5 steps, paid {same_window_cost}");
    let cold_a = retrace_core::seek(trace, 2, 3995).unwrap();
    assert_eq!(cold_a.dbg_regs(), regs_a, "registers diverged: checkpointed vs cold");
    assert_eq!(cold_a.dbg_fp_regs(), fp_a, "FP/SIMD diverged: checkpointed vs cold");
    assert!(cold_a.diff_memory(&mem_a).is_none(), "memory diverged: checkpointed vs cold");
    drop(cold_a); // one VM per process: release the cold session before opening the earlier-window ones

    // EARLIER-WINDOW hit: checkpoint deep in window 1 (landmark 1, ~606 insns; clears the cost
    // gate), then seek into window 2 — must resume via advance_to_landmark(2) + step_insns, not miss.
    let mut cache2 = retrace_core::CheckpointCache::new(256 * 1024 * 1024, 64);
    let _ = retrace_core::checkpointed_seek(trace, &mut cache2, 1, 590).unwrap();
    assert_eq!(cache2.len(), 1);
    let (regs_b, fp_b, mem_b) = {
        let mut s = retrace_core::checkpointed_seek(trace, &mut cache2, 2, 50).unwrap();
        (s.dbg_regs(), s.dbg_fp_regs(), { let (_, mem) = s.snapshot(); mem })
    };
    let cold_b = retrace_core::seek(trace, 2, 50).unwrap();
    assert_eq!(cold_b.dbg_regs(), regs_b, "registers diverged (earlier-window hit): checkpointed vs cold");
    assert_eq!(cold_b.dbg_fp_regs(), fp_b, "FP/SIMD diverged (earlier-window hit): checkpointed vs cold");
    assert!(cold_b.diff_memory(&mem_b).is_none(), "memory diverged (earlier-window hit): checkpointed vs cold");
}

#[test]
fn checkpoint_cache_respects_byte_budget_and_evicts_lru() {
    let (rec, trace) = util::record(retrace_guest::SPINLOOP);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let trace = Path::new(&trace);

    // Measure one checkpoint's real footprint first (cost_gate_steps=1: always cached).
    let mut probe = retrace_core::CheckpointCache::new(usize::MAX, 1);
    let _ = retrace_core::checkpointed_seek(trace, &mut probe, 1, 50).unwrap();
    let one_checkpoint_bytes = probe.used_bytes();
    assert!(one_checkpoint_bytes > 0, "a cached checkpoint must have nonzero measured size");

    // A budget that comfortably fits ONE checkpoint but not three: repeated inserts must evict,
    // never exceed budget, and keep only the most recently used entries.
    let budget = one_checkpoint_bytes + one_checkpoint_bytes / 2;
    let mut cache = retrace_core::CheckpointCache::new(budget, 1);
    for k in [50u64, 150, 250, 350, 450] {
        let _ = retrace_core::checkpointed_seek(trace, &mut cache, 1, k).unwrap();
        assert!(cache.used_bytes() <= budget,
            "cache exceeded its byte budget after seeking to (1,{k}): {} > {budget}", cache.used_bytes());
    }
    assert!(cache.len() < 5,
        "5 inserts into a ~1.5-checkpoint budget must have evicted at least one entry, got {} entries", cache.len());

    // The MOST RECENT position (1, 450) must still be resident (LRU keeps the freshest).
    let before = cache.total_single_steps();
    let _ = retrace_core::checkpointed_seek(trace, &mut cache, 1, 455).unwrap();
    let paid = cache.total_single_steps() - before;
    assert!(paid <= 10, "the most recent checkpoint should still be resident: expected ~5 steps, paid {paid}");
}

/// Probe increasing landmarks for one whose window is at least `min` instructions long (one
/// session per probe, sequential — never two alive). If NO candidate clears `min`, widen this list
/// rather than lowering `min` below the cache's cost gate.
fn first_window_with_len(trace: &Path, min: u64) -> (usize, u64) {
    for n in [3usize, 5, 8, 12, 20, 30, 50, 80, 100, 130, 160, 200, 250, 300] {
        let mut s = retrace_core::seek(trace, n, 0).unwrap();
        let l = s.window_len_here().unwrap();
        drop(s);
        if l >= min { return (n, l); }
    }
    panic!("no window of >= {min} insns among the probes — widen the candidate landmark list");
}

#[test]
fn checkpointed_seek_matches_cold_across_a_neon_window() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let trace = Path::new(&trace);
    // dyld's early init uses NEON (memcpy, hashing) well before any application code runs; a
    // checkpoint taken partway through such a window and resumed must carry the LIVE V-register
    // state, not the zeroed defaults Box_::restore silently assumes at landmark 0.
    let (n, len) = first_window_with_len(trace, 100);
    let k0 = len / 2;
    let mut cache = retrace_core::CheckpointCache::new(256 * 1024 * 1024, 64);
    let _ = retrace_core::checkpointed_seek(trace, &mut cache, n, k0).unwrap();
    assert!(!cache.is_empty(), "a >=50-step seek into a >=100-insn window must clear the cost gate");
    let k1 = k0 + 10;
    let (regs, fp, mem) = {
        let mut s = retrace_core::checkpointed_seek(trace, &mut cache, n, k1).unwrap();
        (s.dbg_regs(), s.dbg_fp_regs(), { let (_, mem) = s.snapshot(); mem })
    };
    // Non-vacuousness: at least one of the 32 V registers must be nonzero here, or this test would
    // silently pass even if FP/SIMD capture were completely broken (e.g. always restoring zeros).
    assert!(fp.matches("=0x00000000000000000000000000000000").count() < 32,
        "all 32 V registers are zero at the checkpoint — the NEON-crossing proof has gone vacuous; widen the probe candidates");
    let cold = retrace_core::seek(trace, n, k1).unwrap();
    assert_eq!(cold.dbg_regs(), regs, "registers diverged across a NEON-crossing window");
    assert_eq!(cold.dbg_fp_regs(), fp, "FP/SIMD state diverged across a NEON-crossing window");
    assert!(cold.diff_memory(&mem).is_none(), "memory diverged across a NEON-crossing window");
}

#[test]
fn large_window_second_nearby_seek_is_far_cheaper_than_the_first() {
    let (rec, trace) = util::record(retrace_guest::SPINLOOP);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let trace = Path::new(&trace);
    let mut cache = retrace_core::CheckpointCache::new(256 * 1024 * 1024, 64);
    // Landmark 2 = the ~4003-instruction loop2 window (the M4 acceleration target).
    let before1 = cache.total_single_steps();
    let _ = retrace_core::checkpointed_seek(trace, &mut cache, 2, 3990).unwrap();
    let first_cost = cache.total_single_steps() - before1;
    assert!(first_cost >= 3000, "the first seek into a ~4003-insn window should pay most of it, paid {first_cost}");

    let before2 = cache.total_single_steps();
    let _ = retrace_core::checkpointed_seek(trace, &mut cache, 2, 3995).unwrap();
    let second_cost = cache.total_single_steps() - before2;
    assert!(second_cost <= 20, "a nearby second seek should reuse the checkpoint, paid {second_cost}");
    assert!(second_cost * 50 < first_cost,
        "second seek ({second_cost} steps) should be far cheaper than the first ({first_cost})");
}
