// M32 Task 1 fix round 1 + 2, Important 3 / (b) / C: real (len, send_size, rcv_size) triples on
// calls `SEED_MACH_MSG2` actually governs, plus a full corpus walk settling whether the seed is
// INERT (every real mach_msg2 this repo can record produces a zero-length band, so
// `SEED_MACH_MSG2` currently changes nothing observable) or governs a real nonzero band somewhere.
//
// **`SEED_MACH_MSG2` NEVER BECAME A SYMBOL, AND NOW NEVER WILL.** It was the decision variable this
// measurement existed to settle: `true` would have added `mach_msg2` argument 0 to a per-argument
// canary-fill allow-list. The measurement came back INERT (13 governed calls across three guests,
// max `avail` 24,672 against a 65,536 threshold, zero bands), so M32 closed as a measurement
// milestone and dropped the tasks that would have built the allow-list -- there is nothing to
// `grep` for. Read `docs/superpowers/specs/2026-09-09-retrace-m32-dirtable-design.md` §9 before
// concluding anything from the name's absence. It is kept in these comments because it names the
// question every assertion below is scoped to, and renaming it would make this file's own history
// unreadable against the report and spec that discuss it.
//
// `crates/retrace-box/tests/machmsgband.rs`'s structural proof establishes the invariant for every
// POSSIBLE forwarded call; this file supplies the thing a pure proof cannot -- genuine samples
// from real dynamically-linked guests, on calls that are actually forwarded (`Route::Forward`),
// which is the only route `SEED_MACH_MSG2` can ever change anything for
// (`crates/retrace-core/src/machmsg.rs:108-109` returns `Route::Forward`; the ids it returns it
// for are `FORWARD_ALLOWLIST`, `machmsg.rs:64-81`).
//
// This lives in `retrace-core` (not `retrace-box`) because reaching a `Route::Forward` call at all
// needs `retrace-core`'s own mach_msg2 routing -- exactly the routing a bare `Box_` harness in
// `retrace-box`'s own tests does not have (see that crate's `machmsgband.rs` file comment).
//
// **The REAL triple below is measured on the REPLAY side, while `SEED_MACH_MSG2` governs the
// RECORD side (`forward_and_diff` runs only on record; replay applies recorded writes verbatim).**
// This is benign, not a mismatch: `Box_::restore` (which `ReplaySession::open` uses) builds
// exactly one backing per snapshot region (`crates/retrace-box/src/lib.rs:2680-2687`), the stack
// is one such region, and the observed `buf + len == 0x27fbe38 + 16840 == 0x2800000 ==
// DYN_STACK_TOP` (`lib.rs:94`) -- so replay's backing for the stack is the SAME region, with the
// SAME size, as record's, and `avail` (hence `win`, hence `band`) is identical on both sides for
// this call. Recorded here so a later reader does not have to re-derive it.
use retrace_box::Box_;
use retrace_core::machmsg::{self, Msg2, Route, SEND_SIZE_MAX};

const MACH_MSG2: u64 = (-47i64) as u64;
const TASK_INFO_MSGH_ID: u32 = 3405;
// task_self_trap: record_box/ReplaySession::advance both learn `guest_task_port` from this
// call's recorded, unerrored return (crates/retrace-core/src/lib.rs:672, :1814) -- `route()`
// needs it to recognize task-destined kernel RPCs. Replicated here (reading the SAME recorded
// value out of the trace, not a re-derivation of any logic) so this file can call the REAL
// `machmsg::route()` rather than hand-copy `FORWARD_ALLOWLIST`'s id list.
const MACH_TASK_SELF: u64 = (-28i64) as u64;
// `SEND_SIZE_MAX` is IMPORTED above, not redefined here (M32 finding 2). It was a local
// `const SEND_SIZE_MAX: usize = 0x1000;` through fix round 2, which made every assertion below a
// comparison against this file's OWN copy of the number: widening the production bound at
// `crates/retrace-core/src/lib.rs:435` to anything at all would have left this file green while the
// structural proof it exists to pin (`PTR_WINDOW_CAP > SEND_SIZE_MAX`) quietly became false. It is
// now the same `pub const` that assert reads, so this file is the one place in the repo where the
// premise is CHECKED against real captured values rather than cited -- see the per-landmark
// assertion in `every_real_mach_msg2_in_the_corpus_is_checked_for_a_nonzero_band` and the one in
// the single-triple test below.

/// Builds `(exe, dyld)` `Loaded` pairs exactly as `crates/retrace/src/main.rs`'s `record-dyn` CLI
/// path does, and records into a fresh temp trace. Shared by every guest this file records, so the
/// dyld-resolution logic (real Homebrew binaries carry their own `LC_LOAD_DYLINKER`; the repo's own
/// fixtures fall back to `DYLD_PATH`) is written once.
fn record_dynamic_guest(guest_path: &str, extra_argv: &[&str], label: &str) -> std::path::PathBuf {
    let exe = retrace_guest::parse_macho(&std::fs::read(guest_path).unwrap());
    let dyld_path = exe.dylinker.clone().unwrap_or_else(|| retrace_guest::DYLD_PATH.to_string());
    let dyld_bytes = std::fs::read(&dyld_path).unwrap_or_else(|e| panic!("read dyld {dyld_path}: {e}"));
    let dyld = retrace_guest::parse_macho(retrace_guest::slice_arm64e(&dyld_bytes));
    let mut argv = vec![guest_path.to_string()];
    argv.extend(extra_argv.iter().map(|s| s.to_string()));
    let trace = std::env::temp_dir()
        .join(format!("retrace-m32t1-corpus-{label}-{}.bin", std::process::id()));
    retrace_core::record_dynamic(&exe, &dyld, &argv, &trace)
        .unwrap_or_else(|e| panic!("record {guest_path}: {e}"));
    trace
}

/// One row of the corpus walk: everything needed to judge whether this call's band could ever be
/// nonzero, plus the numbers that answer it. `governed` is the field that matters most: it is
/// `true` only for a call `machmsg::route()` (called for real, not re-implemented) actually
/// resolves to `Route::Forward` -- the ONLY route `forward_and_diff`, and so `SEED_MACH_MSG2`,
/// ever runs for. A landmark with `governed == false` still gets a `(avail, win, band)` computed
/// (what `forward_and_diff` WOULD produce if this call ever reached it), but that number is
/// informational only: production never calls `forward_and_diff` for it, so `SEED_MACH_MSG2`
/// cannot affect it no matter what its band is.
struct Row {
    msgh_id: u32, governed: bool,
    avail: usize, win: usize, band: usize, send_size: usize, rcv_size: usize,
}

/// `guest_task_port`, learned exactly as `record_box`/`ReplaySession::advance` learn it: the
/// recorded, unerrored return of `task_self_trap` (`-28`). `route()` needs this to recognize
/// task-destined kernel RPCs; without it every landmark would misclassify as ungoverned.
fn learn_guest_task_port(events: &[retrace_trace::Event]) -> Option<u64> {
    events.iter().find_map(|e| match e {
        retrace_trace::Event::Syscall { num, ret, err, .. } if *num == MACH_TASK_SELF && !*err =>
            Some(*ret),
        _ => None,
    })
}

/// Walks EVERY mach_msg2 landmark in `trace` (not just one), seeking a fresh `ReplaySession` to
/// each in turn -- one at a time, per CLAUDE.md's one-VM-per-process rule, each dropped (at the end
/// of its loop iteration's scope) before the next is built.
fn walk_all_mach_msg2_landmarks(trace: &std::path::Path) -> Vec<Row> {
    let events = retrace_trace::Reader::open(trace).unwrap();
    let guest_task_port = learn_guest_task_port(&events);
    let landmarks: Vec<usize> = events.iter().enumerate()
        .filter_map(|(i, e)| match e {
            retrace_trace::Event::Syscall { num, .. } if *num == MACH_MSG2 => Some(i),
            _ => None,
        })
        .collect();
    let mut rows = Vec::new();
    for n in landmarks {
        let session = retrace_core::seek(trace, n, 0).expect("seek to a mach_msg2 landmark");
        let (num, args) = session.peek_syscall().expect("landmark must be a Syscall event");
        assert_eq!(num, MACH_MSG2);
        let m = Msg2::unpack(&args);
        // Calls the REAL router, not a hand-copied `FORWARD_ALLOWLIST` id list (Important A's
        // lesson applied here too): this is the exact function `record_box`'s mach_msg2 dispatch
        // calls to decide whether `forward_and_diff` ever runs for this landmark.
        let governed = matches!(machmsg::route(&m, guest_task_port), Route::Forward(_));
        let avail = session.dbg_avail_for(args[0])
            .expect("a mach_msg2 message buffer must be a mapped guest IPA");
        let win = session.dbg_window_len_for(args[0])
            .expect("a mach_msg2 message buffer must be a mapped guest IPA");
        // The SAME hoisted function forward_and_diff calls (Important A) -- not a re-derived
        // formula. `win <= avail` always (win is `avail.min(window_cap)`), so this cannot underflow.
        let band = Box_::band_len(avail, win);
        rows.push(Row {
            msgh_id: m.msgh_id, governed, avail, win, band,
            send_size: m.send_size as usize, rcv_size: m.rcv_size as usize,
        });
    } // `session` dropped here each iteration, before the next `seek` builds a new one.
    rows
}

#[test]
fn the_real_forwarded_task_info_calls_band_lands_at_or_past_send_size() {
    let trace = record_dynamic_guest(retrace_guest::HELLO_DYN, &[], "hello-dyn-single");

    // Find the landmark index of the REAL 3405 send. `Reader::open` yields the same `events`
    // vector `ReplaySession` walks internally, so this index is a valid `seek` coordinate.
    let events = retrace_trace::Reader::open(&trace).unwrap();
    let n = events.iter().position(|e| matches!(e,
            retrace_trace::Event::Syscall { num, args, .. }
                if *num == MACH_MSG2 && Msg2::unpack(args).msgh_id == TASK_INFO_MSGH_ID))
        .unwrap_or_else(|| panic!(
            "hello_dyn's recorded trace never sent msgh_id {TASK_INFO_MSGH_ID} (task_info) -- \
             the real-call fixture this measurement depends on did not fire; see \
             hello_dyn_e2e.rs's M2-taskinfo history comment for the call this was expected to be"));

    // Seek to (n, 0): the session is positioned with landmark n as the NEXT event to consume,
    // memory state exactly as it was when the real record run reached this same trap (replay is
    // byte-identical by construction) -- every backing that will exist at the live trap already
    // exists, because every syscall that could have created one has already been consumed.
    let session = retrace_core::seek(&trace, n, 0).expect("seek to the 3405 landmark");
    let (num, args) = session.peek_syscall().expect("landmark n must be a Syscall event");
    assert_eq!(num, MACH_MSG2);
    let m = Msg2::unpack(&args);
    assert_eq!(m.msgh_id, TASK_INFO_MSGH_ID);

    // Fix round 2, Important B: ASSERT the premise `crates/retrace-core/src/lib.rs:435` enforces,
    // against the real captured value, rather than only citing that line in a comment. This crate
    // owns that assert (retrace-box cannot depend on retrace-core, so it can only cite the number).
    assert!(m.send_size as usize <= SEND_SIZE_MAX,
        "real msgh_id {TASK_INFO_MSGH_ID} send_size {} exceeds the {SEND_SIZE_MAX} ceiling \
         crates/retrace-core/src/lib.rs:435 is supposed to enforce on every mach_msg2 call -- if \
         this fires, either that assert regressed or this constant drifted out of sync with it",
        m.send_size);

    let len = session.dbg_window_len_for(args[0])
        .expect("task_info's message buffer must be a mapped guest IPA");
    let avail = session.dbg_avail_for(args[0])
        .expect("task_info's message buffer must be a mapped guest IPA");
    let band = Box_::band_len(avail, len);
    // Printed so the triple lands in the fix-round report verbatim.
    eprintln!("[M32 t1 fix] REAL msgh_id={} buf={:#x} avail={avail} len={len} band={band} \
               send_size={} rcv_size={} (Route::Forward, genuinely governed by SEED_MACH_MSG2)",
        m.msgh_id, args[0], m.send_size, m.rcv_size);

    assert!(len >= m.send_size as usize,
        "REAL forwarded call: band would land INSIDE the kernel-read region: len {len} < \
         send_size {} for buffer {:#x} -- the hypothesis is REFUTED on a call this decision \
         actually governs, and mach_msg2 must stay withheld (spec §7, last bullet)",
        m.send_size, args[0]);

    // Cross-checks the `restore` production constructor's window_cap against a REAL instance --
    // `ReplaySession::open` (which `seek` calls) builds `self.b` via `Box_::restore`, one of the
    // two production constructors `machmsgband.rs`'s structural proof cannot reach directly (fix
    // round 2, Minor D: the box-side proof now reads `window_cap` through `diff_window` itself for
    // the two constructors it CAN reach; this is the one `ReplaySession` delegator kept for the
    // two it cannot).
    assert_eq!(session.dbg_window_cap(), retrace_box::PTR_WINDOW_CAP,
        "Box_::restore (the constructor every ReplaySession uses) must ALSO set window_cap == \
         PTR_WINDOW_CAP");

    // And the fourth production constructor (`BoxState`'s restore path, used by
    // `ReplaySession::from_checkpoint`), reachable for free from the session already in hand.
    // `checkpoint()` returns an owned, self-contained value, so it can be captured before
    // dropping `session` -- and it MUST be dropped first: one VM per process (`seek`'s own doc
    // comment says so explicitly), and `from_checkpoint` below builds a second live `Box_`.
    let checkpoint = session.checkpoint();
    drop(session);
    let restored = retrace_core::ReplaySession::from_checkpoint(&trace, &checkpoint)
        .expect("from_checkpoint on a freshly taken checkpoint must succeed");
    assert_eq!(restored.dbg_window_cap(), retrace_box::PTR_WINDOW_CAP,
        "Box_::from_checkpoint's BoxState restore path (the fourth production constructor) must \
         ALSO set window_cap == PTR_WINDOW_CAP -- all four production constructors now checked \
         against a live instance, none only cited");
}

/// Fix round 2, Important C: settle whether `SEED_MACH_MSG2` governs anything a real guest can
/// produce, rather than leaving it as an open question after two single-call samples both landed
/// on `band == 0`. Walks EVERY mach_msg2 landmark (not just one) across every dynamically-linked
/// guest fixture available to this session, reports the full `(msgh_id, avail, win, band,
/// send_size, rcv_size)` table, and states the conclusion plainly.
///
/// Per the dispatch instructions: a finding of "no real call ever produces a nonzero band" is a
/// SUCCESS, reported as one. This test does NOT shrink `window_cap` (`set_window_cap_for_test`) to
/// try to manufacture a nonzero band -- doing so would construct exactly the unsafe configuration
/// (`win` pushed below `send_size`) the structural proof excludes, not a genuine observation.
#[test]
fn every_real_mach_msg2_in_the_corpus_is_checked_for_a_nonzero_band() {
    let mut all_rows: Vec<(&str, Row)> = Vec::new();

    // hello_dyn: always available (a repo-owned fixture built by retrace-guest's build.rs).
    let hello_trace = record_dynamic_guest(retrace_guest::HELLO_DYN, &[], "hello-dyn-corpus");
    for row in walk_all_mach_msg2_landmarks(&hello_trace) { all_rows.push(("hello_dyn", row)); }

    // jq: Homebrew-only, per jq_e2e.rs's own convention -- skip LOUDLY, never silently.
    const JQ: &str = "/opt/homebrew/bin/jq";
    if std::path::Path::new(JQ).exists() {
        let trace = record_dynamic_guest(JQ, &["-n", "1+1"], "jq-corpus");
        for row in walk_all_mach_msg2_landmarks(&trace) { all_rows.push(("jq", row)); }
    } else {
        eprintln!("[M32 t1 corpus] SKIPPED jq: {JQ} not installed (`brew install jq`). The \
                    corpus walk below does NOT include jq's mach_msg2 calls -- this is a gap in \
                    coverage, not evidence that jq has none.");
    }

    // CPython: Homebrew-only, per cpython_e2e.rs's own convention -- same loud-skip posture.
    const CPYTHON: &str =
        "/opt/homebrew/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python";
    if std::path::Path::new(CPYTHON).exists() {
        let trace = record_dynamic_guest(CPYTHON, &["-c", "print(1)"], "cpython-corpus");
        for row in walk_all_mach_msg2_landmarks(&trace) { all_rows.push(("cpython", row)); }
    } else {
        eprintln!("[M32 t1 corpus] SKIPPED cpython: {CPYTHON} not found (expected a Homebrew \
                    python@3.14 install). The corpus walk below does NOT include CPython's \
                    mach_msg2 calls -- this is a gap in coverage, not evidence that it has none.");
    }

    assert!(!all_rows.is_empty(), "the corpus walk found ZERO mach_msg2 landmarks across every \
             guest it tried -- this measured nothing, which is the dead-channel trap the spec's \
             §4b exists to catch");

    // GOVERNED means `machmsg::route()` resolves this landmark to `Route::Forward` -- the only
    // route `forward_and_diff`, and so `SEED_MACH_MSG2`, ever runs for. An UNGOVERNED row (a
    // serviced address-space op, a stubbed MIG no-op, a refused message-queue send, ...) still
    // gets an `(avail, win, band)` computed and printed for context, but `forward_and_diff` never
    // runs for it in production, so its band -- zero or not -- cannot be something
    // `SEED_MACH_MSG2` changes. Both the conclusion below and this test's own assertions are
    // scoped to the GOVERNED subset for exactly that reason.
    let mut governed_max_avail = 0usize;
    let mut ungoverned_max_avail = 0usize;
    let mut any_governed_nonzero_band = false;
    for (guest, row) in &all_rows {
        eprintln!("[M32 t1 corpus:{guest}] msgh_id={} governed={} avail={} win={} band={} \
                    send_size={} rcv_size={}", row.msgh_id, row.governed, row.avail, row.win,
                   row.band, row.send_size, row.rcv_size);
        if !row.governed {
            ungoverned_max_avail = ungoverned_max_avail.max(row.avail);
            continue;
        }
        // Important B, folded into the corpus walk: the premise asserted against EVERY real
        // GOVERNED send in the corpus, not just the one hand-picked 3405 call.
        assert!(row.send_size <= SEND_SIZE_MAX,
            "[{guest}] msgh_id {} (governed) send_size {} exceeds the {SEND_SIZE_MAX} ceiling \
             crates/retrace-core/src/lib.rs:435 is supposed to enforce on every mach_msg2 call",
            row.msgh_id, row.send_size);
        // The hypothesis itself, checked on every real GOVERNED row regardless of whether its
        // band is nonzero.
        //
        // What makes it hold on a `band == 0` row is NOT the structural proof: steps 4/5 cover only
        // the band-EXISTS case, and these rows are by construction not that case. This comment used
        // to cite them anyway -- the milestone's own named failure class ("right conclusion,
        // unmeasured supporting fact", M20), committed by the fix round dispatched to correct it,
        // so it is corrected here rather than deleted. The real reason is an ABI/empirical fact
        // about the caller: the message buffer must have at least `send_size` bytes to the end of
        // its backing, or the kernel would read past the backing on a call the guest itself
        // constructed. With `band == 0` we have `win == avail`, and `avail` IS that
        // buffer-to-end-of-backing distance, so `win >= send_size` follows from the buffer being
        // well-formed -- which is exactly why this assertion is worth making: it is the check that
        // the real calls are well-formed in that sense, not a corollary of the proof.
        assert!(row.win >= row.send_size,
            "[{guest}] msgh_id {} (governed): win {} < send_size {} -- the hypothesis is REFUTED \
             on a REAL call", row.msgh_id, row.win, row.send_size);
        governed_max_avail = governed_max_avail.max(row.avail);
        if row.band > 0 { any_governed_nonzero_band = true; }
    }

    let governed_count = all_rows.iter().filter(|(_, r)| r.governed).count();
    eprintln!("[M32 t1 corpus] {} real mach_msg2 landmark(s) walked across {} guest(s): {} \
                GOVERNED (Route::Forward, subject to SEED_MACH_MSG2), {} not; maximum avail among \
                GOVERNED calls = {governed_max_avail} bytes (window_cap = {}); maximum avail among \
                UNGOVERNED calls = {ungoverned_max_avail} bytes (informational only -- \
                forward_and_diff never runs for these); any nonzero band among GOVERNED calls = \
                {any_governed_nonzero_band}",
        all_rows.len(),
        all_rows.iter().map(|(g, _)| *g).collect::<std::collections::HashSet<_>>().len(),
        governed_count, all_rows.len() - governed_count,
        retrace_box::PTR_WINDOW_CAP);

    // THE CONCLUSION, stated plainly rather than left for a reader to infer from the table above.
    // Scoped to GOVERNED calls only -- an ungoverned call's band, however large, is not a fact
    // about SEED_MACH_MSG2 (see the comment on the loop above; this repo's corpus does contain
    // exactly such a case: a refused message-queue send with `avail` in the millions of bytes and
    // a genuinely nonzero band, which would wrongly read as "NOT inert" if counted here -- the
    // same category error fix round 1's Critical 1 corrected for the single-call measurement).
    if any_governed_nonzero_band {
        eprintln!("[M32 t1 corpus] CONCLUSION: at least one real, GOVERNED mach_msg2 call in this \
                    corpus DOES produce a nonzero band -- SEED_MACH_MSG2 is NOT inert for this \
                    repo's fixtures.");
    } else {
        eprintln!("[M32 t1 corpus] CONCLUSION: NO real GOVERNED mach_msg2 call in this corpus \
                    produces a nonzero band (every governed avail stayed under window_cap = {} \
                    bytes; maximum governed avail observed was {governed_max_avail} bytes -- an \
                    UNGOVERNED call did reach {ungoverned_max_avail} bytes and a nonzero band, but \
                    forward_and_diff never runs for it, so it is not a fact about \
                    SEED_MACH_MSG2). SEED_MACH_MSG2 currently governs zero observable canary \
                    fills across every FORWARDED call in every guest fixture available to this \
                    session -- the seed is structurally sound (per the proof in machmsgband.rs) \
                    but, on this evidence, INERT in practice today. This is a reported finding, \
                    not a manufactured one: this test does not shrink window_cap to force a \
                    nonzero band.", retrace_box::PTR_WINDOW_CAP);
    }

    // THE TRIPWIRE (M32 finding 10). Everything above is `eprintln!` -- a green-by-construction
    // report of a number nobody re-checks. But M32 CLOSED on that number: spec §9 drops the
    // milestone's whole coverage deliverable because no governed call comes within 40 KiB of the
    // threshold, and M33's scoping rests on that. A measurement a milestone closed on owes an
    // assertion, or the close silently rots the first time a fixture contradicts it -- the same
    // discipline as M28's "prove the instrument can fire" and M29's "gate the channel that reports
    // it".
    //
    // Deliberately stricter than "no governed band was nonzero": a band needs `avail > win`, and
    // `win` saturates AT `window_cap`, so `avail == window_cap` exactly still yields band 0. Firing
    // on `avail >= window_cap` therefore warns one step BEFORE the finding is actually overturned,
    // which is the useful moment. It reds on an improvement (a new fixture that finally reaches a
    // deep governed call) -- and that red is the correct signal, not a false alarm, because what it
    // says is "the closed milestone's premise moved, go re-read §9 before trusting it".
    assert!(governed_max_avail < retrace_box::PTR_WINDOW_CAP,
        "a governed mach_msg2 call now carries a nonzero band (max governed avail \
         {governed_max_avail} >= window_cap {}) -- the inertness finding M32 closed on is \
         OVERTURNED; reopen spec §9 before trusting it",
        retrace_box::PTR_WINDOW_CAP);
}
