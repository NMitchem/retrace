//! M58 B3: file and line for a pc in the program's own code, through macOS's `atos` over the executable on
//! disk. The file is trusted only when `image::resolve_image` found one whose `LC_UUID` is the recorded one.
//!
//! This is the one host-derived field of `debug`'s output (it comes from the host's file and tool, not from
//! the recording), and it is never written into a trace. Every failure degrades to "no file, no line" plus one
//! note saying why; nothing here can fail a command.
use std::path::PathBuf;
use std::process::Command;
use retrace_core::EXE_BASE;

/// The executable of one epoch, as far as lines are concerned.
pub struct SourceMap {
    /// The trusted image, or the reason there is none (`resolve_image`'s own line, or empty for no candidate).
    image: Result<PathBuf, String>,
    /// The end of the executable's `__TEXT` in the guest: a pc in `EXE_BASE..text_end` is the program's own.
    text_end: u64,
}

impl SourceMap {
    pub fn new(image: Result<String, String>, text_end: u64) -> SourceMap {
        SourceMap { image: image.map(PathBuf::from), text_end }
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
        let mut cmd = Command::new("/usr/bin/atos");
        cmd.args(["-fullPath", "-o"]).arg(image).args(["-l", &format!("{EXE_BASE:#x}")]);
        for &i in &inside { cmd.arg(format!("{:#x}", pcs[i])); }
        let res = match cmd.output() {
            Ok(r) if r.status.success() => r,
            Ok(r) => return (out, Some(format!("atos is not available: {}", String::from_utf8_lossy(&r.stderr).trim()))),
            Err(e) => return (out, Some(format!("atos is not available: {e}"))),
        };
        let note = assign(&inside, &String::from_utf8_lossy(&res.stdout), &mut out);
        (out, note)
    }
}

/// Ruling P12: the note when the image is trusted and atos ran but gave no `(file:line)` for any queried pc.
/// Rust's std is linked into the executable's text, so this never claims the binary lacks debug info.
const NO_LINE: &str = "no line information at this address";

/// Map atos's output lines to the `inside` slots of `out` by order; the note is `NO_LINE` when none got a location.
fn assign(inside: &[usize], text: &str, out: &mut [Option<(String, u32)>]) -> Option<String> {
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
pub fn parse_atos_line(l: &str) -> Option<(String, u32)> {
    let l = l.trim_end().strip_suffix(')')?;
    let start = l.rfind(" (/")? + 3 - 1; // the '/' that opens the path
    let (path, line) = l[start..].rsplit_once(':')?;
    Some((path.to_string(), line.parse().ok()?))
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
    fn pcs_outside_the_executable_cost_no_atos_run() {
        let m = SourceMap::new(Err(String::new()), EXE_BASE + 0x4000);
        assert_eq!(m.lines(&[0x1000, EXE_BASE + 0x4000]), (vec![None, None], None));
    }
}
