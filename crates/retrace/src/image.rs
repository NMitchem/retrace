//! Which file on disk is the recorded program: M56 §3c's UUID-checked image resolution, shared by the
//! gdbserver (lldb's image list) and the debugger's file:line (M58 B3).
use std::path::{Path, PathBuf};
use retrace_core::{ReplaySession, EXE_BASE};
use crate::rsp;

/// The most of the exe's header and load commands read. `sizeofcmds` comes from guest memory, so a
/// corrupt header must not send the server walking a huge range.
const MAX_HDR: usize = 1 << 20;

/// §3g: the one-image JSON for the executable mapped at `EXE_BASE` in `s`: its header and load
/// commands read from memory, named by `exe_arg` or else by the start stack's `argv[0]`.
///
/// M56 §3c: a candidate path is trusted when the file it names has the image's `LC_UUID` (`resolve_image`). When
/// no candidate is trusted, the one line saying why goes to stderr, and the server sends no image. Epoch 0's is
/// printed by `Server::new`, before `listening on`.
pub(crate) fn exec_image_of(s: &ReplaySession, exe_arg: Option<String>, trace_dir: &Path) -> Option<String> {
    let head = s.read_mem_prefix(EXE_BASE, 32);
    let sizeofcmds = head.get(20..24).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
    let exe_hdr = s.read_mem_prefix(EXE_BASE, 32usize.saturating_add(sizeofcmds).min(MAX_HDR));
    let want = retrace_guest::macho_uuid(&exe_hdr);
    // With no recorded UUID, today's rule: `--exe` is trusted as given, and anything else only when absolute.
    if want.is_none() {
        if let Some(p) = &exe_arg { return rsp::image_json(&exe_hdr, EXE_BASE, p).ok(); }
    }
    let cands: Vec<String> = exe_arg.into_iter().chain(exec_path(s)).chain(argv0(s)).collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    // The path lldb is handed must name the file wherever lldb runs, so a relative trace directory is
    // made absolute against the server's own working directory.
    let trace_dir = cwd.join(trace_dir);
    match resolve_image(&cands, &cwd, &trace_dir, want) {
        Ok(path) => rsp::image_json(&exe_hdr, EXE_BASE, &path).ok(),
        Err(line) => {
            if !line.is_empty() { eprintln!("{line}"); }
            None
        }
    }
}

/// M58 B3: the recorded `LC_UUID` of the image mapped at `EXE_BASE` in `s`, read from its header in memory.
pub(crate) fn recorded_uuid(s: &ReplaySession) -> Option<[u8; 16]> {
    let head = s.read_mem_prefix(EXE_BASE, 32);
    let sizeofcmds = head.get(20..24).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
    retrace_guest::macho_uuid(&s.read_mem_prefix(EXE_BASE, 32usize.saturating_add(sizeofcmds).min(MAX_HDR)))
}

/// M58 B3: the candidates for epoch 0's program, read from the opening start stack: `executable_path=`, then
/// `argv[0]`. Only meaningful at the start of the process.
pub(crate) fn start_candidates(s: &ReplaySession) -> Vec<String> {
    exec_path(s).into_iter().chain(argv0(s)).collect()
}

/// M58 B3: `resolve_image` with the trace's directory made absolute against the working directory, as
/// `exec_image_of` does, for the debugger's source lines.
pub(crate) fn resolve_for_source(cands: &[String], trace_dir: &Path, want: Option<[u8; 16]>) -> Result<String, String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    resolve_image(cands, &cwd, &cwd.join(trace_dir), want)
}

/// M56 §3c: the first of `cands` (in order) whose file has the recorded `LC_UUID`. A relative candidate is
/// tried against `cwd`, then `trace_dir`; the trusted path is returned as it was found. `Err` is the one stderr
/// line for the first candidate, or empty when there is no candidate (a static guest with no path is not news).
pub(crate) fn resolve_image(cands: &[String], cwd: &Path, trace_dir: &Path, want: Option<[u8; 16]>) -> Result<String, String> {
    let Some(first) = cands.first() else { return Err(String::new()) };
    let Some(want) = want else {
        // Measured (t0 §5, Ruling T0-5): 0 of 42 `firstcontact` fixtures lack an LC_UUID, and it matched the
        // file's in all 44 programs probed, so this branch is a rare fallback to the pre-M56 rule.
        return match cands.iter().find(|c| c.starts_with('/')) {
            Some(c) => Ok(c.clone()),
            None => Err("retrace: the recorded program has no LC_UUID - symbols are loaded only from an \
                         absolute path; pass --exe <path> to name the right file".into()),
        };
    };
    let mut first_found = false;
    for (i, cand) in cands.iter().enumerate() {
        let places: Vec<PathBuf> = if cand.starts_with('/') { vec![PathBuf::from(cand)] }
            else { vec![cwd.join(cand), trace_dir.join(cand)] };
        for p in places {
            let Ok(bytes) = std::fs::read(&p) else { continue };
            if i == 0 { first_found = true; }
            if retrace_guest::file_uuid(&bytes) == Some(want) { return Ok(p.to_string_lossy().into_owned()); }
        }
    }
    Err(if first_found {
        format!("retrace: {first} is not the program that was recorded (its UUID differs) - symbols are not loaded; \
                 pass --exe <path> to name the right file")
    } else {
        format!("retrace: {first} was not found - symbols are not loaded; pass --exe <path> to name the right file")
    })
}

/// Final review part 2, M1 (amends Ruling T6-b): the path the kernel used, `executable_path=` (apple[0]) on the same start
/// stack, which a program's own `argv[0]` (a bare name from `Command::exec`, or a launcher's own) need not name. It is
/// the path as the exec call wrote it (for a script, the interpreter as written), and `argv[0]` for epoch 0 by C3.
/// Read as `argv0` reads argv, past envp's NULL: `[sp + 8]` is argc, argv follows, then envp, then apple. Returned as
/// recorded, absolute or relative; `resolve_image` decides whether to trust it, by `LC_UUID` (M56 §3c).
pub(crate) fn exec_path(s: &ReplaySession) -> Option<String> {
    let sp = s.thread_ctx(0)?.regs.sp_el0;
    let word = |a: u64| <[u8; 8]>::try_from(s.read_mem_prefix(a, 8)).ok().map(u64::from_le_bytes);
    if word(sp)? != EXE_BASE { return None; }
    let argc = word(sp + 8)?;
    if argc > 1 << 16 { return None; }
    // argv's `argc` entries and its NULL, then envp up to its NULL, and apple[0] is the next word.
    let mut at = sp + 16 + 8 * (argc + 1);
    for _ in 0..(1 << 16) {
        if word(at)? == 0 { break; }
        at += 8;
    }
    let bytes = s.read_mem_prefix(word(at + 8)?, 1024);
    let end = bytes.iter().position(|&b| b == 0)?;
    let path = String::from_utf8(bytes[..end].to_vec()).ok()?.strip_prefix("executable_path=")?.to_owned();
    Some(path)
}

/// §3g (t0 R3): `record-dyn`'s argv[0], out of the opening stack: `[sp]` is the main image's
/// mach_header (the KernelArgs `build_start_stack` writes) and `[sp + 16]` points at argv[0]. Returned
/// as recorded, absolute or relative; `resolve_image` decides whether to trust it, by `LC_UUID` (M56 §3c). A
/// static guest has no KernelArgs, so `[sp]` is not `EXE_BASE` and this is None.
pub(crate) fn argv0(s: &ReplaySession) -> Option<String> {
    let sp = s.thread_ctx(0)?.regs.sp_el0;
    let word = |a: u64| <[u8; 8]>::try_from(s.read_mem_prefix(a, 8)).ok().map(u64::from_le_bytes);
    if word(sp)? != EXE_BASE { return None; }
    let bytes = s.read_mem_prefix(word(sp + 16)?, 1024);
    let end = bytes.iter().position(|&b| b == 0)?;
    String::from_utf8(bytes[..end].to_vec()).ok()
}

#[cfg(test)]
mod image_tests {
    use super::resolve_image;
    use std::path::{Path, PathBuf};

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("retrace-image-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn want() -> Option<[u8; 16]> { retrace_guest::file_uuid(&std::fs::read(retrace_guest::CRASHY).unwrap()) }
    fn s(p: &Path) -> String { p.to_str().unwrap().to_owned() }

    #[test]
    fn a_relative_candidate_with_the_recorded_uuid_is_trusted() {
        let (cwd, other) = (tmp("rel-cwd"), tmp("rel-other"));
        std::fs::copy(retrace_guest::CRASHY, cwd.join("crashy")).unwrap();
        assert_eq!(resolve_image(&["./crashy".into()], &cwd, &other, want()), Ok(s(&cwd.join("./crashy"))));
    }

    #[test]
    fn the_trace_directory_is_tried_after_the_working_directory() {
        let (cwd, tdir) = (tmp("td-cwd"), tmp("td-trace"));
        std::fs::copy(retrace_guest::CRASHY, tdir.join("crashy")).unwrap();
        assert_eq!(resolve_image(&["crashy".into()], &cwd, &tdir, want()), Ok(s(&tdir.join("crashy"))));
        // A wrong file in the working directory does not stop the search.
        std::fs::copy(retrace_guest::HELLO_DYN, cwd.join("crashy")).unwrap();
        assert_eq!(resolve_image(&["crashy".into()], &cwd, &tdir, want()), Ok(s(&tdir.join("crashy"))));
        // The working directory wins when both match.
        std::fs::copy(retrace_guest::CRASHY, cwd.join("crashy")).unwrap();
        assert_eq!(resolve_image(&["crashy".into()], &cwd, &tdir, want()), Ok(s(&cwd.join("crashy"))));
    }

    #[test]
    fn a_rebuilt_binary_at_the_recorded_path_is_refused_with_the_uuid_line() {
        let d = tmp("rebuilt");
        let p = s(&d.join("crashy"));
        std::fs::copy(retrace_guest::HELLO_DYN, &p).unwrap();
        assert_eq!(resolve_image(std::slice::from_ref(&p), &d, &d, want()), Err(format!(
            "retrace: {p} is not the program that was recorded (its UUID differs) - symbols are not loaded; \
             pass --exe <path> to name the right file")));
    }

    #[test]
    fn a_missing_file_gives_the_not_found_line() {
        let d = tmp("missing");
        assert_eq!(resolve_image(&["./nope".into()], &d, &d, want()), Err(
            "retrace: ./nope was not found - symbols are not loaded; pass --exe <path> to name the right file".into()));
    }

    #[test]
    fn exe_naming_the_right_file_wins() {
        let d = tmp("exe");
        let (good, bad) = (s(&d.join("good")), s(&d.join("bad")));
        std::fs::copy(retrace_guest::CRASHY, &good).unwrap();
        std::fs::copy(retrace_guest::HELLO_DYN, &bad).unwrap();
        assert_eq!(resolve_image(&[good.clone(), bad.clone()], &d, &d, want()), Ok(good.clone()));
        assert_eq!(resolve_image(&[bad, good.clone()], &d, &d, want()), Ok(good));
    }

    #[test]
    fn without_a_recorded_uuid_only_an_absolute_path_is_trusted() {
        let d = tmp("nouuid");
        // No file is read: the absolute path need not exist.
        assert_eq!(resolve_image(&["/no/such/prog".into()], &d, &d, None), Ok("/no/such/prog".into()));
        std::fs::copy(retrace_guest::CRASHY, d.join("crashy")).unwrap();
        assert_eq!(resolve_image(&["crashy".into()], &d, &d, None), Err(
            "retrace: the recorded program has no LC_UUID - symbols are loaded only from an absolute path; \
             pass --exe <path> to name the right file".into()));
    }
}
