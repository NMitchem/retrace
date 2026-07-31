#[derive(Debug, Clone)]
pub struct Segment { pub vaddr: u64, pub data: Vec<u8>, pub memsz: usize, pub exec: bool }
#[derive(Debug, Clone)]
pub struct Loaded { pub segments: Vec<Segment>, pub entry: u64, pub dylinker: Option<String>, pub cpusubtype: u32 }

fn u32le(b: &[u8], o: usize) -> u32 { u32::from_le_bytes(b[o..o+4].try_into().unwrap()) }
fn u64le(b: &[u8], o: usize) -> u64 { u64::from_le_bytes(b[o..o+8].try_into().unwrap()) }

pub fn parse_macho(b: &[u8]) -> Loaded {
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

pub fn slice_arm64e(fat: &[u8]) -> &[u8] {
    let magic = u32::from_le_bytes(fat[0..4].try_into().unwrap());
    if magic == 0xfeed_facf { return fat; }                         // already a thin 64-bit Mach-O
    let be32 = |o: usize| u32::from_be_bytes(fat[o..o+4].try_into().unwrap());
    let fatmagic = be32(0);
    let is64 = fatmagic == retrace_arch::FAT_MAGIC_64;
    assert!(fatmagic == retrace_arch::FAT_MAGIC || is64, "not a fat binary");
    let nfat = be32(4) as usize;
    // fat_arch{,_64}.offset is at struct byte 8 in BOTH layouts; only the stride differs (20 vs 32).
    let (entry_sz, off_field) = if is64 { (32usize, 8usize) } else { (20usize, 8usize) };
    for i in 0..nfat {
        let e = 8 + i * entry_sz;                                   // fat_arch[i]
        let cputype = be32(e);
        let cpusubtype = be32(e + 4);
        if cputype == retrace_arch::CPU_TYPE_ARM64
            && (cpusubtype & 0x00ff_ffff) == retrace_arch::CPU_SUBTYPE_ARM64E {
            let (off, size) = if is64 {
                (u64::from_be_bytes(fat[e+off_field..e+off_field+8].try_into().unwrap()) as usize,
                 u64::from_be_bytes(fat[e+off_field+8..e+off_field+16].try_into().unwrap()) as usize)
            } else {
                (be32(e + off_field) as usize, be32(e + off_field + 4) as usize)
            };
            return &fat[off..off + size];
        }
    }
    panic!("no arm64e slice in fat binary");
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
                   "hello_rust must be CPU_SUBTYPE_ARM64_ALL, got {:#x}", l.cpusubtype);
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
