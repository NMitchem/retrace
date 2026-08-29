#[derive(Debug, Clone)]
pub struct Segment { pub vaddr: u64, pub data: Vec<u8>, pub memsz: usize, pub exec: bool }
#[derive(Debug, Clone)]
pub struct Loaded { pub segments: Vec<Segment>, pub entry: u64, pub dylinker: Option<String>, pub cpusubtype: u32 }

fn u32le(b: &[u8], o: usize) -> u32 { u32::from_le_bytes(b[o..o+4].try_into().unwrap()) }
fn u64le(b: &[u8], o: usize) -> u64 { u64::from_le_bytes(b[o..o+8].try_into().unwrap()) }

pub fn parse_macho(b: &[u8]) -> Loaded {
    // A universal (fat) file is the normal shape for a macOS system binary and for dyld itself,
    // so pick the slice this machine would execute before reading a mach_header out of it. A thin
    // file passes through, and so does a non-Mach-O — the assert below stays the single place a
    // file that is neither is rejected, with the message that says so.
    let b = slice_native(b);
    assert_eq!(u32le(b, 0), 0xfeed_facf, "not a 64-bit Mach-O (MH_MAGIC_64)");
    // mach_header_64: magic(0) cputype(4) cpusubtype(8). The low 24 bits are the subtype proper;
    // the top 8 are capability bits (arm64e carries a ptrauth ABI version there). The box derives
    // the guest's PAC posture from this — macOS enables PAC per process, only for arm64e mains.
    let cpusubtype = u32le(b, 8);
    let ncmds = u32le(b, 16);
    let mut off = 32usize; // mach_header_64 is 32 bytes
    let mut segments = Vec::new();
    let mut text_vmaddr = 0u64;
    let mut text_fileoff = 0u64;
    let mut entry: Option<u64> = None;
    let mut dylinker: Option<String> = None;
    for _ in 0..ncmds {
        let cmd = u32le(b, off);
        let cmdsize = u32le(b, off+4) as usize;
        match cmd {
            0x19 => { // LC_SEGMENT_64
                let vmaddr = u64le(b, off+24);
                let vmsize = u64le(b, off+32) as usize;
                let fileoff = u64le(b, off+40) as usize;
                let filesize = u64le(b, off+48) as usize;
                let initprot = u32le(b, off+60); // initprot (maxprot is off+56); VM_PROT_EXECUTE = 0x4
                let name = &b[off+8..off+24];
                if name.starts_with(b"__TEXT") { text_vmaddr = vmaddr; text_fileoff = fileoff as u64; }
                if vmsize > 0 && name != b"__PAGEZERO\0\0\0\0\0\0" {
                    segments.push(Segment { vaddr: vmaddr, memsz: vmsize,
                        data: b[fileoff..fileoff+filesize].to_vec(), exec: initprot & 0x4 != 0 });
                }
            }
            0x80000028 => { // LC_MAIN: entryoff is file offset from start of file
                let entryoff = u64le(b, off+8);
                entry = Some(text_vmaddr + (entryoff - text_fileoff));
            }
            0x5 => { // LC_UNIXTHREAD: arm64 thread state; PC is at a fixed offset
                // flavor(4) count(4) then 34 u64 GPRs; PC is register index 32.
                let pc = u64le(b, off + 16 + 32*8);
                entry = Some(pc);
            }
            retrace_arch::LC_LOAD_DYLINKER => {
                // dylinker_command: cmd(4) cmdsize(4) name.offset(4) then the NUL-terminated path.
                let name_off = off + u32le(b, off+8) as usize;
                let end = (off + cmdsize).min(b.len());
                let nul = (name_off..end).find(|&i| b[i] == 0).unwrap_or(end);
                dylinker = Some(String::from_utf8_lossy(&b[name_off..nul]).into_owned());
            }
            _ => {}
        }
        off += cmdsize;
    }
    Loaded { segments, entry: entry.expect("no LC_MAIN/LC_UNIXTHREAD entry point"), dylinker, cpusubtype }
}

/// True for a thin 64-bit Mach-O — the shape every slice picker passes through untouched.
fn is_thin(b: &[u8]) -> bool { u32::from_le_bytes(b[0..4].try_into().unwrap()) == 0xfeed_facf }

fn is_fat(b: &[u8]) -> bool {
    let m = u32::from_be_bytes(b[0..4].try_into().unwrap());
    m == retrace_arch::FAT_MAGIC || m == retrace_arch::FAT_MAGIC_64
}

/// The `CPU_TYPE_ARM64` slice whose cpusubtype (low 24 bits) is `want_sub`, or `None`.
///
/// Fat headers and their `fat_arch` tables are BIG-endian on disk — the one place in a Mach-O
/// where that is true. `fat_arch{,_64}.offset` sits at struct byte 8 in both layouts; only the
/// stride (20 vs 32) and the field width (u32 vs u64) differ.
fn fat_find(fat: &[u8], want_sub: u32) -> Option<&[u8]> {
    let be32 = |o: usize| u32::from_be_bytes(fat[o..o + 4].try_into().unwrap());
    let is64 = be32(0) == retrace_arch::FAT_MAGIC_64;
    let entry_sz = if is64 { 32usize } else { 20usize };
    (0..be32(4) as usize).find_map(|i| {
        let e = 8 + i * entry_sz;
        if be32(e) != retrace_arch::CPU_TYPE_ARM64 || be32(e + 4) & 0x00ff_ffff != want_sub {
            return None;
        }
        let (off, size) = if is64 {
            (u64::from_be_bytes(fat[e + 8..e + 16].try_into().unwrap()) as usize,
             u64::from_be_bytes(fat[e + 16..e + 24].try_into().unwrap()) as usize)
        } else {
            (be32(e + 8) as usize, be32(e + 12) as usize)
        };
        Some(&fat[off..off + size])
    })
}

pub fn slice_arm64e(fat: &[u8]) -> &[u8] {
    if is_thin(fat) { return fat; }
    assert!(is_fat(fat), "not a fat binary");
    fat_find(fat, retrace_arch::CPU_SUBTYPE_ARM64E).expect("no arm64e slice in fat binary")
}

/// The slice this machine would actually execute: arm64e if the file carries one, else plain
/// arm64. A thin Mach-O — and anything that is not a fat file at all — passes through unchanged,
/// so a caller never has to know which shape it holds.
///
/// The preference order is load-bearing, not cosmetic. `cpusubtype` is what `pac_posture` reads,
/// so taking the plain-arm64 slice of a file that carries both would run an arm64e guest with PAC
/// turned off — M7's wall, reached by a different road.
pub fn slice_native(fat: &[u8]) -> &[u8] {
    if is_thin(fat) || !is_fat(fat) { return fat; }
    fat_find(fat, retrace_arch::CPU_SUBTYPE_ARM64E)
        .or_else(|| fat_find(fat, retrace_arch::CPU_SUBTYPE_ARM64_ALL))
        .expect("fat binary carries no arm64e or arm64 slice (retrace runs arm64 guests only)")
}

pub const HELLO: &str = concat!(env!("OUT_DIR"), "/hello");
pub const STEPPY: &str = concat!(env!("OUT_DIR"), "/steppy");
pub const FILEIO: &str = concat!(env!("OUT_DIR"), "/fileio");
pub const FIXTURE: &str = concat!(env!("OUT_DIR"), "/fixture.txt");
pub const MMAPGUEST: &str = concat!(env!("OUT_DIR"), "/mmapguest");
pub const UNALIGNED: &str = concat!(env!("OUT_DIR"), "/unaligned");
pub const PACGUEST: &str = concat!(env!("OUT_DIR"), "/pacguest");
pub const FAILSYS: &str = concat!(env!("OUT_DIR"), "/failsys");
pub const REMAP: &str = concat!(env!("OUT_DIR"), "/remap");
pub const MMAPFILE: &str = concat!(env!("OUT_DIR"), "/mmapfile");
pub const MMAPFILE_FIXTURE: &str = concat!(env!("OUT_DIR"), "/mmapfile_fixture.txt");
pub const EXECMAP: &str = concat!(env!("OUT_DIR"), "/execmap");
pub const EXECMAP_FIXTURE: &str = concat!(env!("OUT_DIR"), "/execmap_fixture.bin");
pub const MACHMSG: &str = concat!(env!("OUT_DIR"), "/machmsg");
pub const HELLO_DYN: &str = concat!(env!("OUT_DIR"), "/hello_dyn");
pub const CRASHY: &str = concat!(env!("OUT_DIR"), "/crashy");

/// The M18 fast-follow crash fixture: threaded AND fatal. Main blocks in `pthread_join`, the child
/// runs and faults at `0x4000_DEAD_0000` with no handler installed, so the terminal `Event::Crash`
/// is tagged with the CHILD. Exists to exercise the one `verify_thread` site that had no fixture.
pub const CRASHTHREAD: &str = concat!(env!("OUT_DIR"), "/crashthread");
/// M12: catches SIGSEGV through Apple's real `_sigtramp` (libc's `sigaction()` installs its own
/// `sa_tramp`) — the only guest that exercises the trampoline that actually ships.
pub const SIGCATCH_DYN: &str = concat!(env!("OUT_DIR"), "/sigcatch_dyn");
pub const DYLD_PATH: &str = "/usr/lib/dyld";
pub const STRIP47: &str = concat!(env!("OUT_DIR"), "/strip47");
pub const BFAMSTRIP: &str = concat!(env!("OUT_DIR"), "/bfamstrip");
pub const RESERVECOMMIT: &str = concat!(env!("OUT_DIR"), "/reservecommit");
pub const WILDSTORE: &str = concat!(env!("OUT_DIR"), "/wildstore");
pub const CARVEOUT: &str = concat!(env!("OUT_DIR"), "/carveout");
pub const SPINLOOP: &str = concat!(env!("OUT_DIR"), "/spinloop");
pub const WATCHLOOP: &str = concat!(env!("OUT_DIR"), "/watchloop");
pub const CRASH: &str = concat!(env!("OUT_DIR"), "/crash");
pub const CRASHJMP: &str = concat!(env!("OUT_DIR"), "/crashjmp");
pub const HELLO_RUST: &str = concat!(env!("OUT_DIR"), "/hello_rust");
pub const USRSTACK: &str = concat!(env!("OUT_DIR"), "/usrstack");
pub const FIXEDINNER: &str = concat!(env!("OUT_DIR"), "/fixedinner");
pub const WILDFIXED: &str = concat!(env!("OUT_DIR"), "/wildfixed");
pub const ARGV_ECHO: &str = concat!(env!("OUT_DIR"), "/argv_echo");
pub const STDIO_DYN: &str = concat!(env!("OUT_DIR"), "/stdio_dyn");
pub const CLOSEFD_DYN: &str = concat!(env!("OUT_DIR"), "/closefd_dyn");
/// M10: opens, dups, closes and re-opens, printing each descriptor it is given — so the e2e can
/// assert the guest sees ITS OWN fd numbers (3, 4, …) rather than retrace's host ones (17+).
pub const FDTABLE_DYN: &str = concat!(env!("OUT_DIR"), "/fdtable_dyn");
/// M11 headline: a full-std Rust binary that `panic!()`s into `abort()`/SIGABRT (`-C panic=abort`).
pub const PANICKY: &str = concat!(env!("OUT_DIR"), "/panicky");
/// M12 headline: a stock full-`std` Rust binary that faults on a wild pointer, so libstd's own
/// SIGSEGV handler runs, resets to `SIG_DFL` and returns, and the re-executed store kills it.
pub const SEGVY: &str = concat!(env!("OUT_DIR"), "/segvy");
/// M11: `kill(getpid(), SIGABRT)` — the terminal raise mechanism.
pub const RAISE: &str = concat!(env!("OUT_DIR"), "/raise");
/// M11: `SIG_IGN` then raise then `write("ok\n")` — the non-terminal branch.
pub const SIGIGN: &str = concat!(env!("OUT_DIR"), "/sigign");
/// M11: `kill(1, SIGKILL)` — the safety boundary the recorder must refuse.
pub const KILLOTHER: &str = concat!(env!("OUT_DIR"), "/killother");

// M12 delivery fixtures. Each supplies its OWN trampoline, so they exercise retrace's entry
// contract directly rather than through libc's `_sigtramp`.
/// M12: validates every entry register retrace promises; a distinct exit code per failed check.
pub const SIGFRAME: &str = concat!(env!("OUT_DIR"), "/sigframe");
/// M12: faults, and the handler advances `__ss.__pc` past the store — so continuing at all proves
/// `sigreturn` restored MUTATED state rather than the state captured at delivery.
pub const SEGVCATCH: &str = concat!(env!("OUT_DIR"), "/segvcatch");
/// M12: `SA_ONSTACK` + `sigaltstack`; the handler checks its own `sp` is inside the alt stack.
pub const ALTSTACK: &str = concat!(env!("OUT_DIR"), "/altstack");
/// M12: the handler clobbers `v8`, so only a real vector restore lets this exit 0.
pub const VECSURVIVE: &str = concat!(env!("OUT_DIR"), "/vecsurvive");
/// M12: faults with `SIGSEGV` blocked — the fail-loud fixture; it never exits cleanly by design.
pub const BLOCKEDFAULT: &str = concat!(env!("OUT_DIR"), "/blockedfault");

/// M13 t7: mmaps RW, touches it (populates the TLB), `mprotect`s PROT_NONE, then stores again —
/// which must take a stage-1 permission fault. Exits 7 if the store wrongly succeeds.
pub const PROTNONE: &str = concat!(env!("OUT_DIR"), "/protnone");
/// M13 t7: the restore direction — PROT_NONE then back to RW, proving `unprotect`'s flush too.
pub const PROTRESTORE: &str = concat!(env!("OUT_DIR"), "/protrestore");
/// M13 t9: the `protnone` twin that protects through `mach_vm_protect` (svc −14) instead of
/// `mprotect` (74) — the dispatch arm that returned KERN_SUCCESS without touching the box.
pub const PROTNONE_MACH: &str = concat!(env!("OUT_DIR"), "/protnone_mach");
/// M13 t10: reserves PROT_NONE address space and `mprotect`s a page inside it that was never
/// committed — the fail-loud negative. The box must assert (no-access implies backed), not succeed.
pub const PROTRESERVE: &str = concat!(env!("OUT_DIR"), "/protreserve");
/// M13 t11 headline: a full-`std` Rust binary that `mprotect`s one of its own pages PROT_NONE and
/// stores through it — enforcement proved through real libc, real dyld and libstd's own handlers.
pub const PROTRUST: &str = concat!(env!("OUT_DIR"), "/protrust");
/// M14 t11 headline: a full-`std` Rust binary that `std::thread::spawn`s a child and `join`s it.
/// `joined 42` is the load-bearing output — it requires the child to have RUN on retrace's single
/// vCPU under the cooperative scheduler AND its return value to have crossed back through join.
pub const THREADRUST: &str = concat!(env!("OUT_DIR"), "/threadrust");
/// M15 t9 headline: the `threadrust` twin whose two threads write DIFFERENT static cells, so a
/// hardware watch on one and not the other makes thread attribution a claim that can be wrong.
pub const WATCHTHREAD: &str = concat!(env!("OUT_DIR"), "/watchthread");
/// M16 t5 headline: a full-`std` Rust binary whose MAIN thread `pthread_kill`s its CHILD by name
/// while the child is Runnable-but-not-current. Today retrace ignores the target port and
/// delivers to whoever is running (main); M16 fixes attribution so the child takes it instead.
pub const SIGTHREAD: &str = concat!(env!("OUT_DIR"), "/sigthread");
/// M16 t13, un-parked by M17: the guest for the `sigblocked_e2e` gate — three threads so that `b`
/// can signal `a` while `a` is genuinely BLOCKED in `__ulock_wait` (joining `b`), not merely
/// Runnable-and-never-run. The cooperative scheduler switches only on block or exit, so main can
/// never observe a blocked peer directly; a blocked joiner leaves its own joinee running instead.
pub const SIGBLOCKED: &str = concat!(env!("OUT_DIR"), "/sigblocked");
/// M13 t11: the recursion behind the PARKED `stackoverflow_rust_e2e` gate. Cannot strike libstd's
/// guard today — that guard sits 7.73 MiB below retrace's real stack bottom (M8 spec risk R3).
pub const OVERFLOW: &str = concat!(env!("OUT_DIR"), "/overflow");
pub const TLBIEXEC: &str = concat!(env!("OUT_DIR"), "/tlbiexec");
pub const TLBIEXEC_FIXTURE: &str = concat!(env!("OUT_DIR"), "/tlbiexec_fixture.bin");
/// M18 rung 5: a dynamically-linked C guest that `dispatch_async`es onto a global queue.
pub const DISPATCH_DYN: &str = concat!(env!("OUT_DIR"), "/dispatch_dyn");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_the_hello_guest() {
        let bytes = std::fs::read(HELLO).unwrap();
        let l = parse_macho(&bytes);
        assert!(!l.segments.is_empty(), "must find LC_SEGMENT_64");
        // entry must fall inside an executable segment's mapped range
        assert!(l.segments.iter().any(|s| l.entry >= s.vaddr && l.entry < s.vaddr + s.memsz as u64),
                "entry 0x{:x} not inside any segment", l.entry);
        // __DATA must contain "hello\n" somewhere
        assert!(l.segments.iter().any(|s| s.data.windows(6).any(|w| w == b"hello\n")));
    }

    #[test]
    fn fileio_guest_parses() {
        let l = parse_macho(&std::fs::read(FILEIO).unwrap());
        assert!(l.segments.iter().any(|s| l.entry >= s.vaddr && l.entry < s.vaddr + s.memsz as u64));
        assert_eq!(std::fs::read(FIXTURE).unwrap(), b"retrace-m1-fixture\n");
    }

    #[test]
    fn spinloop_guest_parses() {
        let l = parse_macho(&std::fs::read(SPINLOOP).unwrap());
        assert!(l.segments.iter().any(|s| l.entry >= s.vaddr && l.entry < s.vaddr + s.memsz as u64));
        assert!(l.segments.iter().any(|s| s.data.windows(6).any(|w| w == b"spin!\n")));
    }

    #[test]
    fn watchloop_guest_parses() {
        let l = parse_macho(&std::fs::read(WATCHLOOP).unwrap());
        assert!(l.segments.iter().any(|s| l.entry >= s.vaddr && l.entry < s.vaddr + s.memsz as u64));
    }

    #[test]
    fn parse_macho_surfaces_cpusubtype() {
        // The guest's PAC posture is derived from this field (M7 t6): macOS enables PAC per process
        // only for arm64e main executables. Every guest this repo builds is plain arm64.
        let l = parse_macho(&std::fs::read(HELLO_RUST).unwrap());
        assert_eq!(l.cpusubtype & 0x00ff_ffff, 0,
                   "hello_rust must be retrace_arch::CPU_SUBTYPE_ARM64_ALL, got {:#x}", l.cpusubtype);
        assert_ne!(l.cpusubtype & 0x00ff_ffff, retrace_arch::CPU_SUBTYPE_ARM64E,
                   "hello_rust is not arm64e — the ladder's premise is self-built arm64 binaries");
    }

    #[test]
    fn hello_rust_guest_parses() {
        let l = parse_macho(&std::fs::read(HELLO_RUST).unwrap());
        assert!(l.segments.iter().any(|s| l.entry >= s.vaddr && l.entry < s.vaddr + s.memsz as u64),
                "entry 0x{:x} not inside any segment", l.entry);
        // Rung 1's whole premise is a real dynamic binary through the real dynamic linker.
        assert_eq!(l.dylinker.as_deref(), Some("/usr/lib/dyld"),
                   "hello_rust must be dynamically linked through real dyld");
    }
}

#[cfg(test)]
mod fat_tests {
    use super::*;

    const CPU_TYPE_X86_64: u32 = 0x0100_0007;

    /// Build a `FAT_MAGIC` (32-bit) universal wrapper around `slices`, laid out the way `lipo`
    /// lays one out: a big-endian header, a `fat_arch[]` table, then each slice padded to its
    /// 2^14 alignment. Synthetic because no Apple binary carries the arm64e+arm64 pair the
    /// preference rule below turns on, so only a fixture can exercise it.
    fn fat_wrap(slices: &[(u32, u32, &[u8])]) -> Vec<u8> {
        let n = slices.len();
        let mut hdr = Vec::new();
        hdr.extend_from_slice(&retrace_arch::FAT_MAGIC.to_be_bytes());
        hdr.extend_from_slice(&(n as u32).to_be_bytes());
        let align = 0x4000usize;
        let mut off = (8 + n * 20 + align - 1) & !(align - 1);
        let mut offs = Vec::new();
        for (ct, cs, b) in slices {
            hdr.extend_from_slice(&ct.to_be_bytes());
            hdr.extend_from_slice(&cs.to_be_bytes());
            hdr.extend_from_slice(&(off as u32).to_be_bytes());
            hdr.extend_from_slice(&(b.len() as u32).to_be_bytes());
            hdr.extend_from_slice(&14u32.to_be_bytes()); // align = 2^14
            offs.push(off);
            off = (off + b.len() + align - 1) & !(align - 1);
        }
        let mut out = hdr;
        for (i, (_, _, b)) in slices.iter().enumerate() {
            out.resize(offs[i], 0);
            out.extend_from_slice(b);
        }
        out
    }

    #[test]
    fn parse_macho_accepts_a_fat_binary() {
        // Every macOS system binary — and /usr/lib/dyld itself — ships universal (x86_64 +
        // arm64e). Parsing one must select the arm64e slice rather than reject the fat header,
        // and must land on exactly what explicit slicing already produces.
        let bytes = std::fs::read(DYLD_PATH).unwrap();
        assert_ne!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), 0xfeed_facf,
                   "{DYLD_PATH} is thin on this machine, so this test proves nothing");
        let via_fat = parse_macho(&bytes);
        let via_slice = parse_macho(slice_arm64e(&bytes));
        assert_eq!(via_fat.entry, via_slice.entry, "fat parse must reach the same entry");
        assert_eq!(via_fat.cpusubtype, via_slice.cpusubtype);
        assert_eq!(via_fat.segments.len(), via_slice.segments.len());
    }

    #[test]
    fn fat_parse_prefers_arm64e_over_plain_arm64() {
        // The subtype chosen here is what `pac_posture` reads, so picking the plain-arm64 slice
        // of a dual binary would silently run an arm64e guest with PAC OFF — M7's wall, back
        // again by a different route. arm64 is listed FIRST so a first-match loop picks wrong.
        let hello = std::fs::read(HELLO).unwrap();
        let mut e = hello.clone();
        e[8..12].copy_from_slice(&retrace_arch::CPU_SUBTYPE_ARM64E.to_le_bytes());
        let fat = fat_wrap(&[
            (retrace_arch::CPU_TYPE_ARM64, retrace_arch::CPU_SUBTYPE_ARM64_ALL, &hello),
            (retrace_arch::CPU_TYPE_ARM64, retrace_arch::CPU_SUBTYPE_ARM64E, &e),
        ]);
        assert_eq!(parse_macho(&fat).cpusubtype & 0x00ff_ffff, retrace_arch::CPU_SUBTYPE_ARM64E,
                   "a fat binary carrying both arm64 slices must yield the arm64e one");
    }

    #[test]
    fn fat_parse_falls_back_to_plain_arm64() {
        // The Homebrew universal shape: x86_64 + arm64, no arm64e. `slice_arm64e` panics on
        // this file; parsing must not.
        let hello = std::fs::read(HELLO).unwrap();
        let fat = fat_wrap(&[
            (CPU_TYPE_X86_64, 3, &[0u8; 64][..]),
            (retrace_arch::CPU_TYPE_ARM64, retrace_arch::CPU_SUBTYPE_ARM64_ALL, &hello),
        ]);
        assert_eq!(parse_macho(&fat).cpusubtype & 0x00ff_ffff, retrace_arch::CPU_SUBTYPE_ARM64_ALL,
                   "an x86_64+arm64 universal must yield the plain-arm64 slice");
    }
}
