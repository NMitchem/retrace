// M32 Task 1: does the M30 guard band for a mach_msg2 message buffer land at or past `send_size`
// -- the kernel-read boundary -- so the buffer's argument may be canary-filled without
// re-creating the M30 corruption?
//
// FIX ROUND 1 corrected two defects in the original (single-call) measurement below and added the
// test that is now the PRIMARY evidence for the decision:
//
// - Critical 1: msgh_id 4811 (`_kernelrpc_mach_vm_map`, the only message `machmsg.s` sends) routes
//   to `Route::ServiceVmMap` (`crates/retrace-core/src/machmsg.rs:102`) and is SERVICED, never
//   forwarded -- `forward_and_diff` (the only place `SEED_MACH_MSG2` can change anything) runs
//   ONLY for `Route::Forward` ids (the `FORWARD_ALLOWLIST`: 200/206/3418/3405/412). The original
//   test below measured a call this milestone's decision does not govern: n=0 for the governed
//   population, not n=1.
// - Critical 2: for that one call, `avail == win == 16384` exactly (the buffer sits 16384 bytes
//   from the end of its backing), so `band = GUARD_BAND.min(avail - win) = 0`. No canary would
//   ever be written there regardless of `SEED_MACH_MSG2` -- there was no band to describe.
//
// `whenever_a_band_exists_it_starts_past_every_possible_send_size` below is the fix: a structural
// proof, over every `avail` a real forwarded call's buffer could have, rather than one sample of
// a call the decision never touches. A second test captures one REAL triple on a call that
// actually IS forwarded (msgh_id 3405, task_info) from a live dynamic guest --
// `crates/retrace-core/tests/machmsgband_dyn.rs` -- because a bare `Box_` harness like this one
// has no `retrace-core` routing to reach a `Route::Forward` call at all (see the deviation note
// on the original test below).
//
// The original test is kept: it is still a true, useful regression pin on the ServiceVmMap path's
// own (zero-length) band, and on `dbg_window_len_for`'s behavior on a real mapped buffer. It is
// NOT evidence for `SEED_MACH_MSG2` -- see Critical 1/2 above.
use retrace_arch::SYS_EXIT;
use retrace_box::{Box_, Stop};

const MACH_MSG2: u64 = (-47i64) as u64;

/// The ORIGINAL (fix-round-corrected) observational test. Demonstrates `dbg_window_len_for` on a
/// real serviced (not forwarded) mach_msg2 call; does NOT bear on `SEED_MACH_MSG2` -- see the
/// file-level comment's Critical 1/2.
#[test]
fn the_serviced_vm_map_calls_band_is_measured_zero_and_out_of_scope() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::MACHMSG).unwrap());
    let mut b = Box_::load(&loaded);
    let mut seen = 0usize;
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == MACH_MSG2 => {
                let send_size = (args[2] >> 32) as usize;
                let rcv_size = (args[6] & 0xffff_ffff) as usize;
                let len = b.dbg_window_len_for(args[0])
                    .expect("machmsg.s's message buffer must be a mapped guest IPA");
                // Printed so the triples land in the task report verbatim, per Step 3.
                eprintln!("[M32 t1] buf={:#x} len={len} send_size={send_size} rcv_size={rcv_size} \
                           (msgh_id 4811, Route::ServiceVmMap -- NOT forwarded, NOT governed by \
                           SEED_MACH_MSG2)", args[0]);
                // Still true, and still worth pinning: even on the wrong-population call, the
                // window never lands inside the kernel-read region. But see Critical 2 -- here
                // `len == avail`, so `band = GUARD_BAND.min(avail - len) == 0`: this call has NO
                // band a canary could ever occupy, so this assertion is not exercising the risk
                // `SEED_MACH_MSG2` is about.
                assert!(len >= send_size,
                    "band would land INSIDE the kernel-read region: len {len} < send_size \
                     {send_size} for buffer {:#x}.", args[0]);
                seen += 1;
                // MEASURED (not in the original brief): resuming past this trap here would forward
                // the raw svc to the REAL host kernel, bypassing retrace-core's Route::ServiceVmMap
                // -- this bare `Box_` harness has no such routing. The real `mach_vm_map` RPC then
                // succeeds against the *test process's own* task port (itself forwarded above) and
                // hands back a genuine HOST virtual address in the reply, not a guest IPA. The
                // guest's very next instruction stores through that address as if it were a guest
                // pointer. That store is a GUEST EL0 access mediated by stage-2 translation as well
                // as the guest's own stage-1 tables; the host address is outside BOTH, so it
                // fails at stage 2 before ever consulting the guest's stage-1 tables at all -- a
                // Data Abort taken direct to EL2 (`Stop::Other`, not the trampoline's `Stop::Fault`
                // path, which only carries a stage-1 EL0 miss forwarded up through the vector code;
                // see `lib.rs:2467-2471` vs `lib.rs:2495`). Corrected from the original report,
                // which blamed the guest's stage-1 tables specifically -- the effect (an
                // unreachable address) was right, the fault-level label was not. `machmsg.s`
                // dispatches exactly one `mach_msg2` call, so the measurement this test exists for
                // is already complete the moment it is captured above; stop here rather than resume
                // into a fault that belongs to Route::ServiceVmMap's absence, not to the
                // window-length question this test answers.
                break;
            }
            Stop::Syscall { num, args: _ } if num == SYS_EXIT => break,
            Stop::Syscall { num, args } => {
                let (ret, err, _w) = b.forward_and_diff(num, args);
                b.set_x0_err_and_return(ret, err);
            }
            Stop::Other { esr } => panic!("unexpected exit esr=0x{esr:x}"),
            Stop::Fault { pc, esr, far } => panic!("guest crashed pc=0x{pc:x} esr=0x{esr:x} far=0x{far:x}"),
            Stop::Step => unreachable!("run() does not single-step"),
        }
    }
    assert!(seen > 0,
        "machmsg guest dispatched ZERO mach_msg2 calls — this measurement measured nothing, \
         which is the dead-channel trap the spec's §4b exists to catch");
}

/// PRIMARY EVIDENCE for `SEED_MACH_MSG2`, added in fix round 1. Proves the invariant structurally
/// over every `avail` a real forwarded call's message buffer could have, rather than trusting the
/// one 16384-byte sample the original test happened to exercise (which, per Critical 1, is not
/// even a call this decision governs).
///
/// The argument, each step asserted rather than only stated:
///
/// 1. `retrace_arch::dest_buffer` has no match arm for `mach_msg2_trap` (`-47`) or any other mach
///    trap -- its whole table is keyed on BSD syscall constants -- so `diff_window`'s `None` arm
///    (`crates/retrace-box/src/lib.rs:3069`: `base = avail.min(self.window_cap)`) fires for a
///    mach_msg2 buffer at EVERY `avail`, not just the one this fixture's guest happens to produce.
/// 2. Every production `Box_` constructor (`load`, `load_dynamic`, `restore`, the `BoxState`
///    restore path) hard-codes `window_cap = PTR_WINDOW_CAP`; `set_window_cap_for_test` is the
///    only setter and is called only from `truncguard.rs`. Checked here against two REAL
///    production constructors rather than cited as a sentence; the other two are visually
///    identical `window_cap: PTR_WINDOW_CAP` field literals (`lib.rs:2764`, `:5294`) not exercised
///    by this test but cross-checked live from a real dynamic recording's `ReplaySession` in
///    `crates/retrace-core/tests/machmsgband_dyn.rs` (the `restore` constructor).
/// 3. `band = GUARD_BAND.min(avail - win)` (`lib.rs:3180`) is nonzero only when `avail > win`; with
///    `win = avail.min(window_cap)`, `avail > win` forces the `min` to have saturated at
///    `window_cap` -- so a band can exist AT ALL only when `win == window_cap == 65536`.
/// 4. `crates/retrace-core/src/lib.rs:435` asserts `send_size <= 0x1000` (4096) for EVERY
///    mach_msg2 call, BEFORE `route()` even runs -- so this bound holds regardless of which id
///    fires or whether it is forwarded.
/// 5. `65536 > 4096`: whenever a band exists, it begins at least 61440 bytes past the last byte
///    the kernel is permitted to read. The band can never land inside the kernel-read region.
#[test]
fn whenever_a_band_exists_it_starts_past_every_possible_send_size() {
    // Step 4's bound, named so the arithmetic below reads against the actual cited assert rather
    // than a bare literal.
    const SEND_SIZE_MAX: usize = 0x1000;

    // Step 1, sanity: mach_msg2 is genuinely absent from the destination-length table (mirrors
    // retrace-arch's own `dest_buffer_omits_what_it_should`-style coverage, for THIS trap number
    // specifically, which that suite does not name).
    assert_eq!(retrace_arch::dest_buffer(MACH_MSG2), None,
        "if mach_msg2 ever gained a dest_buffer entry, diff_window could widen its window past \
         window_cap for a KNOWN length, and this proof's `None => base` premise would be false");

    // Step 2 (first of two production constructors), plus steps 1+3's sweep. One VM per process
    // (CLAUDE.md) means the dynamic-constructor check below cannot start until this `Box_` is
    // fully dropped, so this block does ALL of its work -- construct, check, sweep -- and lets
    // `static_box` go out of scope before the second constructor is built.
    {
        let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::MACHMSG).unwrap());
        let static_box = Box_::load(&loaded);
        assert_eq!(static_box.dbg_window_cap(), retrace_box::PTR_WINDOW_CAP,
            "Box_::load (the static production constructor) must set window_cap == PTR_WINDOW_CAP");

        // The concrete headroom this whole test exists to establish. Both operands are
        // compile-time constants, so clippy's `assertions_on_constants` wants this evaluated at
        // compile time rather than asserted at runtime -- which is exactly the point: if this
        // headroom ever stopped holding, the build itself should refuse to produce a binary whose
        // guard-band seed decision rests on it.
        assert_eq!(retrace_box::PTR_WINDOW_CAP, 65536);
        const { assert!(retrace_box::PTR_WINDOW_CAP > SEND_SIZE_MAX,
            "the seed's safety margin (this test's whole conclusion) is only as good as \
             window_cap staying above the kernel's own send_size ceiling") };

        // Steps 1+3, swept over a representative range rather than the one avail this fixture's
        // guest happens to produce -- including both sides of window_cap's saturation point and
        // both sides of SEND_SIZE_MAX.
        const AVAILS: &[usize] = &[
            0, 1, SEND_SIZE_MAX - 1, SEND_SIZE_MAX, SEND_SIZE_MAX + 1,
            65535, 65536, 65537, 100_000, 10_000_000,
        ];
        for &avail in AVAILS {
            // `i` and `args` are irrelevant to the result for MACH_MSG2 (step 1), so any values
            // serve; `diff_window_for_test` is the SAME private `diff_window` `forward_and_diff`
            // calls.
            let win = static_box.diff_window_for_test(MACH_MSG2, 0, avail, &[0u64; 8]);
            assert_eq!(win, avail.min(static_box.dbg_window_cap()),
                "diff_window's None arm must reduce to avail.min(window_cap) for mach_msg2 at \
                 EVERY avail ({avail}), not only the one avail the original single-call test \
                 happened to hit");
            let band = retrace_box::GUARD_BAND.min(avail.saturating_sub(win));
            if band > 0 {
                // Step 3's conclusion, then step 5: whenever a band exists at all, its start
                // (`win`) has saturated at window_cap, which step 4 already bounds send_size well
                // under.
                assert_eq!(win, static_box.dbg_window_cap(),
                    "a nonzero band (avail={avail} win={win} band={band}) must mean win saturated \
                     at window_cap -- if this ever fails, the band could start before window_cap \
                     and this proof's headroom argument no longer holds");
                assert!(win >= SEND_SIZE_MAX,
                    "avail={avail}: a band exists here but win ({win}) does not clear the \
                     kernel's own send_size ceiling ({SEND_SIZE_MAX}) -- the hypothesis is \
                     REFUTED for this avail, and mach_msg2 must stay withheld from the \
                     canary-fill allow-list (spec §7, last bullet)");
            }
            // avail == 0 and avail == SEND_SIZE_MAX - 1 (etc.) are the band == 0 cases: no canary
            // is ever written there, so there is nothing for SEND_SIZE_MAX to be compared against
            // -- asserting `len >= send_size` on them (as the original single-call test did) would
            // be asserting about a risk that provably cannot occur, not about the one this test
            // proves does not occur.
        }
    } // static_box dropped here -- its VM is destroyed before the dynamic constructor below runs.

    // Step 2 (second production constructor): Box_::load_dynamic, live-checked independently.
    // Only its `window_cap` field matters here, so it is dropped unexercised (no `run()` call) --
    // constructing it is already enough to observe what its field initializer set.
    let exe = retrace_guest::parse_macho(&std::fs::read(retrace_guest::HELLO_DYN).unwrap());
    let dyld_path = exe.dylinker.clone().unwrap_or_else(|| retrace_guest::DYLD_PATH.to_string());
    let dyld_bytes = std::fs::read(&dyld_path).unwrap_or_else(|e| panic!("read dyld {dyld_path}: {e}"));
    let dyld = retrace_guest::parse_macho(retrace_guest::slice_arm64e(&dyld_bytes));
    let dynamic_box = Box_::load_dynamic(&exe, &dyld, &[retrace_guest::HELLO_DYN.to_string()]);
    assert_eq!(dynamic_box.dbg_window_cap(), retrace_box::PTR_WINDOW_CAP,
        "Box_::load_dynamic (the dynamic production constructor) must ALSO set window_cap == \
         PTR_WINDOW_CAP -- a mismatch here would mean the two production entry points disagree, \
         which this proof cannot assume away");
}
