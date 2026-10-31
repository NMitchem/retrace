//! M54 t3: the record-only host side of `execve`: resolve the path the way the kernel does and hand back
//! the bytes `Event::Exec` carries. Replay never calls `resolve` (spec 3c: replay takes the image, the
//! dylinker and the close set from the event and reads no file but the shared cache); both sides call
//! `image`, on the same bytes, to build what `Box_::exec_image` installs.
use retrace_arch::WallCategory;
use retrace_box::{ExecCall, ExecImage, ExecRefusal, EXE_BASE};
use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;

/// What `resolve` hands the record arm for `Event::Exec`: the selected slice of the executable, the
/// `#!` line (empty: task 5), and the selected slice of the image's dynamic linker.
pub struct Resolved { pub image: Vec<u8>, pub script: Vec<u8>, pub dylinker: Vec<u8> }

const ENOEXEC: u64 = 8;
const EACCES: u64 = 13;
const MH_EXECUTE: u32 = 2;
const CPU_TYPE_X86_64: u32 = 0x0100_0007;

fn wall(what: impl Into<String>) -> ExecRefusal { ExecRefusal::Wall(retrace_arch::wall(WallCategory::ProcessCreation, what)) }

fn errno_of(e: &std::io::Error) -> ExecRefusal { ExecRefusal::Errno(e.raw_os_error().unwrap_or(EACCES as i32) as u64) }

fn le32(b: &[u8], o: usize) -> u32 { u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) }
fn be32(b: &[u8], o: usize) -> u32 { u32::from_be_bytes(b[o..o + 4].try_into().unwrap()) }

/// The `(cputype, offset, size)` of each slice of a fat file, or `None` for a table that does not fit the
/// file (a `0xcafebabe` that is not a universal binary, such as a Java class, is not one).
fn fat_slices(b: &[u8]) -> Option<Vec<(u32, usize, usize)>> {
    let is64 = be32(b, 0) == retrace_arch::FAT_MAGIC_64;
    let (stride, n) = (if is64 { 32 } else { 20 }, be32(b, 4) as usize);
    if n > 64 || 8 + n * stride > b.len() { return None; }
    (0..n).map(|i| {
        let e = 8 + i * stride;
        let (off, size) = if is64 {
            (u64::from_be_bytes(b[e + 8..e + 16].try_into().unwrap()) as usize,
             u64::from_be_bytes(b[e + 16..e + 24].try_into().unwrap()) as usize)
        } else { (be32(b, e + 8) as usize, be32(b, e + 12) as usize) };
        (off.checked_add(size)? <= b.len()).then(|| (be32(b, e), off, size))
    }).collect()
}

/// The dynamic linker `image` names, read and sliced the way the start does (the CLI reads `/usr/lib/dyld`
/// and takes its arm64e slice).
fn dylinker_slice(path: &str) -> Result<Vec<u8>, ExecRefusal> {
    let bytes = std::fs::read(path)
        .map_err(|e| wall(format!("execve of an image whose dynamic linker {path} cannot be read ({e})")))?;
    if bytes.len() < 8 || (!bytes.starts_with(&[0xcf, 0xfa, 0xed, 0xfe]) && fat_slices(&bytes).is_none()) {
        return Err(wall(format!("execve of an image whose dynamic linker {path} is not a Mach-O")));
    }
    let slice = retrace_guest::try_slice_native(&bytes)
        .map_err(|why| wall(format!("execve of an image whose dynamic linker {path} {why}")))?;
    retrace_guest::try_parse_macho(slice)
        .map_err(|why| wall(format!("execve of an image whose dynamic linker {path} {why}")))?;
    Ok(slice.to_vec())
}

/// Resolve `call.path` as the kernel would, against the cwd, and check the file (spec 3e). Every error is
/// an errno the program is answered, or a wall by value; nothing here runs past the point of no return.
/// The order is the kernel's: the file first, the argument strings after (`ExecCall::args_refusal`).
pub fn resolve(call: &ExecCall) -> Result<Resolved, ExecRefusal> {
    let path = OsStr::from_bytes(&call.path);
    // Not `File::open` first: opening a FIFO blocks until a writer comes, and exec of one is EACCES.
    let md = std::fs::metadata(path).map_err(|e| errno_of(&e))?;
    if !md.is_file() { return Err(ExecRefusal::Errno(EACCES)); }
    let cpath = CString::new(call.path.clone()).map_err(|_| ExecRefusal::Errno(2))?;
    // SAFETY: access(2) reads a NUL-terminated path and writes nothing.
    if unsafe { libc::access(cpath.as_ptr(), libc::X_OK) } != 0 { return Err(errno_of(&std::io::Error::last_os_error())); }
    if md.permissions().mode() & 0o6000 != 0 {
        return Err(wall("execve of a setuid or setgid image (retrace cannot make the credential change)"));
    }
    let bytes = std::fs::read(path).map_err(|e| errno_of(&e))?;
    // `#!` scripts are task 5's; until then they are ENOEXEC, as a file that is not an image.
    if bytes.len() < 8 || bytes.starts_with(b"#!") { return Err(ExecRefusal::Errno(ENOEXEC)); }
    let fat = be32(&bytes, 0) == retrace_arch::FAT_MAGIC || be32(&bytes, 0) == retrace_arch::FAT_MAGIC_64;
    let thin = le32(&bytes, 0) == 0xfeed_facf;
    if !fat && !thin { return Err(ExecRefusal::Errno(ENOEXEC)); }
    let slice: &[u8] = if thin {
        match le32(&bytes, 4) {
            retrace_arch::CPU_TYPE_ARM64 => &bytes,
            CPU_TYPE_X86_64 => return Err(wall("execve of an x86_64-only image (needs Rosetta)")),
            t => return Err(wall(format!("execve of a Mach-O for CPU type {t:#x}, and retrace runs arm64 programs only"))),
        }
    } else {
        let Some(slices) = fat_slices(&bytes) else { return Err(ExecRefusal::Errno(ENOEXEC)) };
        match retrace_guest::try_slice_native(&bytes) {
            Ok(s) => s,
            Err(_) if slices.iter().any(|&(cpu, _, _)| cpu == CPU_TYPE_X86_64) =>
                return Err(wall("execve of an x86_64-only image (needs Rosetta)")),
            Err(_) => return Err(wall("execve of a universal binary with no arm64 slice, and retrace runs arm64 programs only")),
        }
    };
    if slice.len() < 32 || le32(slice, 0) != 0xfeed_facf || le32(slice, 12) != MH_EXECUTE { return Err(ExecRefusal::Errno(ENOEXEC)); }
    let loaded = retrace_guest::try_parse_macho(slice).map_err(|_| ExecRefusal::Errno(ENOEXEC))?;
    let Some(dyld) = loaded.dylinker.as_deref() else {
        return Err(wall("execve of a static image (no LC_LOAD_DYLINKER)"));
    };
    // C2: restore's posture, the symbols and the gdbserver all read the Mach-O header at EXE_BASE.
    if !loaded.segments.iter().any(|s| s.data.starts_with(&[0xcf, 0xfa, 0xed, 0xfe]) && s.vaddr == EXE_BASE) {
        return Err(wall(format!("execve of an image whose Mach-O header is not at {EXE_BASE:#x}")));
    }
    let dylinker = dylinker_slice(dyld)?;
    Ok(Resolved { image: slice.to_vec(), script: Vec::new(), dylinker })
}

/// What `Box_::exec_image` installs, built from the recorded slices: record's resolved bytes and replay's
/// event bytes both come through here, so both sides install the same thing. `Err` is a retrace bug on
/// record (it parsed the same bytes in `resolve`) and a divergence on replay.
pub fn image(call: &ExecCall, image: &[u8], dylinker: &[u8], closed: &[u32]) -> Result<ExecImage, String> {
    let exe = retrace_guest::try_parse_macho(image).map_err(|why| format!("the exec image {why}"))?;
    let dyld = retrace_guest::try_parse_macho(dylinker).map_err(|why| format!("the exec image's dynamic linker {why}"))?;
    Ok(ExecImage { exe, dylinker: dyld, argv: call.argv.clone(), envp: call.envp.clone(),
                   exec_path: call.path.clone(), closed: closed.to_vec(), spawn: call.spawn.clone() })
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
        assert_eq!(errno(resolve(&call(d.join("nope").to_str().unwrap()))), 2, "ENOENT");
        assert_eq!(errno(resolve(&call(d.to_str().unwrap()))), 13, "a directory is EACCES, as measured natively");
        // A path component that is a file: ENOTDIR.
        let f = d.join("f");
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(errno(resolve(&call(d.join("f/x").to_str().unwrap()))), 20, "ENOTDIR");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_file_without_the_execute_bit_is_eacces_and_garbage_is_enoexec() {
        let d = scratch("modes");
        let noexec = d.join("noexec");
        std::fs::write(&noexec, b"x\n").unwrap();
        mode(&noexec, 0o644);
        assert_eq!(errno(resolve(&call(noexec.to_str().unwrap()))), 13, "EACCES");
        let garbage = d.join("garbage");
        std::fs::write(&garbage, b"not a mach-o\n").unwrap();
        mode(&garbage, 0o755);
        assert_eq!(errno(resolve(&call(garbage.to_str().unwrap()))), 8, "ENOEXEC");
        // An empty file with the execute bit, and a Mach-O object or library (not MH_EXECUTE): ENOEXEC.
        let empty = d.join("empty");
        std::fs::write(&empty, b"").unwrap();
        mode(&empty, 0o755);
        assert_eq!(errno(resolve(&call(empty.to_str().unwrap()))), 8, "an empty file");
        let mut dylib = std::fs::read(retrace_guest::EXEC_TARGET_DYN).unwrap();
        dylib[12..16].copy_from_slice(&6u32.to_le_bytes()); // MH_DYLIB
        let lib = d.join("lib.dylib");
        std::fs::write(&lib, &dylib).unwrap();
        mode(&lib, 0o755);
        assert_eq!(errno(resolve(&call(lib.to_str().unwrap()))), 8, "a dylib is not an executable");
        // A `#!` script is ENOEXEC until task 5.
        let script = d.join("script");
        std::fs::write(&script, b"#!/bin/echo hi\n").unwrap();
        mode(&script, 0o755);
        assert_eq!(errno(resolve(&call(script.to_str().unwrap()))), 8, "a script, until task 5");
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
        let r = resolve(&call(both.to_str().unwrap())).expect("a fat file with both slices");
        assert_eq!(r.image, e, "the arm64e slice is preferred");
        assert!(r.script.is_empty());
        assert!(r.dylinker.len() > 1_000_000, "dyld's own selected slice travels with it");
        let only = d.join("only");
        std::fs::write(&only, fat(&[(0, &plain)])).unwrap();
        mode(&only, 0o755);
        let r = resolve(&call(only.to_str().unwrap())).expect("a fat file with an arm64 slice only");
        assert_eq!(r.image, plain, "else the arm64 slice");
        let _ = std::fs::remove_dir_all(&d);
    }
}
