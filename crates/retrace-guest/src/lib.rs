#[derive(Debug, Clone)]
pub struct Segment { pub vaddr: u64, pub data: Vec<u8>, pub memsz: usize }
#[derive(Debug, Clone)]
pub struct Loaded { pub segments: Vec<Segment>, pub entry: u64 }

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
    for _ in 0..ncmds {
        let cmd = u32le(b, off);
        let cmdsize = u32le(b, off+4) as usize;
        match cmd {
            0x19 => { // LC_SEGMENT_64
                let vmaddr = u64le(b, off+24);
                let vmsize = u64le(b, off+32) as usize;
                let fileoff = u64le(b, off+40) as usize;
                let filesize = u64le(b, off+48) as usize;
                let name = &b[off+8..off+24];
                if name.starts_with(b"__TEXT") { text_vmaddr = vmaddr; text_fileoff = fileoff as u64; }
                if vmsize > 0 && name != b"__PAGEZERO\0\0\0\0\0\0" {
                    segments.push(Segment { vaddr: vmaddr, memsz: vmsize,
                        data: b[fileoff..fileoff+filesize].to_vec() });
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
            _ => {}
        }
        off += cmdsize;
    }
    Loaded { segments, entry: entry.expect("no LC_MAIN/LC_UNIXTHREAD entry point") }
}

pub const HELLO: &str = concat!(env!("OUT_DIR"), "/hello");
pub const FILEIO: &str = concat!(env!("OUT_DIR"), "/fileio");
pub const FIXTURE: &str = concat!(env!("OUT_DIR"), "/fixture.txt");
pub const MMAPGUEST: &str = concat!(env!("OUT_DIR"), "/mmapguest");

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
