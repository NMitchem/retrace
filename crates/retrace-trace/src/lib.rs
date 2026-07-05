use serde::{Serialize, Deserialize};
use std::fs::File;
use std::io::{self, Read, Write, BufWriter};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Regs { pub x: [u64;31], pub pc: u64, pub sp_el0: u64, pub cpsr: u64 }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region { pub ipa: u64, pub bytes: Vec<u8> }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    Snapshot { regs: Regs, mem: Vec<Region> },
    Syscall { num: u64, args: [u64;7], ret: u64 },
    Sched { thread: u32, until: u64 },
    Exit { code: u64 },
}

// Minimal in-tree CRC32 (IEEE) — no external checksum dependency.
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xffff_ffff;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 { crc = (crc >> 1) ^ (0xedb8_8320 & (!(crc & 1)).wrapping_add(1)); }
    }
    !crc
}

pub struct Writer { inner: BufWriter<File> }
impl Writer {
    pub fn create<P: AsRef<Path>>(path: P) -> io::Result<Writer> {
        Ok(Writer { inner: BufWriter::new(File::create(path)?) })
    }
    pub fn append(&mut self, e: &Event) -> io::Result<()> {
        let body = bincode::serialize(e).expect("serialize event");
        let len = body.len() as u32;
        let crc = crc32(&body);
        self.inner.write_all(&len.to_le_bytes())?;
        self.inner.write_all(&crc.to_le_bytes())?;
        self.inner.write_all(&body)?;
        self.inner.flush()
    }
}

pub struct Reader;
impl Reader {
    pub fn open_checked<P: AsRef<Path>>(path: P) -> io::Result<(Vec<Event>, bool)> {
        let mut buf = Vec::new();
        File::open(path)?.read_to_end(&mut buf)?;
        let mut out = Vec::new();
        let mut off = 0usize;
        let mut truncated = false;
        while off + 8 <= buf.len() {
            let len = u32::from_le_bytes(buf[off..off+4].try_into().unwrap()) as usize;
            let crc = u32::from_le_bytes(buf[off+4..off+8].try_into().unwrap());
            let start = off + 8;
            if start + len > buf.len() { truncated = true; break; }        // torn length
            let body = &buf[start..start+len];
            if crc32(body) != crc { truncated = true; break; }              // torn body
            match bincode::deserialize::<Event>(body) {
                Ok(e) => out.push(e),
                Err(_) => { truncated = true; break; }
            }
            off = start + len;
        }
        if off != buf.len() { truncated = true; }
        Ok((out, truncated))
    }
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Vec<Event>> {
        Ok(Self::open_checked(path)?.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Vec<Event> {
        vec![
            Event::Snapshot { regs: Regs { x:[0;31], pc:0x100000000, sp_el0:0x2000_0000, cpsr:0 },
                              mem: vec![Region{ ipa:0x100000000, bytes: vec![1,2,3,4] }] },
            Event::Syscall { num:4, args:[1,0x100000100,6,0,0,0,0], ret:6 },
            Event::Exit { code:0 },
        ]
    }
    #[test]
    fn roundtrip() {
        let f = tempfile();
        let mut w = Writer::create(&f).unwrap();
        for e in sample() { w.append(&e).unwrap(); }
        drop(w);
        let (got, truncated) = Reader::open_checked(&f).unwrap();
        assert!(!truncated);
        assert_eq!(got, sample());
    }
    #[test]
    fn torn_tail_recovers_prefix() {
        let f = tempfile();
        let mut w = Writer::create(&f).unwrap();
        for e in sample() { w.append(&e).unwrap(); }
        drop(w);
        // Corrupt the last byte on disk: the final record must be rejected, prefix kept.
        let mut bytes = std::fs::read(&f).unwrap();
        *bytes.last_mut().unwrap() ^= 0xff;
        std::fs::write(&f, &bytes).unwrap();
        let (got, truncated) = Reader::open_checked(&f).unwrap();
        assert!(truncated);
        assert_eq!(got, sample()[..2].to_vec()); // Exit record dropped
    }
    fn tempfile() -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        // Deterministic per-test name; no clock/RNG (deny-list).
        p.push(format!("retrace-trace-test-{}.bin", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }
}
