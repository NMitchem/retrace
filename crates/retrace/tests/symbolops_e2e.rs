// M20 headline gate: the debugger stops *demanding* bare hex.
//
// M19 taught it to print `in _child+0x30`; every operand was still a raw address, so the
// information the CLI had just printed was information it would not accept. This file proves the
// loop closed against a real recording. The unit tests in `retrace-core::symbols` and in
// `debug.rs` prove the reverse lookup and the classification rule against synthetic input; they
// cannot prove that `break _child` actually stops the guest at `_child`, and that is what is here.

// **Verified able to fail** (plan Task 4 Step 2). Stubbing `Symbols::addrs_of` to `Vec::new()`
// turns four of these five red — each with `DEBUG ERROR: no symbol "…"` and exit 5 where 0 was
// expected. Two details from that run are worth keeping:
//
//   * `a_stripped_guest_errors_cleanly_instead_of_guessing` went red **only because of its second
//     half**. Its first half asserts that a missing name errors — which a stub that resolves
//     *nothing* satisfies perfectly. The `break _main` must-succeed check is the guard against
//     that vacuity, and the stub run is what proved the guard load-bearing rather than decorative.
//   * `a_bad_name_fails_after_earlier_commands_have_run` stayed **green**, correctly. It pins the
//     S2 *ordering* change, not resolution; a debugger that resolves nothing still runs `where`
//     before failing. A test insensitive to the stub is only a problem when the stub breaks the
//     thing it claims to test, and this one does not.

mod util;
use retrace_trace::Event;

/// Pull the `pc=0x…` out of a `where` line, or the `hit 0x…` out of a hit line.
///
/// The `0x` strip is not cosmetic: `take_while(is_ascii_hexdigit)` over `0x1000…` stops dead at the
/// `x` and yields `0`, which would make the headline compare a real crash pc against zero.
fn hex_after(line: &str, marker: &str) -> Option<u64> {
    let rest = line.split(marker).nth(1)?.trim_start();
    let rest = rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X"))?;
    let tok: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    u64::from_str_radix(&tok, 16).ok()
}

/// The recorded fault pc, read straight out of the trace's terminal `Crash` landmark.
///
/// This is M20's **independent reference**. M1/M2 measured that `crashthread` faults at
/// `_child+0x30`, so the recording itself says where `_child` begins without consulting the symbol
/// table this milestone is testing. Deriving the expected address from `addrs_of` instead would
/// have made the headline assert that the code agrees with itself.
fn recorded_crash_pc(trace: &std::path::Path) -> u64 {
    let events = retrace_trace::Reader::open(trace).unwrap();
    events.iter().rev().find_map(|e| match e {
        Event::Crash { pc, .. } => Some(*pc), _ => None
    }).expect("crashthread's trace must end in a Crash landmark")
}

/// **THE M20 GATE.** `break _child` stops the guest at `_child`.
///
/// The assertion is on the **pc the guest stopped at**, never on the parse succeeding and never on
/// the echo text — honest-gate discipline, and here it has teeth. A no-op M20 that accepted the
/// token and armed nothing would run straight to the fault and print `guest crashed:`; one that
/// armed a *wrong* address would stop somewhere whose distance from the crash pc is not 0x30. Both
/// are excluded, and neither is excluded by asserting that the command was accepted.
#[test]
fn break_by_name_stops_at_that_function() {
    let (rec, trace) = util::record_dynamic(retrace_guest::CRASHTHREAD);
    assert_eq!(rec.code, 139, "the child must fault; stderr:\n{}", rec.stderr);

    let out = std::process::Command::new(util::bin())
        .args(["debug", trace.to_str().unwrap(), "--script", "break _child; continue; where"])
        .output().expect("spawn debug");
    assert_eq!(out.status.code(), Some(0), "stderr:\n{}", String::from_utf8_lossy(&out.stderr));
    let out = String::from_utf8(out.stdout).unwrap();

    let hit = out.lines().find(|l| l.starts_with("hit ") && !l.starts_with("hit watch"))
        .unwrap_or_else(|| panic!("no breakpoint hit — `break _child` armed nothing:\n{out}"));
    let stop_pc = hex_after(hit, "hit ").expect("a hex pc on the hit line");

    // The independent check: the recorded fault is `_child+0x30` (M1/M2), so the address the
    // breakpoint landed on must be exactly 0x30 below it.
    let crash_pc = recorded_crash_pc(&trace);
    assert_eq!(crash_pc - stop_pc, 0x30,
        "`break _child` must arm _child's ENTRY. Stopped at {stop_pc:#x}, but the recorded fault is \
         at {crash_pc:#x}, and M1/M2 measured the fault as _child+0x30 — so _child begins at \
         {:#x}.\nfull transcript:\n{out}", crash_pc - 0x30);

    // And the transcript names it, at offset 0 — a stop *inside* _child would read `_child+0x…`.
    assert!(hit.contains("_child") && !hit.contains("_child+"),
        "the hit must be at _child's entry, not inside it:\n{hit}");
}

/// `delete` resolves the SAME address `break` did, so a named breakpoint can be removed by name.
///
/// Asserting only that `delete _child` is *accepted* would pass even if it resolved elsewhere and
/// removed nothing. Running to completion afterwards is what proves the breakpoint is actually gone.
#[test]
fn delete_by_name_removes_the_breakpoint_it_set() {
    let (_rec, trace) = util::record_dynamic(retrace_guest::CRASHTHREAD);

    let out = std::process::Command::new(util::bin())
        .args(["debug", trace.to_str().unwrap(),
               "--script", "break _child; delete _child; continue"])
        .output().expect("spawn debug");
    assert_eq!(out.status.code(), Some(0), "stderr:\n{}", String::from_utf8_lossy(&out.stderr));
    let out = String::from_utf8(out.stdout).unwrap();

    assert!(!out.lines().any(|l| l.starts_with("hit ") && !l.starts_with("hit watch")),
        "the breakpoint was deleted, so nothing may stop at it:\n{out}");
    assert!(out.contains("guest crashed:"),
        "with no breakpoint armed the guest must run to its fault:\n{out}");
}

/// S2's ordering change, pinned rather than merely mentioned in the README.
///
/// Before M20 a bad operand rejected the whole script *before any command ran*, so this transcript
/// was empty. Resolution now happens at execution (S1), so `where` runs and echoes first. That is
/// the milestone's one behavioural regression; it is deliberate, it is the price of keeping the
/// parse pure, and a test says so out loud.
#[test]
fn a_bad_name_fails_after_earlier_commands_have_run() {
    let (_rec, trace) = util::record_dynamic(retrace_guest::CRASHTHREAD);

    let out = std::process::Command::new(util::bin())
        .args(["debug", trace.to_str().unwrap(), "--script", "where; break _no_such_symbol"])
        .output().expect("spawn debug");

    assert_eq!(out.status.code(), Some(5), "a failed resolution still exits 5, as it always did");
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(stdout.contains("pc=0x"),
        "the `where` BEFORE the bad operand must have run and printed — this is the S2 change:\n{stdout}");
    assert!(stderr.contains("no symbol") && stderr.contains("_no_such_symbol"),
        "and the failure must name the symbol it could not find:\n{stderr}");
}

/// The adversary: a stripped guest. `jq` ships with 7 defined text symbols (M3), so almost every
/// name a user could type is absent — and absence must be a clean error, never a panic and never a
/// fallback to interpreting the token as an address.
///
/// NOT a repo artifact: `jq` comes from Homebrew. When it is absent this announces the skip loudly
/// rather than passing quietly — a silent skip reads as a green it did not earn.
#[test]
fn a_stripped_guest_errors_cleanly_instead_of_guessing() {
    const JQ: &str = "/opt/homebrew/bin/jq";
    if !std::path::Path::new(JQ).exists() {
        eprintln!("SKIPPED a_stripped_guest_errors_cleanly_instead_of_guessing: {JQ} not installed \
                   (`brew install jq`). This gate did NOT run — it is not evidence of anything.");
        return;
    }
    let (rec, trace) = util::record_dynamic_args(JQ, &["-n", "1+1"]);
    assert_eq!(rec.code, 0, "jq must run; stderr:\n{}", rec.stderr);

    let out = std::process::Command::new(util::bin())
        .args(["debug", trace.to_str().unwrap(), "--script", "break _definitely_not_in_jq"])
        .output().expect("spawn debug");

    assert_eq!(out.status.code(), Some(5), "an unresolvable name is an error, not a panic");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no symbol"), "and it says so plainly:\n{stderr}");
    assert!(!stderr.contains("panicked"), "a debug session must never panic on a missing name:\n{stderr}");

    // jq DOES define _main (M3's 7 symbols include it), so the thin table still works — which is
    // what makes the assertion above about absence rather than about a broken reader.
    let ok = std::process::Command::new(util::bin())
        .args(["debug", trace.to_str().unwrap(), "--script", "break _main"])
        .output().expect("spawn debug");
    assert_eq!(ok.status.code(), Some(0),
        "jq's own _main must still resolve, or the test above proves nothing:\n{}",
        String::from_utf8_lossy(&ok.stderr));
}

/// Self-review 4: ambiguity, against a **real** duplicated name rather than only the synthetic ones
/// the unit tests build.
///
/// The name is discovered at runtime instead of hardcoded, because which symbols dyld duplicates is
/// a property of whatever `/usr/lib/dyld` this machine shipped — an OS update would turn a genuine
/// pass into a spurious red. **`-arch arm64e` is load-bearing**: `/usr/lib/dyld` is a universal
/// binary, and `nm` without it concatenates the x86_64 and arm64e slices so that nearly every symbol
/// looks duplicated. That mistake is what made S4's first draft wrong by a factor of ~235, and
/// repeating it here would make this test pass against names that are not actually ambiguous.
#[test]
fn a_genuinely_duplicated_dyld_name_is_refused_not_guessed() {
    let nm = std::process::Command::new("nm")
        .args(["-arch", "arm64e", "/usr/lib/dyld"]).output();
    let Ok(nm) = nm else {
        eprintln!("SKIPPED a_genuinely_duplicated_dyld_name_is_refused_not_guessed: `nm` unavailable. \
                   This gate did NOT run.");
        return;
    };
    let text = String::from_utf8_lossy(&nm.stdout);
    let mut names: Vec<&str> = text.lines()
        .filter(|l| { let f: Vec<&str> = l.split_whitespace().collect();
                      f.len() == 3 && (f[1] == "t" || f[1] == "T") })
        .map(|l| l.rsplit(' ').next().unwrap_or("")).collect();
    names.sort_unstable();
    let dup = names.windows(2).find(|w| w[0] == w[1]).map(|w| w[0].to_string());
    let Some(dup) = dup else {
        eprintln!("SKIPPED a_genuinely_duplicated_dyld_name_is_refused_not_guessed: this dyld's \
                   arm64e slice duplicates no text symbol name. This gate did NOT run.");
        return;
    };

    let (_rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    let out = std::process::Command::new(util::bin())
        .args(["debug", trace.to_str().unwrap(), "--script", &format!("break {dup}")])
        .output().expect("spawn debug");

    assert_eq!(out.status.code(), Some(5),
        "an ambiguous name must be REFUSED, not silently resolved to one of its addresses");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ambiguous"), "and say why:\n{stderr}");
    // The addresses are what make the error actionable — without them the user has no way forward.
    assert!(stderr.matches("0x").count() >= 2,
        "the error must LIST the candidate addresses, not merely report a count:\n{stderr}");
}
