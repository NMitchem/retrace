//! M58 final review, Group 1: `debug`'s file and line come from the build that was recorded, never from another
//! copy of it. Cargo dev builds of one source in two directories get the same `LC_UUID` (the linker leaves the debug
//! map out of the hash), and `atos` given such a binary answers from macOS's UUID-keyed symbol cache: it printed the
//! paths, and after an edit that kept the UUID the lines, of whichever copy was looked up first, and the UUID check
//! cannot see it. Each copy here is recorded in its own directory, one is looked up before the other, and every path
//! and line the later one prints must be its own. Each `debug` runs with a private `TMPDIR`, which must hold no
//! `retrace-dsym-` directory once it exits.
//!
//! Every run is bounded. The tests skip loudly (`util::announce`) when cargo does not run.
mod util;
use std::path::{Path, PathBuf};
use util::cargo::{cargo_in, cargo_path, copy_fixture, fresh_dir, remove_fixture_copy, saved_after_running, LEDGER_MANIFEST};

fn skip_no_cargo(name: &str) -> bool {
    if cargo_path().is_none() {
        util::announce(&format!("SKIPPED {name}: cargo did not run. This gate did NOT run."));
        return true;
    }
    false
}

fn json_lines(s: &str) -> Vec<serde_json::Value> {
    s.lines().map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}"))).collect()
}

/// One recorded copy: its directory, the unit-test trace and the test binary's `LC_UUID`.
struct Copy { dir: PathBuf, trace: String, uuid: Option<[u8; 16]> }

/// The ledger copied to a fresh directory, `edit`ed, and its unit tests recorded there by the README's recipe
/// (`--lib`: the unit-test binary alone), with `env` added.
fn record(tag: &str, env: &[(&str, &str)], edit: impl Fn(&Path)) -> Copy {
    let dir = copy_fixture(LEDGER_MANIFEST, tag);
    edit(&dir);
    let (code, out) = cargo_in(&dir, &["test", "--offline", "--lib"], env);
    assert_eq!(code, Some(101), "cargo's failing exit in {}:\n{out}", dir.display());
    let pairs = saved_after_running(&out);
    let (running, trace) = pairs.iter().find(|(r, _)| r.contains("unittests")).unwrap_or_else(|| panic!("{pairs:?}\n{out}"));
    // `Running unittests src/lib.rs (target/debug/deps/ledger-<hash>)`: the binary is in the last parentheses.
    let exe = running.rsplit_once('(').and_then(|(_, r)| r.strip_suffix(')')).unwrap_or_else(|| panic!("{running}"));
    let uuid = retrace_guest::file_uuid(&std::fs::read(dir.join(exe)).unwrap_or_else(|e| panic!("{exe}: {e}")));
    Copy { dir, trace: trace.clone(), uuid }
}

/// `retrace debug <trace> --json --script <script>`, with a private `TMPDIR` that must be empty of dSYM scratch
/// directories afterwards. Returns the last command's object.
fn debug(trace: &str, script: &str) -> serde_json::Value {
    let tmp = fresh_dir("source-tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let r = util::run_bounded(util::bin(), &["debug", trace, "--json", "--script", script], &[("TMPDIR", tmp.to_str().unwrap())], 300);
    let out = String::from_utf8_lossy(&r.stdout).into_owned();
    assert_eq!(r.code, Some(0), "debug {trace} {script:?}: {}\n{out}", r.stderr);
    let left: Vec<String> = std::fs::read_dir(&tmp).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("retrace-dsym-")).collect();
    assert!(left.is_empty(), "debug left its dSYM scratch behind in {}: {left:?}", tmp.display());
    let _ = std::fs::remove_dir_all(&tmp);
    let mut objs = json_lines(&out);
    for o in &objs { assert_eq!(o["ok"], true, "{o}"); }
    objs.pop().unwrap_or_else(|| panic!("no output:\n{out}"))
}

/// Every `file` string anywhere in `v`.
fn files(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::Object(m) => for (k, x) in m {
            if let Some(s) = x.as_str().filter(|_| k == "file") { out.push(s.to_string()); }
            files(x, out);
        },
        serde_json::Value::Array(a) => for x in a { files(x, out); },
        _ => {}
    }
}

/// Whether `path` lies under `dir`, as given or with symlinks resolved (`/tmp` is `/private/tmp`).
fn under(path: &str, dir: &Path) -> bool {
    Path::new(path).starts_with(dir) || dir.canonicalize().is_ok_and(|c| Path::new(path).starts_with(c))
}

/// The 1-based line of the first line of `file` at or after the one containing `after` that contains `what`.
fn line_in(file: &Path, after: &str, what: &str) -> u64 {
    let src = std::fs::read_to_string(file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    let lines: Vec<&str> = src.lines().collect();
    let t = lines.iter().position(|l| l.contains(after)).unwrap_or_else(|| panic!("{after:?} in {}", file.display()));
    1 + (t + lines[t..].iter().position(|l| l.contains(what)).unwrap_or_else(|| panic!("{what:?} in {}", file.display()))) as u64
}

/// Asserts every path of the copy's own sources in `o` lies under the copy, and that there is at least one.
fn paths_are_its_own(o: &serde_json::Value, c: &Copy) {
    let mut fs = Vec::new();
    files(o, &mut fs);
    let own: Vec<&String> = fs.iter().filter(|f| f.contains("/ledger/src/")).collect();
    assert!(!own.is_empty(), "no frame in the fixture's sources:\n{o}");
    for f in own { assert!(under(f, &c.dir), "{f} is not under this copy, {}:\n{o}", c.dir.display()); }
}

/// The frame named `function` in a `bt` object.
fn frame<'a>(o: &'a serde_json::Value, function: &str) -> &'a serde_json::Value {
    o["frames"].as_array().unwrap_or_else(|| panic!("no frames:\n{o}")).iter()
        .find(|f| f["function"] == function).unwrap_or_else(|| panic!("no frame of {function}:\n{o}"))
}

const PANIC_BT: &str = "break rust_panic; continue; bt";
const TEST_FN: &str = "ledger::tests::settling_four_hundred_leaves_the_rest";

/// Two copies of one build: B, looked up after A, names B's own files. A third copy with three comment lines above
/// the failing test has the assert frame on the shifted line. That copy's UUID differs from the others' (measured:
/// the shift moves the `assert_eq!`'s embedded `Location`), so it guards each session's own dSYM, not the cache.
#[test]
fn a_second_copy_of_one_build_names_its_own_files() {
    if skip_no_cargo("a_second_copy_of_one_build_names_its_own_files") { return; }
    let a = record("src-a", &[], |_| {});
    let b = record("src-b", &[], |_| {});
    let c = record("src-c", &[], |d| {
        let p = d.join("src/lib.rs");
        let s = std::fs::read_to_string(&p).unwrap();
        let at = "    #[test]\n    fn settling_four_hundred_leaves_the_rest";
        assert!(s.contains(at), "the failing test's header in {}", p.display());
        std::fs::write(&p, s.replace(at, &format!("    // one\n    // two\n    // three\n{at}"))).unwrap();
    });
    assert!(a.uuid.is_some() && a.uuid == b.uuid, "the premise: two copies of one build share an LC_UUID ({:?}, {:?})", a.uuid, b.uuid);
    for x in [&a, &b, &c] {
        let o = debug(&x.trace, PANIC_BT);
        paths_are_its_own(&o, x);
        let lib = x.dir.join("src/lib.rs");
        let f = frame(&o, TEST_FN);
        assert!(f["file"].as_str().is_some_and(|p| under(p, &x.dir) && p.ends_with("/ledger/src/lib.rs")), "{f}");
        let want = line_in(&lib, "fn settling_four_hundred_leaves_the_rest", "assert_eq!(l.balance");
        assert_eq!(f["line"].as_u64(), Some(want), "the assert's line in {}: {f}", lib.display());
    }
    for x in [a, b, c] { remove_fixture_copy(&x.dir); }
}

/// Two builds with the same UUID and different lines: with overflow checks off, `fees.rs` embeds no line, so three
/// comment lines at its top shift every line in it and leave the binary (and its UUID) as it was. E, looked up after
/// D, gets its own file and its own, shifted, line for `charge`.
#[test]
fn a_same_uuid_build_with_shifted_lines_gets_its_own_lines() {
    if skip_no_cargo("a_same_uuid_build_with_shifted_lines_gets_its_own_lines") { return; }
    let env = [("CARGO_PROFILE_TEST_OVERFLOW_CHECKS", "false"), ("CARGO_PROFILE_DEV_OVERFLOW_CHECKS", "false")];
    let d = record("src-d", &env, |_| {});
    let e = record("src-e", &env, |dir| {
        let p = dir.join("src/fees.rs");
        let s = std::fs::read_to_string(&p).unwrap();
        std::fs::write(&p, format!("// one\n// two\n// three\n{s}")).unwrap();
    });
    assert!(d.uuid.is_some() && d.uuid == e.uuid, "the premise: the shifted build keeps the LC_UUID ({:?}, {:?})", d.uuid, e.uuid);
    let script = "break ledger::fees::charge; continue; bt";
    for x in [&d, &e] {
        let o = debug(&x.trace, script);
        paths_are_its_own(&o, x);
        let fees = x.dir.join("src/fees.rs");
        let f0 = &o["frames"][0];
        assert_eq!(f0["function"], "ledger::fees::charge", "{o}");
        assert!(f0["file"].as_str().is_some_and(|p| under(p, &x.dir) && p.ends_with("/ledger/src/fees.rs")), "{f0}");
        assert_eq!(f0["line"].as_u64(), Some(line_in(&fees, "fn fee_for", "fn charge")), "charge's line in {}: {f0}", fees.display());
    }
    for x in [d, e] { remove_fixture_copy(&x.dir); }
}
