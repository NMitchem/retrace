//! M50 (spec §3f): every wall a newcomer can reach ends in one clean line, and every input retrace
//! cannot use is refused in one. Each test runs the CLI and asserts the documented exit code,
//! exactly one `retrace:` line naming the thing, the output the guest printed before a wall, and no
//! `panicked at` on either stream. Guest walls also replay, and the replay must end at the same
//! place with the incomplete-recording line (C2).
//!
//! An exit code is never the only evidence (CLAUDE.md): `record` passes the guest's own code
//! through, so every assertion pins the line too. Fixtures: `cf_walls` (one mode per wall),
//! `cf_rs_fsync`, and `tests/fixtures/cf_script.sh`.
mod util;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// The docs every wall line links to (spec §3b). Spelled out, not taken from retrace_arch, so a
/// wrong constant cannot agree with itself.
#[allow(dead_code)] // used from Task 2 on
const WALLS: &str = "https://github.com/NMitchem/retrace/blob/main/docs/walls.md";
/// Each run's bound. A run the bound kills fails its test rather than stalling the gate.
const BOUND: u64 = 300;

/// A scratch path unique to this process and call.
fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!("retrace-cf-{}-{}-{name}", std::process::id(),
                                      NEXT.fetch_add(1, Ordering::Relaxed)))
}

/// The signed CLI, bounded.
fn cli(args: &[&str]) -> util::Bounded { util::run_bounded(util::bin(), args, &[], BOUND) }

/// The lines retrace itself printed as its verdict. Every test asserts how many there are.
fn retrace_lines(stderr: &str) -> Vec<&str> { stderr.lines().filter(|l| l.starts_with("retrace:")).collect() }

/// No panic reached the user, on either stream, in any spelling Rust prints.
fn assert_no_panic(r: &util::Bounded, what: &str) {
    let out = String::from_utf8_lossy(&r.stdout);
    assert!(!r.stderr.contains("panicked at") && !out.contains("panicked at"),
        "{what}: a panic reached the user. stderr:\n{}", r.stderr);
}

/// Bad input (spec §3b, R2): exactly one `retrace:` line, containing each of `says`, and exit 2.
fn assert_bad_input(r: &util::Bounded, what: &str, says: &[&str]) {
    assert_no_panic(r, what);
    let lines = retrace_lines(&r.stderr);
    assert_eq!(lines.len(), 1, "{what}: exactly one retrace: line. stderr:\n{}", r.stderr);
    for s in says { assert!(lines[0].contains(s), "{what}: the line must say {s:?}: {}", lines[0]); }
    assert!(!lines[0].contains("internal error"), "{what}: bad input is not a bug: {}", lines[0]);
    assert_eq!(r.code, Some(2), "{what}: bad input exits 2. stderr:\n{}", r.stderr);
}

/// A script handed to record-dyn is refused by name; before M50 it failed `parse_macho`'s assert.
#[test]
fn a_shell_script_is_refused_as_not_a_mach_o() {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/cf_script.sh");
    let t = scratch("script.bin");
    let r = cli(&["record-dyn", script, "-o", t.to_str().unwrap()]);
    assert_bad_input(&r, "script", &[script, "is a script, not a Mach-O executable"]);
    assert!(!t.exists(), "nothing is recorded for bad input");
}

/// An empty file panicked slicing its magic before M50.
#[test]
fn an_empty_file_is_refused_as_not_a_mach_o() {
    let empty = scratch("empty");
    std::fs::write(&empty, b"").unwrap();
    let t = scratch("empty.bin");
    let r = cli(&["record-dyn", empty.to_str().unwrap(), "-o", t.to_str().unwrap()]);
    assert_bad_input(&r, "empty", &["is empty or too short to be a Mach-O executable"]);
    let _ = std::fs::remove_file(&empty);
}

/// A thin x86_64 binary passed the magic check before M50 and was loaded as if it were arm64.
#[test]
fn a_thin_x86_64_binary_is_refused_by_name() {
    let exe = scratch("x86_64");
    let built = std::process::Command::new("clang")
        .args(["-arch", "x86_64", "-o", exe.to_str().unwrap(), retrace_guest::CF_WALLS_SRC]).output();
    if !built.as_ref().is_ok_and(|o| o.status.success()) {
        util::announce("SKIPPED a_thin_x86_64_binary_is_refused_by_name: `clang -arch x86_64` did not \
            build cf_walls.c (no x86_64 SDK support), so this row did NOT run and is not evidence of anything.");
        return;
    }
    let t = scratch("x86_64.bin");
    let r = cli(&["record-dyn", exe.to_str().unwrap(), "-o", t.to_str().unwrap(), "--", "fsync"]);
    assert_bad_input(&r, "x86_64", &["is an x86_64 program, and retrace runs arm64 programs only"]);
    let _ = std::fs::remove_file(&exe);
}

/// A missing operand is a usage error with exit 2 on every subcommand (R2). Before M50 `record-dyn`
/// alone indexed past its arguments and panicked with exit 101.
#[test]
fn missing_arguments_are_a_usage_error_not_a_panic() {
    let cases: [&[&str]; 7] = [&["record-dyn"], &["record-dyn", retrace_guest::CF_WALLS],
        &["record-dyn", retrace_guest::CF_WALLS, "-o"], &["record"], &["replay"], &["frobnicate"], &[]];
    for args in cases {
        assert_bad_input(&cli(args), &format!("{args:?}"), &["usage: retrace"]);
    }
}

/// Without the hypervisor entitlement `hv_vm_create` is denied. That is one line naming the
/// entitlement and the command that grants it, not a panic in `Vm::create().expect`.
#[test]
fn a_retrace_without_the_hypervisor_entitlement_says_how_to_sign_it() {
    // Signed ad hoc WITHOUT the entitlement: arm64 runs only signed code, so unsigned is no option,
    // and util::bin()'s copy carries the entitlement.
    let copy = scratch("retrace-unentitled");
    std::fs::copy(env!("CARGO_BIN_EXE_retrace"), &copy).unwrap();
    let signed = std::process::Command::new("codesign").args(["-s", "-", "-f", copy.to_str().unwrap()])
        .output().unwrap();
    assert!(signed.status.success(), "codesign: {}", String::from_utf8_lossy(&signed.stderr));
    let t = scratch("unentitled.bin");
    let r = util::run_bounded(copy.to_str().unwrap(),
        &["record-dyn", retrace_guest::CF_WALLS, "-o", t.to_str().unwrap(), "--", "fsync"], &[], BOUND);
    assert_bad_input(&r, "unentitled", &["cannot create a VM", "com.apple.security.hypervisor", "codesign"]);
    assert!(!t.exists(), "the check comes before the trace is created");
    let _ = std::fs::remove_file(&copy);
}

/// The bug banner (spec §3b): a panic that is not a wall prints three lines, naming the bug, the
/// issues page and the panic's own message and location, then exits 70, never 101 (R3). The trigger
/// is `RETRACE_DEBUG_PANIC`, a seam that exists only in debug builds (§3f), so no program reaches it.
#[test]
fn a_retrace_bug_prints_the_banner_and_exits_70() {
    let t = scratch("bug.bin");
    let r = util::run_bounded(util::bin(),
        &["record-dyn", retrace_guest::CF_WALLS, "-o", t.to_str().unwrap(), "--", "fsync"],
        &[("RETRACE_DEBUG_PANIC", "1")], BOUND);
    assert_no_panic(&r, "banner");
    let lines: Vec<&str> = r.stderr.lines().collect();
    let at = lines.iter()
        .position(|l| *l == "retrace: internal error - this is a bug in retrace, not in your program.")
        .unwrap_or_else(|| panic!("no banner. stderr:\n{}", r.stderr));
    assert_eq!(lines.get(at + 1).copied(),
        Some("  please report it at https://github.com/NMitchem/retrace/issues and include the next line:"));
    let msg = lines.get(at + 2).copied().unwrap_or("");
    assert!(msg.starts_with("  RETRACE_DEBUG_PANIC is set: a deliberate internal error")
            && msg.contains(" (at crates/retrace/src/main.rs:"), "the panic's message and location: {msg}");
    assert_eq!(retrace_lines(&r.stderr).len(), 1, "the banner's first line is the only retrace: line");
    assert_eq!(r.code, Some(70), "a retrace bug exits 70 (R3). stderr:\n{}", r.stderr);
}

/// A guest path that does not exist panicked in `fs::read(..).expect` before M50.
#[test]
fn an_unreadable_guest_path_is_one_clean_line() {
    let t = scratch("missing.bin");
    let r = cli(&["record-dyn", "/no/such/program", "-o", t.to_str().unwrap()]);
    assert_bad_input(&r, "missing guest", &["cannot read /no/such/program"]);
}
