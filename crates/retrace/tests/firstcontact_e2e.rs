//! M49 Task 1 (spec §3b): the first-contact corpus, tier 1 — the programs a newcomer tries first,
//! each run natively once, recorded once and replayed twice, and each one's outcome pinned.
//!
//! A row's outcome is `Pass` when all of these hold (spec §3b):
//! - the recording exits as native did and prints what native printed (through `canonical`, which
//!   masks libtest's run-dependent figures, Ruling A1);
//! - both replays reproduce the recording's exit status and stdout byte for byte;
//! - when native dies by a signal, the trace's terminal event is of the right kind and carries that
//!   signal (spec §3b, CLAUDE.md's distinction): `Event::Signal` only for SIGABRT (6, raised by
//!   `abort`), `Event::Crash` (through `signal_of_esr`) for every other signal, which is a hardware
//!   fault. The wrong kind is a named `MISMATCH terminal kind`, never a pass.
//!
//! Otherwise the outcome is `Refused` (the recording ended at a clean wall line and both replays end
//! at the same place, M50), `Bug` (the bug banner's message), or the first difference as one line
//! (`judge`): `DIVERGENCE` and a replay's detail, `TIMEOUT`, or `MISMATCH` and what differed, with
//! its addresses masked (`mask_addresses`), or `Toolchain`. `Expect::Wall` pins a prefix of that
//! line, an in-scope row's clean wall line included; `Expect::Refused` is for an out-of-scope row
//! (charter §6).
//!
//! The gate asserts every row's outcome equals its pinned outcome (R10): a wall that is fixed, or
//! that moves, fails here until the task that moved it edits its row. One `#[test]` runs the whole
//! table (Ruling A2): the `FIRSTCONTACT` tally needs every row's outcome in one process, and the
//! failure names every row that moved, with what it expected and what it got.
//!
//! Swift rows build only where `swiftc` runs (R11); elsewhere each announces `SKIPPED (no swiftc)`
//! and counts under `toolchain=`. Every trace is deleted after its row: most rows are walls by
//! design, so a kept trace would be the common case, not a diagnostic one.
mod util;
use retrace_trace::Event;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Expect {
    Pass,
    Refused(&'static str),
    Wall(&'static str),
    Toolchain,
}

struct Row { name: &'static str, bin: &'static str, args: &'static [&'static str], expect: Expect }

/// The outcome table (spec §3b), pinned from t0 M1 (Task 1's addendum). A row flips only in the
/// task that changes it (R10): M49's F rows in its Task 3 and S rows in its Task 5; M51's fc_assert
/// and fc_cpp_terminate in its Task 1, the brk rows in Task 3, fc_env in Task 5.
const ROWS: &[Row] = &[
    // C (the hello_dyn recipe).
    Row { name: "fc_null_load", bin: retrace_guest::FC_NULL_LOAD, args: &[],
          expect: Expect::Pass },
    Row { name: "fc_null_store", bin: retrace_guest::FC_NULL_STORE, args: &[],
          expect: Expect::Pass },
    Row { name: "fc_null_call", bin: retrace_guest::FC_NULL_CALL, args: &[],
          expect: Expect::Pass },
    Row { name: "fc_munmapped", bin: retrace_guest::FC_MUNMAPPED, args: &[],
          expect: Expect::Pass },
    // Before Task 3 any wall, by design: the store rewrote retrace's L2 table, and whether that
    // was silent (`MISMATCH record exit 0, native 139`) or loud (`RECORD ERROR: … instruction
    // abort … far/ipa= 0x4400`) depended on the TLB; 17 and 3 of 20 at plan time, 13 and 7 of 20
    // at t0 (t0 M1). Task 3's A-narrow (F3) unmaps the table pages at stage 1, so the store is
    // the crash native takes, and `Pass` is the one outcome only A-narrow can produce.
    Row { name: "fc_low_store", bin: retrace_guest::FC_LOW_STORE, args: &[],
          expect: Expect::Pass },
    Row { name: "fc_stack_overflow", bin: retrace_guest::FC_STACK_OVERFLOW, args: &[],
          expect: Expect::Pass },
    Row { name: "fc_uaf", bin: retrace_guest::FC_UAF, args: &[], expect: Expect::Pass },
    Row { name: "fc_assert", bin: retrace_guest::FC_ASSERT, args: &[],
          expect: Expect::Pass },
    Row { name: "fc_sleep", bin: retrace_guest::FC_SLEEP, args: &[],
          expect: Expect::Pass },
    Row { name: "fc_usleep", bin: retrace_guest::FC_USLEEP, args: &[],
          expect: Expect::Pass },
    Row { name: "fc_nanosleep", bin: retrace_guest::FC_NANOSLEEP, args: &[],
          expect: Expect::Pass },
    Row { name: "fc_mutex", bin: retrace_guest::FC_MUTEX, args: &[],
          expect: Expect::Wall("retrace: unsupported: M48: psynch psynch_mutexwait (301) is not modelled") },
    Row { name: "fc_fileio", bin: retrace_guest::FC_FILEIO, args: &[], expect: Expect::Pass },
    Row { name: "fc_env", bin: retrace_guest::FC_ENV, args: &["one", "two words"],
          expect: Expect::Wall("MISMATCH record stdout at line 4: native \"FC_ENV_PROBE=firstcontact\\n\", record \"FC_ENV_PROBE=(unset)\\n\"") },
    // C++ (clang++ -std=c++17).
    Row { name: "fc_cpp_hello", bin: retrace_guest::FC_CPP_HELLO, args: &[], expect: Expect::Pass },
    Row { name: "fc_cpp_throw", bin: retrace_guest::FC_CPP_THROW, args: &[], expect: Expect::Pass },
    Row { name: "fc_cpp_terminate", bin: retrace_guest::FC_CPP_TERMINATE, args: &[],
          expect: Expect::Pass },
    Row { name: "fc_cpp_thread", bin: retrace_guest::FC_CPP_THREAD, args: &[], expect: Expect::Pass },
    Row { name: "fc_cpp_sleep", bin: retrace_guest::FC_CPP_SLEEP, args: &[],
          expect: Expect::Pass },
    // Rust (cargo's own rustc).
    Row { name: "fc_rs_null", bin: retrace_guest::FC_RS_NULL, args: &[],
          expect: Expect::Pass },
    Row { name: "fc_rs_sleep", bin: retrace_guest::FC_RS_SLEEP, args: &[],
          expect: Expect::Pass },
    Row { name: "fc_rs_tests", bin: retrace_guest::FC_RS_TESTS, args: &[],
          expect: Expect::Wall("retrace: unsupported: semaphore_timedwait_trap (mach trap -38)") },
    Row { name: "fc_rs_tests_fail", bin: retrace_guest::FC_RS_TESTS_FAIL, args: &[],
          expect: Expect::Wall("retrace: unsupported: semaphore_timedwait_trap (mach trap -38)") },
    // Swift (swiftc, only where it runs: R11).
    Row { name: "fc_sw_hello", bin: retrace_guest::FC_SW_HELLO, args: &[], expect: Expect::Pass },
    Row { name: "fc_sw_throw", bin: retrace_guest::FC_SW_THROW, args: &[], expect: Expect::Pass },
    Row { name: "fc_sw_oob", bin: retrace_guest::FC_SW_OOB, args: &[],
          expect: Expect::Wall("retrace: unsupported: brk #0x1 at pc <addr> (exception class 0x3c, instruction 0x") },
    Row { name: "fc_sw_unwrap", bin: retrace_guest::FC_SW_UNWRAP, args: &[],
          expect: Expect::Wall("retrace: unsupported: brk #0x1 at pc <addr> (exception class 0x3c, instruction 0x") },
    Row { name: "fc_sw_sleep", bin: retrace_guest::FC_SW_SLEEP, args: &[],
          expect: Expect::Pass },
    Row { name: "fc_sw_gcd", bin: retrace_guest::FC_SW_GCD, args: &[],
          expect: Expect::Refused("retrace: unsupported: kevent_id (syscall 375)") },
    Row { name: "fc_sw_async", bin: retrace_guest::FC_SW_ASYNC, args: &[],
          expect: Expect::Refused("retrace: unsupported: M18 Stage 2b: unmeasured workq_kernreturn opcode 0x30") },
];

/// Each run's bound. A phase that hangs fails its row rather than stalling the gate; t0 M1 measured
/// the slowest phase (6 s) well under it.
const PHASE_SECS: u64 = 300;
/// Set for the native run and the recorder alike, so `fc_env` shows what a guest's `getenv` sees.
const ENV: &[(&str, &str)] = &[("FC_ENV_PROBE", "firstcontact")];

/// What a row did: `Pass`, its first difference as one line, or nothing to run (a Swift row where
/// swiftc does not run).
enum Outcome { Pass, Refused(String), Wall(String), Bug(String), Toolchain }

fn describe(o: &Outcome) -> &str {
    match o {
        Outcome::Pass => "Pass",
        Outcome::Refused(l) | Outcome::Wall(l) | Outcome::Bug(l) => l,
        Outcome::Toolchain => "Toolchain",
    }
}

/// What a row is expected to do here: its pinned outcome, or `Toolchain` for a Swift row whose
/// fixture build.rs could not build (R11).
fn expected(row: &Row) -> Expect {
    if row.name.starts_with("fc_sw_") && !Path::new(row.bin).exists() { Expect::Toolchain } else { row.expect }
}

/// libtest's output, made comparable across runs (Ruling A1). libtest prints each result line as its
/// test completes on its own thread pool, so the lines after a `running N tests` header come in a
/// run-dependent order; its summary carries a wall-clock `finished in N.NNs`; and a captured panic
/// line carries the thread's id. Each is masked or sorted here; nothing else in any fixture's output
/// matches these shapes. Applied to native's and the recording's stdout, never to a replay's.
fn canonical(out: &[u8]) -> String {
    let text = String::from_utf8_lossy(out);
    let mut lines: Vec<String> = text.split_inclusive('\n').map(mask).collect();
    let mut i = 0;
    while i < lines.len() {
        let h = lines[i].trim_end();
        let header = h.starts_with("running ") && (h.ends_with(" test") || h.ends_with(" tests"));
        i += 1;
        if header {
            let start = i;
            while i < lines.len() && lines[i].starts_with("test ") { i += 1; }
            lines[start..i].sort();
        }
    }
    lines.concat()
}

fn mask(line: &str) -> String {
    let mut l = line.to_owned();
    if let Some(at) = l.find("finished in ") {
        let start = at + "finished in ".len();
        if let Some(len) = l[start..].find('s') { l.replace_range(start..start + len, "<T>"); }
    }
    if l.starts_with("thread '") {
        if let (Some(open), Some(close)) = (l.find("' ("), l.find(") panicked at ")) {
            if open < close { l.replace_range(open + 3..close, "<TID>"); }
        }
    }
    l
}

/// An outcome line with every address masked to `<addr>`: a hex number of 9 to 12 digits, the span
/// of a 47-bit guest VA at or above 4 GiB (Ruling A3). Shared-cache addresses move with every macOS
/// update and heap addresses with every allocator change, and a pinned outcome must move only when
/// retrace does. Shorter numbers (ESR fields, `dest 0x903`, `far/ipa=0x10`) and 16-digit ones (a
/// negative trap number) stay.
fn mask_addresses(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(i) = rest.find("0x") {
        let digits = rest[i + 2..].bytes().take_while(u8::is_ascii_hexdigit).count();
        out.push_str(&rest[..i]);
        out.push_str(if (9..=12).contains(&digits) { "<addr>" } else { &rest[i..i + 2 + digits] });
        rest = &rest[i + 2 + digits..];
    }
    out.push_str(rest);
    out
}

/// The first line where `b` differs from `a`, 1-based, each side Debug-quoted (cut to 100 chars) or
/// `<eof>`; `None` when they are equal.
fn first_difference(a: &str, b: &str, la: &str, lb: &str) -> Option<String> {
    if a == b { return None; }
    let (x, y): (Vec<&str>, Vec<&str>) = (a.split_inclusive('\n').collect(), b.split_inclusive('\n').collect());
    let show = |l: Option<&&str>| l.map_or("<eof>".to_owned(), |l| format!("{:?}", l.chars().take(100).collect::<String>()));
    let n = (0..x.len().max(y.len())).find(|&i| x.get(i) != y.get(i)).unwrap_or(0);
    Some(format!("at line {}: {la} {}, {lb} {}", n + 1, show(x.get(n)), show(y.get(n))))
}

/// The message of a retrace bug: the line after the banner's "please report" line (M50 §3b), which
/// is the panic's own message and location. Since M50's hook the banner replaces Rust's
/// `panicked at` line, which `panic_message` still reads for a panic the hook never saw.
fn bug_message(stderr: &str) -> Option<String> {
    let mut lines = stderr.lines();
    lines.find(|l| l.starts_with("retrace: internal error"))?;
    lines.next()?;
    Some(lines.next().unwrap_or("").trim().to_owned())
}

/// The message of a recorder or replayer panic: the line after `panicked at crates/`, which is
/// anchored on retrace's own sources so a Rust guest's own panic is never taken for one (the
/// apple-sweep's anchor).
fn panic_message(stderr: &str) -> Option<String> {
    let mut lines = stderr.lines();
    lines.find(|l| l.contains("panicked at crates/"))?;
    Some(lines.next().unwrap_or("").to_owned())
}

/// `None` when the recording's terminal event carries native's signal `want`, else what it carries.
fn terminal_mismatch(trace: &Path, want: i32) -> Option<String> {
    // Only abort() raises (SIGABRT); every other fatal signal a fixture dies of is a hardware fault.
    let raised = want == 6;
    let (events, torn) = retrace_trace::Reader::open_checked(trace).expect("read the recording");
    let got = match events.iter().rev().find(|e| !matches!(e, Event::Snapshot { .. })) {
        _ if torn => "a torn trace".to_owned(),
        Some(Event::Crash { esr, .. }) => {
            let sig = retrace_arch::signal_of_esr(*esr).0;
            if raised { return Some(format!("MISMATCH terminal kind: native {want} (expected Signal), trace Crash {sig} esr={esr:#x}")); }
            if sig as i32 == want { return None; }
            format!("Crash {sig} esr={esr:#x}")
        }
        Some(Event::Signal { sig, .. }) => {
            if !raised { return Some(format!("MISMATCH terminal kind: native {want} (expected Crash), trace Signal {sig}")); }
            if *sig as i32 == want { return None; }
            format!("Signal {sig}")
        }
        Some(Event::Exit { code, .. }) => format!("Exit {code}"),
        _ => "no terminal event".to_owned(),
    };
    Some(format!("MISMATCH terminal signal: native {want}, trace {got}"))
}

/// A recording that ended at a clean wall line (spec §3e). It must exit 4 with exactly one such line,
/// and both replays must end at the same place: the incomplete-recording line (C2), exit 4, and the
/// recording's stdout. The first thing that fails is the row's outcome.
fn refused(rec: &util::Bounded, rcode: i32, line: &str, trace: &str) -> Outcome {
    let wall = Outcome::Wall;
    let n = rec.stderr.lines().filter(|l| l.starts_with("retrace: unsupported: ")).count();
    if n != 1 { return wall(format!("MISMATCH record printed {n} wall lines")); }
    if rcode != 4 { return wall(format!("MISMATCH record exit {rcode} after a wall line, not 4")); }
    for i in 1..=2 {
        let rp = util::run_bounded(util::bin(), &["replay", trace], &[], PHASE_SECS);
        let Some(pcode) = rp.code else { return wall(format!("TIMEOUT replay {i}")) };
        if let Some(m) = bug_message(&rp.stderr).or_else(|| panic_message(&rp.stderr)) {
            return Outcome::Bug(format!("BUG replay {i}: {m}"));
        }
        if let Some(l) = rp.stderr.lines().find(|l| l.starts_with("DIVERGENCE at landmark ")) {
            return wall(format!("DIVERGENCE replay {i}: {}", l.split_once(": ").map_or(l, |(_, d)| d)));
        }
        if pcode != 4 || !rp.stderr.lines().any(|l| l.starts_with("retrace: incomplete recording: ")) {
            return wall(format!("MISMATCH replay {i} of a walled recording: exit {pcode} without the incomplete-recording line"));
        }
        if rp.stdout != rec.stdout { return wall(format!("MISMATCH replay {i} stdout of a walled recording")); }
    }
    Outcome::Refused(line.to_owned())
}

/// The first difference between native's run and the recording's, then the two replays' (spec §3b).
fn judge(ncode: i32, nout: &[u8], rec: &util::Bounded, trace: &str) -> Outcome {
    let wall = Outcome::Wall;
    let Some(rcode) = rec.code else { return wall("TIMEOUT record".into()) };
    if let Some(m) = bug_message(&rec.stderr).or_else(|| panic_message(&rec.stderr)) {
        return Outcome::Bug(format!("BUG record: {m}"));
    }
    if let Some(line) = rec.stderr.lines().find(|l| l.starts_with("retrace: unsupported: ")) {
        return refused(rec, rcode, line, trace);
    }
    if rcode != ncode { return wall(format!("MISMATCH record exit {rcode}, native {ncode}")); }
    if let Some(d) = first_difference(&canonical(nout), &canonical(&rec.stdout), "native", "record") {
        return wall(format!("MISMATCH record stdout {d}"));
    }
    if ncode >= 128 {
        if let Some(m) = terminal_mismatch(Path::new(trace), ncode - 128) { return wall(m); }
    }
    let rout = String::from_utf8_lossy(&rec.stdout);
    for i in 1..=2 {
        let rp = util::run_bounded(util::bin(), &["replay", trace], &[], PHASE_SECS);
        let Some(pcode) = rp.code else { return wall(format!("TIMEOUT replay {i}")) };
        if let Some(l) = rp.stderr.lines().find(|l| l.starts_with("DIVERGENCE at landmark ")) {
            return wall(format!("DIVERGENCE replay {i}: {}", l.split_once(": ").map_or(l, |(_, d)| d)));
        }
        if let Some(m) = bug_message(&rp.stderr).or_else(|| panic_message(&rp.stderr)) {
            return Outcome::Bug(format!("BUG replay {i}: {m}"));
        }
        if pcode != rcode { return wall(format!("MISMATCH replay {i} exit {pcode}, record {rcode}")); }
        if let Some(d) = first_difference(&rout, &String::from_utf8_lossy(&rp.stdout), "record", "replay") {
            return wall(format!("MISMATCH replay {i} stdout {d}"));
        }
    }
    Outcome::Pass
}

/// Native once, record once, replay twice (spec §3b).
fn measure(row: &Row) -> Outcome {
    if !Path::new(row.bin).exists() {
        assert!(row.name.starts_with("fc_sw_"), "{}: {} was not built, and build.rs builds every non-Swift fixture", row.name, row.bin);
        return Outcome::Toolchain;
    }
    let native = util::run_bounded(row.bin, row.args, ENV, PHASE_SECS);
    let Some(ncode) = native.code else { return Outcome::Wall("TIMEOUT native".into()) };
    let trace = std::env::temp_dir().join(format!("retrace-fc-{}-{}.bin", row.name, std::process::id()));
    let t = trace.to_str().unwrap();
    let mut argv = vec!["record-dyn", row.bin, "-o", t];
    if !row.args.is_empty() { argv.push("--"); argv.extend_from_slice(row.args); }
    let rec = util::run_bounded(util::bin(), &argv, ENV, PHASE_SECS);
    let got = match judge(ncode, &native.stdout, &rec, t) {
        Outcome::Wall(l) => Outcome::Wall(mask_addresses(&l)),
        Outcome::Refused(l) => Outcome::Refused(mask_addresses(&l)),
        Outcome::Bug(l) => Outcome::Bug(mask_addresses(&l)),
        o => o,
    };
    let _ = std::fs::remove_file(&trace);
    got
}

#[test]
fn every_corpus_row_meets_its_pinned_outcome() {
    let (mut pass, mut refused, mut wall, mut toolchain) = (0, 0, 0, 0);
    let mut moved = Vec::new();
    for row in ROWS {
        // A harness panic (a spawn that failed, an unreadable trace) is that row's outcome, so it is
        // named like any other.
        let got = std::panic::catch_unwind(|| measure(row)).unwrap_or_else(|e| {
            let m = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()));
            Outcome::Wall(format!("HARNESS PANIC: {}", m.unwrap_or_default()))
        });
        let want = expected(row);
        let ok = match (&got, want) {
            (Outcome::Pass, Expect::Pass) | (Outcome::Toolchain, Expect::Toolchain) => true,
            (Outcome::Refused(line), Expect::Refused(prefix)) => line.starts_with(prefix),
            // An in-scope wall pins its first difference, which since M50 may be a clean wall line.
            (Outcome::Wall(line) | Outcome::Refused(line), Expect::Wall(prefix)) => line.starts_with(prefix),
            _ => false,
        };
        match (&got, want) {
            (Outcome::Pass, _) => pass += 1,
            // Charter §6: `refused` counts the out-of-scope rows that refused cleanly. An in-scope
            // row's clean wall line is still a wall, which is what L1 measures.
            (Outcome::Refused(_), Expect::Refused(_)) => refused += 1,
            (Outcome::Toolchain, _) => {
                toolchain += 1;
                util::announce(&format!("SKIPPED {} (no swiftc): {} was not built, so this row did NOT run \
                    and is not evidence of anything.", row.name, row.bin));
            }
            _ => wall += 1,
        }
        util::announce(&format!("FIRSTCONTACT row {} {}: {}", row.name, if ok { "as pinned" } else { "MOVED" }, describe(&got)));
        if !ok { moved.push(format!("{}: expected {:?}, got {}", row.name, want, describe(&got))); }
    }
    util::announce(&format!("FIRSTCONTACT pass={pass} refused={refused} wall={wall} toolchain={toolchain}"));
    util::announce(&format!("LAUNCH t1 pass={pass} refused={refused} wall={wall} toolchain={toolchain}"));
    assert!(moved.is_empty(), "{} row(s) moved off their pinned outcome; the task that moved each one edits its \
        row (R10):\n{}", moved.len(), moved.join("\n"));
}

#[test]
fn the_table_has_one_row_per_corpus_fixture() {
    // A fixture added without a row would go unmeasured, and a row naming another row's binary
    // would measure it twice.
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../retrace-guest");
    let mut sources: Vec<String> = ["c", "cpp", "rs", "swift"].iter()
        .flat_map(|d| std::fs::read_dir(format!("{dir}/{d}")).unwrap())
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|f| f.starts_with("fc_"))
        .map(|f| f.rsplit_once('.').unwrap().0.to_owned())
        .collect();
    sources.sort();
    let mut names: Vec<&str> = ROWS.iter().map(|r| r.name).collect();
    names.sort();
    assert_eq!(names, sources, "every fc_* source has exactly one row, and every row a source");
    for r in ROWS {
        assert!(r.bin.ends_with(&format!("/{}", r.name)), "{}'s binary is {}", r.name, r.bin);
    }
}

#[test]
fn canonical_output_hides_only_libtests_run_dependent_figures() {
    // No row reaches the comparison through libtest yet: both harnesses stop at record (t0 M1). So
    // this pins `canonical` on what native printed at plan time. Two runs of `fc_rs_tests` differ in
    // their result lines' order and their `finished in`, and two of `fc_rs_tests_fail` in the panic
    // line's thread id.
    let tests = |order: [&str; 4], secs: &str| format!(
        "\nrunning 4 tests\n{}\ntest result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; \
         finished in {secs}s\n\n", order.map(|t| format!("test tests::{t}\n")).concat());
    let a = tests(["adds ... ok", "sorts ... ok", "joins ... ok", "indexes_past_the_end - should panic ... ok"], "0.00");
    let b = tests(["joins ... ok", "indexes_past_the_end - should panic ... ok", "adds ... ok", "sorts ... ok"], "0.01");
    assert_ne!(a, b);
    assert_eq!(canonical(a.as_bytes()), canonical(b.as_bytes()), "the order and the time are run-dependent");
    let fail = |tid: &str, left: u32| format!(
        "\nrunning 1 test\ntest tests::two_plus_two_is_five ... FAILED\n\nfailures:\n\n---- tests::two_plus_two_is_five \
         stdout ----\n\nthread 'tests::two_plus_two_is_five' ({tid}) panicked at fc_rs_tests_fail.rs:9:9:\n\
         assertion `left == right` failed: a newcomer's failing test\n  left: {left}\n right: 5\n");
    assert_eq!(canonical(fail("30348290", 4).as_bytes()), canonical(fail("30348293", 4).as_bytes()), "the thread id is");
    // Everything else still counts: a different value, or a different verdict.
    assert_ne!(canonical(fail("30348290", 4).as_bytes()), canonical(fail("30348290", 3).as_bytes()));
    let c = tests(["adds ... FAILED", "sorts ... ok", "joins ... ok", "indexes_past_the_end - should panic ... ok"], "0.00");
    assert_ne!(canonical(a.as_bytes()), canonical(c.as_bytes()));
}
