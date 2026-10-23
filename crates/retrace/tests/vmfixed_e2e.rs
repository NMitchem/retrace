// M52 §3b: FIXED Mach VM requests answer as xnu does. Without VM_FLAGS_OVERWRITE, a FIXED
// mach_vm_map or mach_vm_allocate over occupied memory is KERN_NO_SPACE and the old bytes survive;
// before M52 the box replaced them silently. libmalloc relies on that answer when it grows a huge
// chunk in place (the regex crash, and a Rust Vec grown past 2 MiB then a thread spawn, which was the
// bug banner because the bump cursor did not float past a FIXED placement). `mach_vm_copy` (MIG 4807)
// is the box-side copy libmalloc falls back to. Every row runs natively first, so the expected output
// is native's and not a hand-written guess.
mod util;
use retrace_trace::{Event, Reader};

/// Native run of `guest` with `args`: exit 0, then record and replay it twice and require the same
/// stdout. Returns that stdout.
fn as_native(guest: &str, args: &[&str]) -> String {
    let (out, _) = as_native_run(guest, args);
    out
}

fn as_native_run(guest: &str, args: &[&str]) -> (String, util::RungOut) {
    let native = util::run_bounded(guest, args, &[], 30);
    assert_eq!(native.code, Some(0), "native {guest} {args:?}: {}", native.stderr);
    let r = util::assert_rung_records_and_replays(guest, args, &native.stdout);
    (String::from_utf8(native.stdout).unwrap(), r)
}

#[test]
fn a_fixed_request_over_occupied_memory_answers_kern_no_space_and_keeps_the_memory() {
    let out = as_native(retrace_guest::VMFIXED_DYN, &["cases"]);
    for case in ["contained", "covering", "straddling", "reservation", "allocate", "mig"] {
        assert!(out.contains(&format!("{case} kr=3")), "{case}:\n{out}");
    }
    for case in ["contained", "covering", "straddling", "allocate", "mig"] {
        assert!(out.contains(&format!("{case} kr=3 marker=intact")), "{case}:\n{out}");
    }
    assert!(out.contains("free kr=0") && out.contains("adjacent kr=0 marker=intact"), "{out}");
}

#[test]
fn an_overwrite_request_over_the_same_range_still_replaces_it() {
    let out = as_native(retrace_guest::VMFIXED_DYN, &["overwrite"]);
    assert!(out.contains("overwrite kr=0 marker=lost"), "{out}");
}

#[test]
fn a_page_mapped_past_a_huge_chunk_survives_the_chunks_growth() {
    let out = as_native(retrace_guest::HUGEREALLOC_DYN, &["page"]);
    assert!(out.contains("page marker=intact"), "{out}");
}

#[test]
fn a_vec_grown_past_24_mib_then_a_thread_spawn_exits_0_as_natively() {
    // Exit 70 (the bug banner) is the failure, so the exit code and the output are asserted together
    // (assert_rung_records_and_replays demands exit 0 with exactly native's stdout).
    let out = as_native(retrace_guest::VECGROW, &[]);
    assert_eq!(out, "vecgrow ok\n");
    let out = as_native(retrace_guest::HUGEREALLOC_DYN, &["grow"]);
    assert_eq!(out, "grow ok\n");
}

#[test]
fn mach_vm_copy_copies_the_bytes_and_answers_natives_codes_without_recording_them() {
    for case in ["disjoint", "overlap_fwd", "overlap_back", "unmapped_src", "none_dst", "unaligned", "zero", "reserved"] {
        let (out, r) = as_native_run(retrace_guest::VMCOPY_DYN, &[case]);
        assert!(out.starts_with(&format!("{case} kr=")), "{case}: {out}");
        if case != "disjoint" { continue; }
        // The copy is recomputed, never recorded: every 4807 landmark's only write is the 44-byte
        // reply (msgh_id 4907) at the message buffer.
        let mut replies = 0;
        for e in Reader::open(&r.trace).unwrap().iter() {
            let Event::Syscall { writes, .. } = e else { continue };
            let Some(w) = writes.iter().find(|w| w.bytes.len() == 44
                && u32::from_le_bytes(w.bytes[20..24].try_into().unwrap()) == 4907) else { continue };
            replies += 1;
            assert_eq!(writes.len(), 1, "a 4807 landmark records the reply only, got {} regions", writes.len());
            assert_eq!(w.bytes.len(), 44);
        }
        assert!(replies >= 1, "the disjoint recording holds a 4807 landmark");
    }
    // The destination's bytes arrived: the unaligned case copied exactly 100 bytes, which only the
    // checksum (compared with native above) can tell from a page.
}

#[test]
fn a_fixed_request_into_retraces_own_windows_answers_as_native_does() {
    // No window is a wall under M52 §3b (ruling T0-M2): a start below 4 GiB is KERN_INVALID_ADDRESS, the
    // shared region and the commpages are KERN_NO_SPACE, so the plan's "a window ruled a wall ends the
    // recording" clause has no row. 0x300000000 is free natively and in the box (the nano band starts
    // at 0x400000000).
    let out = as_native(retrace_guest::VMFIXED_DYN, &["windows"]);
    for (addr, kr) in [("0x4000", 1), ("0x8000", 1), ("0x40000", 1), ("0x800000", 1), ("0x180000000", 3),
                       ("0x2ffffc000", 3), ("0xfffffc000", 3), ("0x300000000", 0)] {
        assert!(out.contains(&format!("window {addr} kr={kr}\n")), "{addr}:\n{out}");
    }
}
