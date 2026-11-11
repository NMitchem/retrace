//! M58 B3: file and line for a pc in the program's own code, through macOS's `atos`. The image is trusted only when
//! `image::resolve_image` found one whose `LC_UUID` is the recorded one.
//!
//! M58 final review: `atos` reads a dSYM, never the image itself. Cargo dev builds of one source in two directories
//! get the same `LC_UUID` (the linker leaves the debug map out of its hash), and `atos` given a binary that relies on
//! a debug map answers from macOS's UUID-keyed symbol cache: it printed the path, and after an edit that kept the
//! UUID the line, of whichever copy had been looked up first (`source_e2e`), and the UUID check cannot see that. So
//! each epoch's map makes its own dSYM once, with `dsymutil <image> -o <private temp dir>/<name>.dSYM`, which reads
//! that image's own debug map, and removes it on drop. A dSYM already beside the image (`<image>.dSYM`, which
//! `clang -g` writes when it compiles and links in one step, after deleting the objects the debug map names) is read
//! in place when its `LC_UUID` is the recorded one. `atos` is given the slice's `-arch`.
//!
//! This is the one host-derived field of `debug`'s output (it comes from the host's file and tools, not from
//! the recording), and it is never written into a trace. Every failure degrades to "no file, no line" plus one
//! note saying why; nothing here can fail a command, and every tool run is bounded (`tool.rs`).
use std::cell::OnceCell;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use retrace_core::EXE_BASE;
use crate::tool::{self, ToolError};

/// The executable of one epoch, as far as lines are concerned.
pub struct SourceMap {
    /// The trusted image, or the reason there is none (`resolve_image`'s own line, or empty for no candidate).
    image: Result<PathBuf, String>,
    /// The recorded `LC_UUID`, which the dSYM read must carry too. `None` only for an image with none (T0-5's
    /// fallback).
    want: Option<[u8; 16]>,
    /// The end of the executable's `__TEXT` in the guest: a pc in `EXE_BASE..text_end` is the program's own.
    text_end: u64,
    /// The dSYM `atos` reads, made at the first lookup that needs one, or the note saying why there is none.
    dsym: OnceCell<Result<Dsym, String>>,
}

/// A dSYM for one image: where it is, the `-arch` of the slice, and the private directory to remove on drop when
/// this map made it.
struct Dsym {
    bundle: PathBuf,
    arch: &'static str,
    /// Held for its `Drop`.
    _scratch: Option<Scratch>,
}

/// A private temp directory (`retrace-dsym-<pid>-<n>` under `std::env::temp_dir()`), removed with everything in it
/// on drop.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

impl Scratch {
    fn new() -> Result<Scratch, String> {
        use std::os::unix::fs::DirBuilderExt;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir();
        let mut last = String::new();
        for _ in 0..16 {
            let p = base.join(format!("retrace-dsym-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match std::fs::DirBuilder::new().mode(0o700).create(&p) {
                Ok(()) => return Ok(Scratch(p)),
                Err(e) => last = e.to_string(),
            }
        }
        Err(format!("dsymutil failed: no private directory under {}: {last}", base.display()))
    }
}

impl SourceMap {
    pub fn new(image: Result<String, String>, want: Option<[u8; 16]>, text_end: u64) -> SourceMap {
        SourceMap { image: image.map(PathBuf::from), want, text_end, dsym: OnceCell::new() }
    }

    /// Per-pc `(absolute file, line)` for pcs inside the executable and `None` for the rest, plus at most one note
    /// explaining an absence. One `atos` run per call, over only the pcs inside the executable.
    pub fn lines(&self, pcs: &[u64]) -> (Vec<Option<(String, u32)>>, Option<String>) {
        let mut out = vec![None; pcs.len()];
        let inside: Vec<usize> = (0..pcs.len()).filter(|&i| (EXE_BASE..self.text_end).contains(&pcs[i])).collect();
        if inside.is_empty() { return (out, None); }
        let image = match &self.image {
            Ok(p) => p,
            Err(line) => return (out, note_of(line)),
        };
        let dsym = match self.dsym.get_or_init(|| make_dsym(image, self.want)) {
            Ok(d) => d,
            Err(note) => return (out, Some(note.clone())),
        };
        let mut cmd = Command::new("/usr/bin/atos");
        cmd.args(["-fullPath", "-o"]).arg(&dsym.bundle).args(["-arch", dsym.arch, "-l", &format!("{EXE_BASE:#x}")]);
        for &i in &inside { cmd.arg(format!("{:#x}", pcs[i])); }
        let res = match tool::run(&mut cmd, None, tool::ATOS_SECS) {
            Ok(r) if r.status.success() => r,
            Ok(r) => return (out, Some(format!("atos is not available: {}", String::from_utf8_lossy(&r.stderr).trim()))),
            Err(ToolError::Spawn(e)) => return (out, Some(format!("atos is not available: {e}"))),
            Err(ToolError::TimedOut) => return (out, Some(format!("atos timed out after {} s", tool::ATOS_SECS))),
        };
        let note = assign(&inside, &String::from_utf8_lossy(&res.stdout), &mut out);
        (out, note)
    }
}

/// The `-arch` name of the slice `resolve_image` checked (`try_slice_native`: arm64e when the file has one, else
/// arm64), from that slice's `cpusubtype`.
pub(crate) fn arch_of(bytes: &[u8]) -> Option<&'static str> {
    let thin = retrace_guest::try_slice_native(bytes).ok()?;
    let sub = u32::from_le_bytes(thin.get(8..12)?.try_into().ok()?);
    Some(if sub & 0x00ff_ffff == retrace_arch::CPU_SUBTYPE_ARM64E { "arm64e" } else { "arm64" })
}

/// Whether `bundle` holds a DWARF file whose `LC_UUID` is `want` (any DWARF file when `want` is `None`).
fn dsym_matches(bundle: &Path, want: Option<[u8; 16]>) -> bool {
    let Ok(rd) = std::fs::read_dir(bundle.join("Contents/Resources/DWARF")) else { return false };
    rd.flatten().any(|e| match want {
        None => true,
        Some(w) => std::fs::read(e.path()).ok().and_then(|b| retrace_guest::file_uuid(&b)) == Some(w),
    })
}

/// The dSYM for `image`: the one beside it when it carries `want`, else a fresh one from `dsymutil` in a private
/// temp directory. `Err` is the note. Never the image itself: see the module's header.
fn make_dsym(image: &Path, want: Option<[u8; 16]>) -> Result<Dsym, String> {
    let bytes = std::fs::read(image).map_err(|_| format!("{} was not found", image.display()))?;
    if want.is_some() && retrace_guest::file_uuid(&bytes) != want {
        return Err("the binary changed since the recording".into());
    }
    let arch = arch_of(&bytes).ok_or_else(|| format!("{} is not an arm64 Mach-O", image.display()))?;
    let mut beside = image.as_os_str().to_owned();
    beside.push(".dSYM");
    let beside = PathBuf::from(beside);
    if want.is_some() && dsym_matches(&beside, want) {
        return Ok(Dsym { bundle: beside, arch, _scratch: None });
    }
    make_dsym_with(Path::new("/usr/bin/dsymutil"), image, want, arch, tool::DSYMUTIL_SECS)
}

/// `dsymutil <image> -o <scratch>/<name>.dSYM`, with the tool's path and bound given (a test passes a missing one).
fn make_dsym_with(dsymutil: &Path, image: &Path, want: Option<[u8; 16]>, arch: &'static str, secs: u64) -> Result<Dsym, String> {
    let scratch = Scratch::new()?;
    let name = image.file_name().map_or_else(|| "image".into(), |n| n.to_string_lossy().into_owned());
    let bundle = scratch.0.join(format!("{name}.dSYM"));
    let mut cmd = Command::new(dsymutil);
    cmd.arg(image).arg("-o").arg(&bundle);
    match tool::run(&mut cmd, None, secs) {
        Ok(r) if r.status.success() => {}
        Ok(r) => {
            let err = String::from_utf8_lossy(&r.stderr);
            let first = err.lines().map(str::trim).find(|l| !l.is_empty())
                .map_or_else(|| format!("exit status {}", r.status), str::to_string);
            return Err(format!("dsymutil failed: {first}"));
        }
        Err(ToolError::Spawn(e)) => return Err(format!("dsymutil is not available: {e}")),
        Err(ToolError::TimedOut) => return Err(format!("dsymutil timed out after {secs} s")),
    }
    if !dsym_matches(&bundle, want) {
        // The file changed between the UUID check and dsymutil's read, or dsymutil wrote nothing.
        return Err(if bundle.exists() { "the binary changed since the recording".into() }
            else { "dsymutil failed: it wrote no dSYM".into() });
    }
    Ok(Dsym { bundle, arch, _scratch: Some(scratch) })
}

/// Ruling P12: the note when the image is trusted and atos ran but gave no `(file:line)` for any queried pc.
/// Rust's std is linked into the executable's text, so this never claims the binary lacks debug info.
const NO_LINE: &str = "no line information at this address";

/// Map atos's output lines to the `inside` slots of `out` by order; the note is `NO_LINE` when none got a location.
/// The match is by position alone, so output with another line count than the pcs asked for is dropped whole,
/// rather than a line given to the wrong frame.
fn assign(inside: &[usize], text: &str, out: &mut [Option<(String, u32)>]) -> Option<String> {
    if text.lines().count() != inside.len() { return Some(NO_LINE.to_string()); }
    let mut any = false;
    for (&i, l) in inside.iter().zip(text.lines()) {
        out[i] = parse_atos_line(l);
        any |= out[i].is_some();
    }
    if any { None } else { Some(NO_LINE.to_string()) }
}

/// `resolve_image`'s stderr line, as the note of a session that has no trusted image.
fn note_of(line: &str) -> Option<String> {
    if line.contains("its UUID differs") { return Some("the binary changed since the recording".into()); }
    if let Some(i) = line.find(" was not found") {
        return Some(format!("{} was not found", line["retrace: ".len().min(i)..i].trim()));
    }
    if line.is_empty() { None } else { Some(line.trim_start_matches("retrace: ").to_string()) }
}

/// The `(file, line)` of one `atos -fullPath` output line: its last `(<absolute path>:<line>)` group. A line
/// without one (`__rustc::rust_panic (in x) + 0`, or a bare address) is `None`.
///
/// Two locations a dSYM holds are not lines in a file on this machine, and are `None` too (measured on the ledger's
/// dSYM): line 0, DWARF's "no source line" (`core::panicking::assert_failed_inner`), and a path under `/rustc/`,
/// the prefix Rust's prebuilt standard library is compiled under (`/rustc/<commit>/library/std/src/panicking.rs`),
/// which exists on no machine. `atos` over the executable never gave either, so a frame in the precompiled standard
/// library has no line, as before (Ruling P12), while generic standard-library code compiled into the program keeps
/// the toolchain's real path.
pub fn parse_atos_line(l: &str) -> Option<(String, u32)> {
    let l = l.trim_end().strip_suffix(')')?;
    let start = l.rfind(" (/")? + 3 - 1; // the '/' that opens the path
    let (path, line) = l[start..].rsplit_once(':')?;
    let line: u32 = line.parse().ok().filter(|&n| n > 0)?;
    if path.starts_with("/rustc/") { return None; }
    Some((path.to_string(), line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_located_line_gives_its_file_and_line() {
        assert_eq!(
            parse_atos_line("rt_probe::tests::fails_on_purpose::h64099bb73bd71502 (in rt_probe-92534031f5c95304) (/a/b/src/lib.rs:21)"),
            Some(("/a/b/src/lib.rs".to_string(), 21)));
    }
    #[test]
    fn a_line_without_a_location_is_none() {
        assert_eq!(parse_atos_line("__rustc::rust_panic (in x) + 0"), None);
        assert_eq!(parse_atos_line("0x100000e70"), None);
    }
    #[test]
    fn line_zero_and_rustc_s_virtual_paths_are_no_location() {
        assert_eq!(parse_atos_line("core::panicking::assert_failed_inner (in x) (/Users/u/.rustup/toolchains/t/lib/rustlib/src/rust/library/core/src/panicking.rs:0)"), None);
        assert_eq!(parse_atos_line("__rustc::rust_panic (in x) (/rustc/59807616e1fa2540724bfbac14d7976d7e4a3860/library/std/src/panicking.rs:885)"), None);
        // The toolchain's real path, for generic code compiled into the program, stays.
        assert_eq!(parse_atos_line("core::panicking::assert_failed (in x) (/Users/u/.rustup/toolchains/t/lib/rustlib/src/rust/library/core/src/panicking.rs:394)"),
                   Some(("/Users/u/.rustup/toolchains/t/lib/rustlib/src/rust/library/core/src/panicking.rs".to_string(), 394)));
    }
    #[test]
    fn a_path_with_parentheses_takes_the_last_group() {
        assert_eq!(parse_atos_line("f (in x) (/a (b)/c(d)/src/lib.rs:7)"), Some(("/a (b)/c(d)/src/lib.rs".to_string(), 7)));
        assert_eq!(parse_atos_line("f (in x) (/a/(b)/src/lib.rs:7)"), Some(("/a/(b)/src/lib.rs".to_string(), 7)));
    }
    #[test]
    fn notes_map_the_resolver_lines() {
        assert_eq!(note_of("retrace: ./p is not the program that was recorded (its UUID differs) - symbols are not loaded; pass --exe <path> to name the right file").as_deref(),
                   Some("the binary changed since the recording"));
        assert_eq!(note_of("retrace: ./p was not found - symbols are not loaded; pass --exe <path> to name the right file").as_deref(),
                   Some("./p was not found"));
        assert_eq!(note_of(""), None);
    }
    #[test]
    fn a_symbol_without_a_location_gets_the_no_line_note() {
        let mut out = vec![None];
        assert_eq!(assign(&[0], "__rustc::rust_panic (in x) + 0\n", &mut out).as_deref(), Some("no line information at this address"));
        assert_eq!(out, vec![None]);
        let mut out = vec![None, None];
        assert_eq!(assign(&[0, 1], "f (in x) (/a/b.rs:3)\ng (in x) + 4\n", &mut out), None);
        assert_eq!(out[0], Some(("/a/b.rs".to_string(), 3)));
    }
    #[test]
    fn output_of_another_line_count_is_dropped_whole() {
        // One line short, or one line over: no frame gets a line that might be another frame's.
        let mut out = vec![None, None];
        assert_eq!(assign(&[0, 1], "f (in x) (/a/b.rs:3)\n", &mut out).as_deref(), Some("no line information at this address"));
        assert_eq!(out, vec![None, None]);
        assert_eq!(assign(&[0, 1], "f (in x) (/a/b.rs:3)\ng (in x) (/a/c.rs:4)\nwarning\n", &mut out).as_deref(),
                   Some("no line information at this address"));
        assert_eq!(out, vec![None, None]);
    }
    #[test]
    fn pcs_outside_the_executable_cost_no_atos_run() {
        let m = SourceMap::new(Err(String::new()), None, EXE_BASE + 0x4000);
        assert_eq!(m.lines(&[0x1000, EXE_BASE + 0x4000]), (vec![None, None], None));
    }
    #[test]
    fn the_arch_is_the_checked_slice_s() {
        assert_eq!(arch_of(&std::fs::read(retrace_guest::CRASHY).unwrap()), Some("arm64"));
        // A universal Apple binary: the arm64e slice, as `try_slice_native` picks it.
        assert_eq!(arch_of(&std::fs::read("/bin/ls").unwrap()), Some("arm64e"));
    }
    #[test]
    fn a_missing_dsymutil_gives_no_lines_and_says_so() {
        let e = make_dsym_with(Path::new("/nonexistent/dsymutil"), Path::new(retrace_guest::CRASHY), None, "arm64", 5).err();
        assert!(e.as_deref().is_some_and(|e| e.starts_with("dsymutil is not available: ")), "{e:?}");
    }
    #[test]
    fn a_failing_dsymutil_names_its_first_stderr_line() {
        // `/usr/bin/false` exits 1 with no stderr: the status stands in for the line.
        let e = make_dsym_with(Path::new("/usr/bin/false"), Path::new(retrace_guest::CRASHY), None, "arm64", 5).err();
        assert!(e.as_deref().is_some_and(|e| e.starts_with("dsymutil failed: exit status")), "{e:?}");
    }
    #[test]
    fn a_made_dsym_carries_the_image_s_uuid_and_goes_with_its_map() {
        let want = retrace_guest::file_uuid(&std::fs::read(retrace_guest::CRASHY).unwrap());
        let d = make_dsym(Path::new(retrace_guest::CRASHY), want).unwrap_or_else(|e| panic!("{e}"));
        let dir = d._scratch.as_ref().expect("a scratch directory").0.clone();
        assert!(dir.file_name().unwrap().to_string_lossy().starts_with("retrace-dsym-"), "{}", dir.display());
        assert!(dsym_matches(&d.bundle, want));
        drop(d);
        assert!(!dir.exists(), "{} outlived its map", dir.display());
        // Another image's UUID is refused, before dsymutil runs.
        assert_eq!(make_dsym(Path::new(retrace_guest::HELLO_DYN), want).err().as_deref(), Some("the binary changed since the recording"));
    }
}
