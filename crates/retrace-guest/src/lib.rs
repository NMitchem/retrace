#[derive(Debug, Clone)]
pub struct Segment { pub vaddr: u64, pub data: Vec<u8>, pub memsz: usize, pub exec: bool }
#[derive(Debug, Clone)]
pub struct Loaded { pub segments: Vec<Segment>, pub entry: u64, pub dylinker: Option<String> }

fn u32le(b: &[u8], o: usize) -> u32 { u32::from_le_bytes(b[o..o+4].try_into().unwrap()) }
fn u64le(b: &[u8], o: usize) -> u64 { u64::from_le_bytes(b[o..o+8].try_into().unwrap()) }

pub fn parse_macho(b: &[u8]) -> Loaded {
    assert_eq!(u32le(b, 0), 0xfeed_facf, "not a 64-bit Mach-O (MH_MAGIC_64)");
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
    Loaded { segments, entry: entry.expect("no LC_MAIN/LC_UNIXTHREAD entry point"), dylinker }
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
pub const DYLD_PATH: &str = "/usr/lib/dyld";
pub const STRIP47: &str = concat!(env!("OUT_DIR"), "/strip47");

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
}
