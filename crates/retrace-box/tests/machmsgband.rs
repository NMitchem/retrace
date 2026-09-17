// M32 Task 1: does the M30 guard band for a mach_msg2 message buffer land at or past `send_size`
// -- the kernel-read boundary -- so the buffer's argument may be canary-filled without
// re-creating the M30 corruption?
//
// FIX ROUND 1 corrected two defects in the original (single-call) measurement below and added the
// test that is now the PRIMARY evidence for the decision:
//
// - Critical 1: msgh_id 4811 (`_kernelrpc_mach_vm_map`, the only message `machmsg.s` sends) routes
//   to `Route::ServiceVmMap` (`crates/retrace-core/src/machmsg.rs:113`) and is SERVICED, never
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
//
// **`SEED_MACH_MSG2` NEVER BECAME A SYMBOL, AND NOW NEVER WILL** -- `grep` finds these comments and
// no definition. It was the decision variable this measurement existed to settle: `true` would have
// added `mach_msg2` argument 0 to a per-argument canary-fill allow-list. The measurement came back
// INERT (13 governed calls across three guests, max `avail` 24,672 against a 65,536 threshold, zero
// bands), so M32 closed as a measurement milestone, dropped the tasks that would have built that
// allow-list, and there is no allow-list to withhold `mach_msg2` FROM: the exclusion is still
// `retrace_arch::reads_guest_buffer`'s whole-syscall predicate, exactly as before M32. See
// `docs/superpowers/specs/2026-09-09-retrace-m32-dirtable-design.md` §9. The name is kept in these
// comments because it names the question every assertion below is scoped to.
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
                // The `_band_is_measured_zero_` half of this test's NAME, asserted rather than left
                // to the comment above (M32 finding 6: through fix round 2 the body read only the
                // window length and never `avail`, so the name claimed a measurement the test did
                // not make -- the shape this repo has caught in itself repeatedly). `band_len` is
                // the same hoisted function `forward_and_diff` calls, over the same `avail` the
                // diff loop's own `host_span` reports.
                let (_, avail) = b.host_span_for_test(args[0])
                    .expect("machmsg.s's message buffer must be a mapped guest IPA");
                assert_eq!(Box_::band_len(avail, len), 0,
                    "this call's band is supposed to be measured ZERO (avail {avail} == win {len}, \
                     so GUARD_BAND.min(avail - win) == 0) -- if it is not, the fixture's geometry \
                     changed and this test's name no longer describes what it measures");
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
                let (ret, _ret1, err, _w) = b.forward_and_diff(num, args);
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

/// PRIMARY EVIDENCE for `SEED_MACH_MSG2`, added in fix round 1, hardened in fix round 2. Proves
/// the invariant structurally over every `avail` a real forwarded call's message buffer could
/// have, rather than trusting the one 16384-byte sample the original test happened to exercise
/// (which, per Critical 1, is not even a call this decision governs).
///
/// The argument, each step asserted rather than only stated:
///
/// 1. `retrace_arch::dest_buffer` has no match arm for `mach_msg2_trap` (`-47`) or any other mach
///    trap -- its whole table is keyed on BSD syscall constants -- so `diff_window`'s `None` arm
///    (`crates/retrace-box/src/lib.rs:3083`: `base = avail.min(self.window_cap)` -- it moved from
///    the `:3069` this comment cited through fix round 2, pushed down by `band_len`'s own hoist)
///    fires for a
///    mach_msg2 buffer at EVERY `avail`, not just the one this fixture's guest happens to produce.
/// 2. Every production `Box_` constructor (`load`, `load_dynamic`, `restore`, the `BoxState`
///    restore path) hard-codes `window_cap = PTR_WINDOW_CAP`; `set_window_cap_for_test` is the
///    only setter and is called only from `truncguard.rs`. Checked here against two REAL
///    production constructors by reading `window_cap` THROUGH `diff_window` itself --
///    `diff_window_for_test(MACH_MSG2, 0, usize::MAX, &[0u64;8])` returns `usize::MAX.min(window_cap)
///    == window_cap`, exercising the exact function `forward_and_diff` calls rather than a
///    dedicated field-reading accessor (fix round 2, Minor D: strictly better evidence, and it
///    reduces the accessor surface at the same time). The other two production constructors,
///    `restore` and `from_checkpoint`'s `BoxState` path, are reachable only through a
///    `ReplaySession`, not a bare `Box_`, and are cross-checked live in
///    `crates/retrace-core/tests/machmsgband_dyn.rs`.
/// 3. `band = Box_::band_len(avail, win)` -- `GUARD_BAND.min(avail - win)`, hoisted verbatim out of
///    `forward_and_diff` (fix round 2, Important A: this test now calls the SAME function
///    production runs, not a re-derived copy of its formula) -- is nonzero only when `avail > win`;
///    with `win = avail.min(window_cap)`, `avail > win` forces the `min` to have saturated at
///    `window_cap` -- so a band can exist AT ALL only when `win == window_cap`. This also depends
///    on `band_not_covered` (`lib.rs:2992`) never being able to MOVE a band's start once
///    `band_len` has fixed it: that function's signature returns a shrunk *length* only, and the
///    start (`ipa + len`) is computed independently at both the fill and the restore sites, so no
///    value it returns could relocate the start `band_len` establishes here. Not asserted
///    separately (the reviewer's ruling: an assertion here would be theatre, since no test could
///    fail the property without an API change first) -- recorded here as the reason this proof
///    does not need to.
/// 4. **Steps 4 and 5 are an EXTERNAL PREMISE this crate structurally cannot check.**
///    `crates/retrace-core/src/lib.rs:435` asserts `send_size <= machmsg::SEND_SIZE_MAX` (4096) for
///    EVERY mach_msg2 call, BEFORE `route()` even runs, so the bound holds regardless of which id
///    fires or whether it is forwarded -- but `retrace-box` cannot depend on `retrace-core` (the
///    dependency runs the other way), so `machmsg::SEND_SIZE_MAX` is INVISIBLE here and **no
///    constant in this file stands in for it**. One did, briefly: a hand-copied
///    `SEND_SIZE_MAX_MIRROR`, which nothing compared to the thing it mirrored, so it could not
///    detect the drift its own name implied it caught. It is deleted rather than renamed.
/// 5. The premise is asserted at COMPILE TIME in the crate that owns both operands, by the
///    module-scope `const _: () = assert!(machmsg::SEND_SIZE_MAX < retrace_box::PTR_WINDOW_CAP)` in
///    `crates/retrace-core/tests/machmsgband_dyn.rs`. **Steps 1-3 above are what THIS file proves**:
///    a band, whenever one exists, starts at `ipa + window_cap`. Step 5's conclusion -- that the
///    band therefore begins past the last byte the kernel may read, and can never land inside the
///    kernel-read region -- follows from steps 1-3 TOGETHER WITH that assertion, and from nothing
///    in this file alone.
#[test]
fn whenever_a_band_exists_it_starts_past_every_possible_send_size() {
    // Step 1, sanity: mach_msg2 is genuinely absent from the destination-length table (mirrors
    // retrace-arch's own `dest_buffer_omits_what_it_should`-style coverage, for THIS trap number
    // specifically, which that suite does not name).
    assert_eq!(retrace_arch::dest_buffer(MACH_MSG2), None,
        "if mach_msg2 ever gained a dest_buffer entry, diff_window could widen its window past \
         window_cap for a KNOWN length, and this proof's `None => base` premise would be false");

    // Step 2 (first of two production constructors reachable from a bare Box_), plus steps 1+3's
    // sweep. One VM per process (CLAUDE.md) means the dynamic-constructor check below cannot start
    // until this `Box_` is fully dropped, so this block does ALL of its work -- construct, check,
    // sweep -- and lets `static_box` go out of scope before the second constructor is built.
    {
        let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::MACHMSG).unwrap());
        let static_box = Box_::load(&loaded);
        // Reads window_cap THROUGH diff_window (see step 2's doc comment) rather than through a
        // field-reading accessor: `usize::MAX.min(window_cap)` can only equal `window_cap`.
        let window_cap = static_box.diff_window_for_test(MACH_MSG2, 0, usize::MAX, &[0u64; 8]);
        // The headroom half of the conclusion (`window_cap` above the kernel's send_size ceiling)
        // is NOT asserted here and cannot be -- see steps 4-5: it lives in
        // `crates/retrace-core/tests/machmsgband_dyn.rs`'s module-scope `const _: () = assert!(...)`,
        // the one place both constants are visible. What IS checkable in this crate, and is the
        // thing steps 1-3 rest on, is that a real production constructor's `window_cap` is
        // `PTR_WINDOW_CAP`.
        assert_eq!(window_cap, retrace_box::PTR_WINDOW_CAP,
            "Box_::load (the static production constructor), read through diff_window itself, \
             must report window_cap == PTR_WINDOW_CAP");

        // Steps 1+3, swept over a representative range rather than the one avail this fixture's
        // guest happens to produce -- including both sides of window_cap's saturation point
        // (65535/65536/65537) and, as bare literals, the neighbourhood of the kernel's send_size
        // ceiling AS OF M32 (4095/4096/4097). Those three are sweep POINTS only: this file enforces
        // nothing about that ceiling and cannot (steps 4-5), so they are written as numbers rather
        // than through a named constant that would imply a correspondence nothing checks.
        const AVAILS: &[usize] = &[
            0, 1, 4095, 4096, 4097,
            65535, 65536, 65537, 100_000, 10_000_000,
        ];
        for &avail in AVAILS {
            // `i` and `args` are irrelevant to the result for MACH_MSG2 (step 1), so any values
            // serve; `diff_window_for_test` is the SAME private `diff_window` `forward_and_diff`
            // calls.
            let win = static_box.diff_window_for_test(MACH_MSG2, 0, avail, &[0u64; 8]);
            assert_eq!(win, avail.min(window_cap),
                "diff_window's None arm must reduce to avail.min(window_cap) for mach_msg2 at \
                 EVERY avail ({avail}), not only the one avail the original single-call test \
                 happened to hit");
            // `Box_::band_len` is the SAME hoisted function `forward_and_diff` calls (fix round 2,
            // Important A) -- not a re-derivation of its formula. `win <= avail` always here (it
            // is `avail.min(window_cap)`), so this can never hit `band_len`'s underflow panic.
            let band = Box_::band_len(avail, win);
            if band > 0 {
                // Step 3's conclusion, then step 5: whenever a band exists at all, its start
                // (`win`) has saturated at window_cap, which step 4 already bounds send_size well
                // under.
                assert_eq!(win, window_cap,
                    "a nonzero band (avail={avail} win={win} band={band}) must mean win saturated \
                     at window_cap -- if this ever fails, the band could start before window_cap \
                     and this proof's headroom argument no longer holds");
                // Nothing further is asserted here about the kernel's send_size ceiling. A
                // `win >= 4096` would be strictly implied by the `assert_eq!` above (which pins
                // `win` to 65536) and would compare against a literal this file cannot relate to
                // the production bound -- the mirror's whole defect, in one line. The ceiling is
                // handled where it can be: steps 4-5.
            }
            // avail == 0 and avail == 4095 (etc.) are the band == 0 cases: no canary
            // is ever written there, so there is nothing for a send_size to be compared against
            // -- asserting `len >= send_size` on them (as the original single-call test did) would
            // be asserting about a risk that provably cannot occur, not about the one this test
            // proves does not occur.
        }
    } // static_box dropped here -- its VM is destroyed before the dynamic constructor below runs.

    // Step 2 (second production constructor reachable from a bare Box_): Box_::load_dynamic,
    // live-checked independently, again through `diff_window_for_test` rather than a field-reading
    // accessor. Only its `window_cap` matters here, so it is dropped unexercised (no `run()` call)
    // -- constructing it is already enough to observe what its field initializer set.
    let exe = retrace_guest::parse_macho(&std::fs::read(retrace_guest::HELLO_DYN).unwrap());
    let dyld_path = exe.dylinker.clone().unwrap_or_else(|| retrace_guest::DYLD_PATH.to_string());
    let dyld_bytes = std::fs::read(&dyld_path).unwrap_or_else(|e| panic!("read dyld {dyld_path}: {e}"));
    let dyld = retrace_guest::parse_macho(retrace_guest::slice_arm64e(&dyld_bytes));
    let dynamic_box = Box_::load_dynamic(&exe, &dyld, &[retrace_guest::HELLO_DYN.to_string()]);
    let dynamic_window_cap = dynamic_box.diff_window_for_test(MACH_MSG2, 0, usize::MAX, &[0u64; 8]);
    assert_eq!(dynamic_window_cap, retrace_box::PTR_WINDOW_CAP,
        "Box_::load_dynamic (the dynamic production constructor), read through diff_window itself, \
         must ALSO report window_cap == PTR_WINDOW_CAP -- a mismatch here would mean the two \
         production entry points disagree, which this proof cannot assume away");
}
