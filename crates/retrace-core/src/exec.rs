//! M54 t3: the record-only host side of `execve`: resolve the path the way the kernel does and hand back
//! the bytes `Event::Exec` carries. Replay never calls `resolve` (spec 3c: replay takes the image, the
//! dylinker and the close set from the event and reads no file but the shared cache); both sides call
//! `image`, on the same bytes, to build what `Box_::exec_image` installs.
use retrace_arch::WallCategory;
use retrace_box::{final_args_refusal, script_argv, ExecCall, ExecImage, ExecRefusal, EXE_BASE, SHEBANG_MAX};
use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;

/// What `resolve` hands the record arm for `Event::Exec`: the selected slice of the executable, the
/// `#!` line (without its newline; empty when the file is not a script), and the selected slice of the image's dynamic linker.
pub struct Resolved { pub image: Vec<u8>, pub script: Vec<u8>, pub dylinker: Vec<u8> }

const ENOEXEC: u64 = 8;
const EACCES: u64 = 13;
const MH_EXECUTE: u32 = 2;
const CPU_TYPE_X86_64: u32 = 0x0100_0007;

/// The call the program made, as `read_exec_call` names it.
fn cname(call: &ExecCall) -> &'static str { if call.num == retrace_arch::SYS_POSIX_SPAWN { "posix_spawn" } else { "execve" } }

fn wall(what: impl Into<String>) -> ExecRefusal { ExecRefusal::Wall(retrace_arch::wall(WallCategory::ProcessCreation, what)) }

fn errno_of(e: &std::io::Error) -> ExecRefusal { ExecRefusal::Errno(e.raw_os_error().unwrap_or(EACCES as i32) as u64) }

const EBADARCH: u64 = 86;

/// What a binary preference (`posix_spawnattr_setbinpref_np`, `setarchpref_np`) decides about an image's slices
/// (t4 addendum D, from t0 §M3's table).
enum Pick {
    /// Run the slice at this index; `None` is the default choice (arm64e, else arm64) over the whole file.
    Index(Option<usize>),
    /// `EBADARCH`: no slice satisfies the preference.
    NoArch,
    /// A preference for a cputype the image has and retrace cannot run (Rosetta): a wall.
    Rosetta(u32),
}

/// Entry 0 decides (entries 1..3 are unset by `decode_spawn`). Unset, `CPU_TYPE_ANY`, and `CPU_TYPE_ARM64` with
/// no subtype (-1) take the default choice. `CPU_TYPE_ARM64` with a subtype takes the arm64 slice whose
/// cpusubtype, capability byte masked off, equals it, else `EBADARCH`. A non-arm64 cputype is `EBADARCH` when
/// no slice has it and a wall when one does. `slices` is `(cputype, cpusubtype)` per slice.
fn select(pref: (u32, i32), slices: &[(u32, u32)]) -> Pick {
    let (cpu, sub) = pref;
    if cpu == 0 || cpu == retrace_arch::CPU_TYPE_ANY || (cpu == retrace_arch::CPU_TYPE_ARM64 && sub == -1) {
        return Pick::Index(None);
    }
    if cpu == retrace_arch::CPU_TYPE_ARM64 {
        return match slices.iter().position(|&(c, s)| c == cpu && (s & 0x00ff_ffff) as i32 == sub) {
            Some(i) => Pick::Index(Some(i)),
            None => Pick::NoArch,
        };
    }
    if slices.iter().any(|&(c, _)| c == cpu) { Pick::Rosetta(cpu) } else { Pick::NoArch }
}

fn rosetta(cpu: u32) -> ExecRefusal {
    wall(format!("posix_spawn with a binary preference for CPU type {cpu:#x}, an architecture retrace does not run (needs Rosetta)"))
}

fn le32(b: &[u8], o: usize) -> u32 { u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) }
fn be32(b: &[u8], o: usize) -> u32 { u32::from_be_bytes(b[o..o + 4].try_into().unwrap()) }

/// The `(cputype, cpusubtype, offset, size)` of each slice of a fat file, or `None` for a table that does not fit the
/// file (a `0xcafebabe` that is not a universal binary, such as a Java class, is not one).
fn fat_slices(b: &[u8]) -> Option<Vec<(u32, u32, usize, usize)>> {
    let is64 = be32(b, 0) == retrace_arch::FAT_MAGIC_64;
    let (stride, n) = (if is64 { 32 } else { 20 }, be32(b, 4) as usize);
    if n > 64 || 8 + n * stride > b.len() { return None; }
    (0..n).map(|i| {
        let e = 8 + i * stride;
        let (off, size) = if is64 {
            (u64::from_be_bytes(b[e + 8..e + 16].try_into().unwrap()) as usize,
             u64::from_be_bytes(b[e + 16..e + 24].try_into().unwrap()) as usize)
        } else { (be32(b, e + 8) as usize, be32(b, e + 12) as usize) };
        (off.checked_add(size)? <= b.len()).then(|| (be32(b, e), be32(b, e + 4), off, size))
    }).collect()
}

/// The dynamic linker `image` names, read and sliced the way the start does (the CLI reads `/usr/lib/dyld`
/// and takes its arm64e slice).
fn dylinker_slice(path: &str, name: &str) -> Result<Vec<u8>, ExecRefusal> {
    let bytes = std::fs::read(path)
        .map_err(|e| wall(format!("{name} of an image whose dynamic linker {path} cannot be read ({e})")))?;
    if bytes.len() < 8 || (!bytes.starts_with(&[0xcf, 0xfa, 0xed, 0xfe]) && fat_slices(&bytes).is_none()) {
        return Err(wall(format!("{name} of an image whose dynamic linker {path} is not a Mach-O")));
    }
    let slice = retrace_guest::try_slice_native(&bytes)
        .map_err(|why| wall(format!("{name} of an image whose dynamic linker {path} {why}")))?;
    retrace_guest::try_parse_macho(slice)
        .map_err(|why| wall(format!("{name} of an image whose dynamic linker {path} {why}")))?;
    Ok(slice.to_vec())
}

/// Resolve `call.path` as the kernel would, against the cwd, and check the file (spec 3e). Every error is
/// an errno the program is answered, or a wall by value; nothing here runs past the point of no return.
/// The order is the kernel's: the file first, the argument strings after (`ExecCall::args_refusal`).
pub fn resolve(call: &ExecCall, shared_region_end: u64) -> Result<Resolved, ExecRefusal> {
    let mut r = resolve_file(&call.path, call, true, shared_region_end)?;
    if !r.script.is_empty() && call.args_refusal.is_none() {
        // Ruling T5-b (inferred): the limits apply to the final vector, which the kernel copies into the image.
        let argv = script_argv(&r.script, &call.path, &call.argv).expect("resolve_file checked the line");
        if let Some(refusal) = final_args_refusal(&argv, &call.envp) { return Err(refusal); }
    }
    r.script.shrink_to_fit();
    Ok(r)
}

/// `resolve` for one path. A `#!` file (`allow_script`) resolves its interpreter through this same function,
/// once: an interpreter that is itself a script is ENOEXEC, as natively (t0 M2, t5 addendum C).
fn resolve_file(path_bytes: &[u8], call: &ExecCall, allow_script: bool, shared_region_end: u64) -> Result<Resolved, ExecRefusal> {
    let path = OsStr::from_bytes(path_bytes);
    // Not `File::open` first: opening a FIFO blocks until a writer comes, and exec of one is EACCES.
    let md = std::fs::metadata(path).map_err(|e| errno_of(&e))?;
    if !md.is_file() { return Err(ExecRefusal::Errno(EACCES)); }
    let cpath = CString::new(path_bytes.to_vec()).map_err(|_| ExecRefusal::Errno(2))?;
    // SAFETY: access(2) reads a NUL-terminated path and writes nothing.
    if unsafe { libc::access(cpath.as_ptr(), libc::X_OK) } != 0 { return Err(errno_of(&std::io::Error::last_os_error())); }
    if md.permissions().mode() & 0o6000 != 0 {
        return Err(wall(format!("{} of a setuid or setgid image (retrace cannot make the credential change)", cname(call))));
    }
    let bytes = std::fs::read(path).map_err(|e| errno_of(&e))?;
    if bytes.starts_with(b"#!") {
        // The line must end within its first 512 bytes (`script_argv` also refuses a long or empty one).
        if !allow_script { return Err(ExecRefusal::Errno(ENOEXEC)); }
        let Some(nl) = bytes[..bytes.len().min(SHEBANG_MAX)].iter().position(|&c| c == b'\n') else {
            return Err(ExecRefusal::Errno(ENOEXEC));
        };
        let line = bytes[..nl].to_vec();
        let words = script_argv(&line, path_bytes, &[]).map_err(ExecRefusal::Errno)?;
        let mut inner = resolve_file(&words[0], call, false, shared_region_end)?;
        inner.script = line;
        return Ok(inner);
    }
    // A `mach_header_64` is 32 bytes: the thin branch reads its cputype and cpusubtype at 4 and 8.
    if bytes.len() < 32 { return Err(ExecRefusal::Errno(ENOEXEC)); }
    let fat = be32(&bytes, 0) == retrace_arch::FAT_MAGIC || be32(&bytes, 0) == retrace_arch::FAT_MAGIC_64;
    let thin = le32(&bytes, 0) == 0xfeed_facf;
    if !fat && !thin { return Err(ExecRefusal::Errno(ENOEXEC)); }
    let pref = call.spawn.as_ref().map_or((0, -1), |p| p.binpref);
    let slice: &[u8] = if thin {
        let (cpu, sub) = (le32(&bytes, 4), le32(&bytes, 8));
        match select(pref, &[(cpu, sub)]) {
            Pick::Index(_) => match cpu {
                retrace_arch::CPU_TYPE_ARM64 => &bytes,
                CPU_TYPE_X86_64 => return Err(wall(format!("{} of an x86_64-only image (needs Rosetta)", cname(call)))),
                t => return Err(wall(format!("{} of a Mach-O for CPU type {t:#x}, and retrace runs arm64 programs only", cname(call)))),
            },
            Pick::NoArch => return Err(ExecRefusal::Errno(EBADARCH)),
            Pick::Rosetta(c) => return Err(rosetta(c)),
        }
    } else {
        let Some(slices) = fat_slices(&bytes) else { return Err(ExecRefusal::Errno(ENOEXEC)) };
        let list: Vec<(u32, u32)> = slices.iter().map(|&(c, s, _, _)| (c, s)).collect();
        match select(pref, &list) {
            Pick::Index(Some(i)) => { let (_, _, off, size) = slices[i]; &bytes[off..off + size] }
            Pick::Index(None) => match retrace_guest::try_slice_native(&bytes) {
                Ok(s) => s,
                Err(_) if slices.iter().any(|&(cpu, _, _, _)| cpu == CPU_TYPE_X86_64) =>
                    return Err(wall(format!("{} of an x86_64-only image (needs Rosetta)", cname(call)))),
                Err(_) => return Err(wall(format!("{} of a universal binary with no arm64 slice, and retrace runs arm64 programs only", cname(call)))),
            },
            Pick::NoArch => return Err(ExecRefusal::Errno(EBADARCH)),
            Pick::Rosetta(c) => return Err(rosetta(c)),
        }
    };
    if slice.len() < 32 || le32(slice, 0) != 0xfeed_facf || le32(slice, 12) != MH_EXECUTE { return Err(ExecRefusal::Errno(ENOEXEC)); }
    // Final review part 2, I3: load commands that cannot be walked are a wall, by value. Natively the kernel
    // accepts the file and then kills the process with SIGKILL while loading it (measured, exec_e2e), which
    // is no errno retrace can answer. Every other parse refusal keeps ENOEXEC.
    let loaded = retrace_guest::try_parse_macho(slice).map_err(|why| if why == retrace_guest::MALFORMED {
        wall(format!("{} of an image whose load commands cannot be walked (natively the kernel kills the process with SIGKILL while loading it)", cname(call)))
    } else { ExecRefusal::Errno(ENOEXEC) })?;
    let Some(dyld) = loaded.dylinker.as_deref() else {
        return Err(wall(format!("{} of a static image (no LC_LOAD_DYLINKER)", cname(call))));
    };
    // C2: restore's posture, the symbols and the gdbserver all read the Mach-O header at EXE_BASE.
    if !loaded.segments.iter().any(|s| s.data.starts_with(&[0xcf, 0xfa, 0xed, 0xfe]) && s.vaddr == EXE_BASE) {
        return Err(wall(format!("{} of an image whose Mach-O header is not at {EXE_BASE:#x}", cname(call))));
    }
    let dylinker = dylinker_slice(dyld, cname(call))?;
    // M6: segments `stage_image` cannot place are refused here, before the point of no return.
    let dl = retrace_guest::try_parse_macho(&dylinker).expect("dylinker_slice parsed it");
    if let Some(why) = retrace_box::image_layout_refusal(&loaded, &dl, shared_region_end) {
        return Err(wall(format!("{} of an image whose {why}", cname(call))));
    }
    Ok(Resolved { image: slice.to_vec(), script: Vec::new(), dylinker })
}

/// What `Box_::exec_image` installs, built from the recorded slices: record's resolved bytes and replay's
/// event bytes both come through here, so both sides install the same thing. `Err` is a retrace bug on
/// record (it parsed the same bytes in `resolve`) and a divergence on replay.
///
/// A script (`script` non-empty, M54 t5) installs its interpreter: the argv is `script_argv`'s over the call's own
/// path and argv, and `executable_path=` is the interpreter as the line wrote it.
pub fn image(call: &ExecCall, image: &[u8], script: &[u8], dylinker: &[u8], closed: &[u32]) -> Result<ExecImage, String> {
    let exe = retrace_guest::try_parse_macho(image).map_err(|why| format!("the exec image {why}"))?;
    let dyld = retrace_guest::try_parse_macho(dylinker).map_err(|why| format!("the exec image's dynamic linker {why}"))?;
    let (argv, exec_path) = if script.is_empty() { (call.argv.clone(), call.path.clone()) } else {
        let v = script_argv(script, &call.path, &call.argv).map_err(|e| format!("the recorded #! line is refused (errno {e})"))?;
        let p = v[0].clone();
        (v, p)
    };
    Ok(ExecImage { exe, dylinker: dyld, argv, envp: call.envp.clone(),
                   exec_path, closed: closed.to_vec(), spawn: call.spawn.clone(), host_new: Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use retrace_box::{ExecCall, ExecRefusal};
    use std::path::PathBuf;

    fn call(path: &str) -> ExecCall {
        ExecCall { num: 59, path: path.as_bytes().to_vec(), argv: vec![path.as_bytes().to_vec()], envp: vec![], spawn: None, args_refusal: None }
    }
    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("retrace-core-exec-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn mode(p: &std::path::Path, m: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap();
    }
    fn errno(r: Result<Resolved, ExecRefusal>) -> u64 {
        match r { Err(ExecRefusal::Errno(e)) => e, Err(ExecRefusal::Wall(m)) => panic!("a wall, not an errno: {m}"), Ok(_) => panic!("resolved") }
    }

    #[test]
    fn a_missing_path_is_enoent_and_a_directory_is_eacces() {
        let d = scratch("missing");
        assert_eq!(errno(resolve(&call(d.join("nope").to_str().unwrap()), retrace_box::SHARED_REGION_END_MIN)), 2, "ENOENT");
        assert_eq!(errno(resolve(&call(d.to_str().unwrap()), retrace_box::SHARED_REGION_END_MIN)), 13, "a directory is EACCES, as measured natively");
        // A path component that is a file: ENOTDIR.
        let f = d.join("f");
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(errno(resolve(&call(d.join("f/x").to_str().unwrap()), retrace_box::SHARED_REGION_END_MIN)), 20, "ENOTDIR");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_file_without_the_execute_bit_is_eacces_and_garbage_is_enoexec() {
        let d = scratch("modes");
        let noexec = d.join("noexec");
        std::fs::write(&noexec, b"x\n").unwrap();
        mode(&noexec, 0o644);
        assert_eq!(errno(resolve(&call(noexec.to_str().unwrap()), retrace_box::SHARED_REGION_END_MIN)), 13, "EACCES");
        let garbage = d.join("garbage");
        std::fs::write(&garbage, b"not a mach-o\n").unwrap();
        mode(&garbage, 0o755);
        assert_eq!(errno(resolve(&call(garbage.to_str().unwrap()), retrace_box::SHARED_REGION_END_MIN)), 8, "ENOEXEC");
        // An empty file with the execute bit, and a Mach-O object or library (not MH_EXECUTE): ENOEXEC.
        let empty = d.join("empty");
        std::fs::write(&empty, b"").unwrap();
        mode(&empty, 0o755);
        assert_eq!(errno(resolve(&call(empty.to_str().unwrap()), retrace_box::SHARED_REGION_END_MIN)), 8, "an empty file");
        let mut dylib = std::fs::read(retrace_guest::EXEC_TARGET_DYN).unwrap();
        dylib[12..16].copy_from_slice(&6u32.to_le_bytes()); // MH_DYLIB
        let lib = d.join("lib.dylib");
        std::fs::write(&lib, &dylib).unwrap();
        mode(&lib, 0o755);
        assert_eq!(errno(resolve(&call(lib.to_str().unwrap()), retrace_box::SHARED_REGION_END_MIN)), 8, "a dylib is not an executable");
        // A script whose interpreter is itself a script (nested) is ENOEXEC; one with no newline, too.
        let inner = d.join("inner");
        std::fs::write(&inner, b"#!/bin/echo hi\n").unwrap();
        mode(&inner, 0o755);
        let nested = d.join("nested");
        std::fs::write(&nested, format!("#!{}\n", inner.display())).unwrap();
        mode(&nested, 0o755);
        assert_eq!(errno(resolve(&call(nested.to_str().unwrap()), retrace_box::SHARED_REGION_END_MIN)), 8, "a nested script");
        let nonl = d.join("nonl");
        std::fs::write(&nonl, format!("#!{}", retrace_guest::EXEC_TARGET_DYN)).unwrap();
        mode(&nonl, 0o755);
        assert_eq!(errno(resolve(&call(nonl.to_str().unwrap()), retrace_box::SHARED_REGION_END_MIN)), 8, "a script whose only line has no newline");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_thin_header_shorter_than_a_mach_header_is_enoexec_not_a_panic() {
        let d = scratch("short");
        let short = d.join("short");
        std::fs::write(&short, [0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0x00, 0x00, 0x01, 0x02, 0x00]).unwrap();
        mode(&short, 0o755);
        assert_eq!(errno(resolve(&call(short.to_str().unwrap()), retrace_box::SHARED_REGION_END_MIN)), 8, "a 10-byte thin Mach-O");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A fat file around `slices` (cpusubtype, bytes), each 16 KiB aligned, as `lipo` writes it.
    fn fat(slices: &[(u32, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&0xcafe_babeu32.to_be_bytes());
        out.extend_from_slice(&(slices.len() as u32).to_be_bytes());
        let mut off = 0x4000usize;
        let mut offs = Vec::new();
        for (sub, b) in slices {
            out.extend_from_slice(&0x0100_000cu32.to_be_bytes());
            out.extend_from_slice(&sub.to_be_bytes());
            out.extend_from_slice(&(off as u32).to_be_bytes());
            out.extend_from_slice(&(b.len() as u32).to_be_bytes());
            out.extend_from_slice(&14u32.to_be_bytes());
            offs.push(off);
            off = (off + b.len()).div_ceil(0x4000) * 0x4000;
        }
        for ((_, b), o) in slices.iter().zip(offs) {
            out.resize(o, 0);
            out.extend_from_slice(b);
        }
        out
    }

    #[test]
    fn a_fat_file_yields_its_arm64e_slice_or_else_its_arm64_slice() {
        let d = scratch("fat");
        let plain = std::fs::read(retrace_guest::EXEC_TARGET_DYN).unwrap();
        let mut e = plain.clone();
        e[8..12].copy_from_slice(&0x8000_0002u32.to_le_bytes()); // cpusubtype arm64e: tells the slices apart
        let both = d.join("both");
        std::fs::write(&both, fat(&[(0, &plain), (2, &e)])).unwrap();
        mode(&both, 0o755);
        let r = resolve(&call(both.to_str().unwrap()), retrace_box::SHARED_REGION_END_MIN).expect("a fat file with both slices");
        assert_eq!(r.image, e, "the arm64e slice is preferred");
        assert!(r.script.is_empty());
        assert!(r.dylinker.len() > 1_000_000, "dyld's own selected slice travels with it");
        let only = d.join("only");
        std::fs::write(&only, fat(&[(0, &plain)])).unwrap();
        mode(&only, 0o755);
        let r = resolve(&call(only.to_str().unwrap()), retrace_box::SHARED_REGION_END_MIN).expect("a fat file with an arm64 slice only");
        assert_eq!(r.image, plain, "else the arm64 slice");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_script_resolves_to_its_interpreters_image_and_keeps_its_line() {
        let d = scratch("script");
        let s = d.join("s");
        std::fs::write(&s, format!("#!{} one\n", retrace_guest::EXEC_TARGET_DYN)).unwrap();
        mode(&s, 0o755);
        let r = resolve(&call(s.to_str().unwrap()), retrace_box::SHARED_REGION_END_MIN).expect("a script");
        assert_eq!(r.image, std::fs::read(retrace_guest::EXEC_TARGET_DYN).unwrap(), "the interpreter's slice");
        assert_eq!(r.script, format!("#!{} one", retrace_guest::EXEC_TARGET_DYN).into_bytes(), "the line, without its newline");
        let c = call(s.to_str().unwrap());
        let img = image(&c, &r.image, &r.script, &r.dylinker, &[]).unwrap();
        assert_eq!(img.exec_path, retrace_guest::EXEC_TARGET_DYN.as_bytes(), "executable_path is the interpreter as written");
        assert_eq!(img.argv[1], b"one");
        assert_eq!(img.argv[2], s.to_str().unwrap().as_bytes(), "then the script path as passed");
        // missing, directory and non-executable interpreters
        for (name, line, want) in [("m", "#!/nonexistent/i\n".to_string(), 2u64), ("d", format!("#!{}\n", d.display()), 13),
                                   ("e", "#!\n".to_string(), 8)] {
            let p = d.join(name);
            std::fs::write(&p, line).unwrap();
            mode(&p, 0o755);
            assert_eq!(errno(resolve(&call(p.to_str().unwrap()), retrace_box::SHARED_REGION_END_MIN)), want, "{name}");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    // Task 5 minors: the interpreter's own mode, and the line's 511/512-byte boundary (`< 512`, t5 addendum B).
    #[test]
    fn an_interpreter_needs_the_execute_bit_and_the_line_fits_in_511_bytes() {
        let d = scratch("shebang");
        let target = retrace_guest::EXEC_TARGET_DYN;
        let interp = d.join("interp");
        std::fs::copy(target, &interp).unwrap();
        mode(&interp, 0o644);
        let s = d.join("s");
        std::fs::write(&s, format!("#!{}\n", interp.display())).unwrap();
        mode(&s, 0o755);
        assert_eq!(errno(resolve(&call(s.to_str().unwrap()), retrace_box::SHARED_REGION_END_MIN)), 13, "an interpreter without the execute bit is EACCES");
        // A line of exactly 511 bytes (the `#!`, the interpreter, a comment tail) resolves; 512 is ENOEXEC.
        for (total, want) in [(511usize, 0u64), (512, 8)] {
            let head = format!("#!{} #", retrace_guest::EXEC_TARGET_DYN);
            let line = format!("{head}{}", "x".repeat(total - head.len()));
            assert_eq!(line.len(), total);
            let p = d.join(format!("l{total}"));
            std::fs::write(&p, format!("{line}\n")).unwrap();
            mode(&p, 0o755);
            match (resolve(&call(p.to_str().unwrap()), retrace_box::SHARED_REGION_END_MIN), want) {
                (Ok(r), 0) => assert_eq!(r.script.len(), 511),
                (r, w) if w != 0 => assert_eq!(errno(r), w, "a {total}-byte line"),
                (r, _) => panic!("a {total}-byte line should resolve: {:?}", r.err().map(|e| format!("{e:?}"))),
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    // M4: the wall names the call the program made.
    #[test]
    fn a_refused_image_names_the_call_that_asked() {
        let d = scratch("names");
        let x86 = d.join("x86");
        std::fs::copy(retrace_guest::EXEC_X86, &x86).unwrap();
        mode(&x86, 0o755);
        let mut c = call(x86.to_str().unwrap());
        let text = |c: &ExecCall| match resolve(c, retrace_box::SHARED_REGION_END_MIN) { Err(ExecRefusal::Wall(m)) => m, other => panic!("expected a wall: {:?}", other.err()) };
        assert!(text(&c).contains("execve of an x86_64-only image"), "{}", text(&c));
        c.num = 244;
        assert!(text(&c).contains("posix_spawn of an x86_64-only image"), "{}", text(&c));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Final review part 2, I3: thin arm64 `MH_EXECUTE` headers whose load commands cannot be walked.
    fn malformed() -> Vec<(&'static str, Vec<u8>)> {
        let hdr = |ncmds: u32, sizeofcmds: u32, cmds: &[u8]| {
            let mut b = Vec::new();
            for w in [0xfeed_facfu32, retrace_arch::CPU_TYPE_ARM64, 0, 2, ncmds, sizeofcmds, 0, 0] { b.extend_from_slice(&w.to_le_bytes()); }
            b.extend_from_slice(cmds);
            b
        };
        vec![("truncated: ncmds 1, no command bytes", hdr(1, 8, &[])),
             ("cmdsize 0", hdr(1, 8, &[0x19, 0, 0, 0, 0, 0, 0, 0])),
             ("huge ncmds", hdr(u32::MAX, 8, &[0x19, 0, 0, 0, 0, 0, 0, 0]))]
    }

    #[test]
    fn a_malformed_image_is_a_wall_on_record_and_an_error_on_replay_never_a_panic() {
        let d = scratch("malformed");
        for (i, (what, bytes)) in malformed().into_iter().enumerate() {
            let p = d.join(format!("bad{i}"));
            std::fs::write(&p, &bytes).unwrap();
            mode(&p, 0o755);
            match resolve(&call(p.to_str().unwrap()), retrace_box::SHARED_REGION_END_MIN) {
                Err(ExecRefusal::Wall(m)) => assert!(m.contains("load commands"), "{what}: {m}"),
                other => panic!("{what}: expected a wall, got {:?}", other.err()),
            }
            let e = image(&call("/x"), &bytes, &[], &bytes, &[]);
            assert!(e.is_err(), "{what}: replay's image() answers an error");
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
