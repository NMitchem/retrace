// M39 wall-1 guard: a shared, FIXED|OVERWRITE mach_vm_remap (4813) of an RX page into a
// vm_allocate'd region is serviced as a stage-1 alias — executable through the alias (SELF's
// call returns 42), byte-identical through it (FFI's memcmp), with the protections the KERNEL
// returns natively (measured 2026-09-17, Task 2 Step 5 — the two numbers in EXPECT), and it
// replays bit-for-bit. Repo-owned so the mechanism is guarded on a machine without Homebrew
// Python (cpython_crash_e2e skips there and guards nothing).
//
// The second assertion is on the TRACE: the 4813 landmark carries a synthesised 60-byte reply as
// its recorded write — the route serviced it, nothing forwarded it (a forward would have handed
// the host kernel retrace's own address space, the M2-mach wall).
mod util;
use retrace_trace::{Event, Reader};

// From the native run (Task 2 Step 5). The prediction was cur=5 max=5 for BOTH remaps; the
// kernel disagreed on the second: FFI's max is 7 (rwx), not 5 (r-x) — libffi-trampolines.dylib's
// `__TEXT` is mapped with an elevated max protection (it hands out writable sub-mappings for
// JIT'd closure thunks under W^X), so SELF and FFI genuinely differ. The kernel is right; this
// line carries its numbers, not the prediction's. If a future OS returns different protections
// this line is what changes, together with machmsg::VM_REMAP_{CUR,MAX}_PROT — by measurement, both.
const EXPECT: &[u8] = b"SELF kr=0 cur=5 max=5 call=42\nFFI kr=0 cur=5 max=7 same=1\n";

#[test]
fn a_shared_fixed_remap_is_an_executable_alias_and_replays() {
    let r = util::assert_rung_records_and_replays(retrace_guest::VMREMAP_DYN, &[], EXPECT);
    let remaps: Vec<usize> = Reader::open(&r.trace).unwrap().iter().filter_map(|e| match e {
        Event::Syscall { num, args, writes, .. }
            if *num == (-47i64) as u64 && (args[4] >> 32) == 4813 => Some(writes.iter().map(|w| w.bytes.len()).sum()),
        _ => None,
    }).collect();
    assert_eq!(remaps, vec![60, 60], "two 4813 landmarks, each with one 60-byte synthesised reply: {remaps:?}");
}
