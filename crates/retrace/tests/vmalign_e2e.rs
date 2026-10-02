// The mach_vm_map alignment-mask guard (M47 abort diagnosis). libmalloc's xzone allocator maps
// each 4 MiB segment ANYWHERE with mask 0x3fffff and registers it in a segment table indexed by
// `addr >> 22`, one entry per 4 MiB step from the base (`_xzm_segment_table_allocated_at`). Until
// the fix the box ignored the mask, so the segment came back off a 4 MiB boundary, the granule its
// tail straddled was never registered, and `free` of a chunk there aborted with "pointer being
// freed was not allocated": Xcode's git, `log -1` about one run in ten, decided by the random size
// of libmalloc's guarded-range reservation. Record and replay agreed throughout — both sides
// ignored the mask identically — so this asserts on the GUEST's view of the address, which is the
// one thing the bug changed. A repo-owned fixture rather than git: git is not a repo artifact, and
// its abort depends on recorded entropy and the wall clock.
//
// Each map in the fixture is preceded by a one-page allocation, so under a placement that ignores
// the mask at most one of the five can come back aligned: this fails whatever state the run starts
// in (measured on the unfixed binary: all five `aligned=0`, and replay exits 0 reproducing them).
mod util;
use retrace_trace::{Event, Reader};

// The native run's output, every route aligned.
const EXPECT: &[u8] = b"trap kr=0 aligned=1\nmig kr=0 aligned=1\ntrap-reserve kr=0 aligned=1\n\
mig-reserve kr=0 aligned=1\ntrap-hinted kr=0 aligned=1\n";

#[test]
fn every_masked_anywhere_map_comes_back_aligned_and_replays() {
    let r = util::assert_rung_records_and_replays(retrace_guest::VMALIGN_DYN, &[], EXPECT);
    // Both routes were exercised. The fixture's own trap-route maps are the only -15 calls with
    // this mask AND untagged flags (libmalloc's segments carry VM tag 2), so exactly three — map,
    // reserve, hinted — means the other two went to MIG 4811, whose mask sits in the message body
    // where the trace cannot see it. Without this, a libsystem change that moved every call onto
    // one route would leave the other unguarded while the stdout still matched.
    let trap = Reader::open(&r.trace).unwrap().iter().filter(|e| matches!(e,
        Event::Syscall { num, args, .. }
            if *num == (-15i64) as u64 && args[3] == 0x3f_ffff && args[4] == 1)).count();
    assert_eq!(trap, 3, "three masked maps take the trap route (map, reserve, hinted); the other two are 4811's");
}
