use serde::{Serialize, Deserialize};
use std::fs::File;
use std::io::{self, Read, Write, BufWriter};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Regs { pub x: [u64;31], pub pc: u64, pub sp_el0: u64, pub cpsr: u64 }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region { pub ipa: u64, pub bytes: Vec<u8> }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
// exact shape is a cross-crate contract; boxing a variant would change the trace format
#[allow(clippy::large_enum_variant)]
pub enum Event {
    Snapshot { regs: Regs, mem: Vec<Region> },
    Syscall { num: u64, args: [u64;8], ret: u64, err: bool, writes: Vec<Region>, thread: u32 },
    Exit { code: u64 },
    Crash { pc: u64, esr: u64, far: u64 },
    /// M11: the guest raised a signal on itself whose disposition is the default fatal action.
    /// Terminal, exactly like `Crash` — followed by the final full-memory `Snapshot`.
    ///
    /// Deliberately NOT folded into `Crash` with a synthetic ESR: a signal is not a fault, and a
    /// SIGABRT printing as a fault bearing an ESR the hardware never produced is a lie the debug
    /// output would carry forever. `pc` names the raise site, which is what makes it useful.
    Signal { sig: u64, pc: u64 },
    /// M12: control transferred to one of the guest's own signal handlers.
    ///
    /// NOT terminal — the guest keeps running inside the handler, and a later `sigreturn`(184)
    /// syscall event brings it back. One shape for BOTH causes (a fault, and a self-raise via
    /// kill/__pthread_kill) so there is one seek target, one debug line, and one replay mirror.
    ///
    /// Deliberately a trace event rather than emulation hidden inside `Box_::run()`: symmetry rule
    /// 2's precedents (the timebase MRS, the undef-MRS, the FPAC strip) are INSTRUCTION emulations —
    /// micro, high-frequency, semantically invisible. Entering a handler is a control transfer, and
    /// "rewind to where the signal was delivered" is a query a reverse debugger's users have.
    ///
    /// `writes` carries the frame bytes; replay recomputes them and byte-compares before applying,
    /// the same posture as M11's `sigaction` oldact writeback. `resume_pc` is where the guest
    /// resumes on `sigreturn` — for a fault, the faulting instruction itself, which re-executes.
    SignalDelivery {
        sig: u64, si_code: u64, si_addr: u64, handler: u64, resume_pc: u64, writes: Vec<Region>,
    },
}

pub const TRACE_MAGIC: [u8;4] = *b"RT\x00\x07"; // "RT" + format version 0x0007 (M15: Syscall.thread)

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
        let mut inner = BufWriter::new(File::create(path)?);
        inner.write_all(&TRACE_MAGIC)?;
        Ok(Writer { inner })
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
        if buf.len() < 4 || buf[0..4] != TRACE_MAGIC {
            return Ok((Vec::new(), true)); // wrong/absent magic: reject loudly, keep nothing
        }
        let mut out = Vec::new();
        let mut off = 4usize;               // skip the 4-byte header
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
            Event::Syscall { num:3, args:[5,0x100000100,6,0,0,0,0,0], ret:6, err:false,
                             writes: vec![Region{ ipa:0x100000100, bytes: vec![9,9,9,9,9,9] }],
                             thread: 0 },
            Event::Exit { code:0 },
        ]
    }
    #[test]
    fn wrong_version_is_rejected() {
        let f = tempfile();
        std::fs::write(&f, b"XX\x00\x01some garbage").unwrap();
        let (got, truncated) = Reader::open_checked(&f).unwrap();
        assert!(truncated);
        assert!(got.is_empty());
    }
    #[test]
    fn rejects_prior_format_version() {
        // A genuine prior-version trace (RT\x00\x02) with an otherwise well-formed, correctly
        // CRC'd record: proves rejection is by MAGIC, not by CRC/framing.
        let f = tempfile();
        let prior_magic = b"RT\x00\x02";
        let body = b"plausible record body bytes";
        let crc = crc32(body);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(prior_magic);
        bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&crc.to_le_bytes());
        bytes.extend_from_slice(body);
        std::fs::write(&f, &bytes).unwrap();
        let (got, truncated) = Reader::open_checked(&f).unwrap();
        assert!(truncated);
        assert!(got.is_empty());
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
    fn crash_sample() -> Vec<Event> {
        vec![
            Event::Snapshot { regs: Regs { x:[0;31], pc:0x100000000, sp_el0:0x2000_0000, cpsr:0 },
                              mem: vec![Region{ ipa:0x100000000, bytes: vec![1,2,3,4] }] },
            Event::Crash { pc: 0x100000010, esr: 0x92000005, far: 0x4000DEAD0000 },
            Event::Snapshot { regs: Regs { x:[0;31], pc:0x100000010, sp_el0:0x2000_0000, cpsr:0 },
                              mem: vec![Region{ ipa:0x100000000, bytes: vec![1,2,3,9] }] },
        ]
    }
    #[test]
    fn crash_roundtrip() {
        let f = tempfile();
        let mut w = Writer::create(&f).unwrap();
        for e in crash_sample() { w.append(&e).unwrap(); }
        drop(w);
        let (got, truncated) = Reader::open_checked(&f).unwrap();
        assert!(!truncated);
        assert_eq!(got, crash_sample());
    }
    #[test]
    fn rejects_v3_traces() {
        // The M6 magic bump: a well-formed 0x03-era trace must be rejected wholesale.
        let f = tempfile();
        let body = b"plausible record body bytes";
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RT\x00\x03");
        bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&crc32(body).to_le_bytes());
        bytes.extend_from_slice(body);
        std::fs::write(&f, &bytes).unwrap();
        let (got, truncated) = Reader::open_checked(&f).unwrap();
        assert!(truncated);
        assert!(got.is_empty());
    }
    #[test]
    fn torn_crash_tail_recovers_prefix() {
        let f = tempfile();
        let mut w = Writer::create(&f).unwrap();
        for e in crash_sample() { w.append(&e).unwrap(); }
        drop(w);
        let mut bytes = std::fs::read(&f).unwrap();
        *bytes.last_mut().unwrap() ^= 0xff;
        std::fs::write(&f, &bytes).unwrap();
        let (got, truncated) = Reader::open_checked(&f).unwrap();
        assert!(truncated);
        assert_eq!(got, crash_sample()[..2].to_vec()); // torn final Snapshot dropped, Crash kept
    }
    #[test]
    fn signal_event_round_trips() {
        let p = named_tempfile("sigev");
        let mut w = Writer::create(&p).unwrap();
        w.append(&Event::Signal { sig: 6, pc: 0x1_0000 }).unwrap();
        drop(w);
        let (events, torn) = Reader::open_checked(&p).unwrap();
        assert!(!torn);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::Signal { sig, pc } => {
                assert_eq!(*sig, 6);
                assert_eq!(*pc, 0x1_0000);
            }
            other => panic!("expected Signal, got {other:?}"),
        }
        std::fs::remove_file(&p).ok();
    }

    // M11's magic assertion moved to `magic_bumped_for_the_syscall_thread_tag` when M15 bumped to
    // v7. What is left here is the half that does not go stale: a v4 trace stays rejected.
    #[test]
    fn rejects_v4_traces() {
        let p = named_tempfile("oldmagic");
        std::fs::write(&p, b"RT\x00\x04junkjunk").unwrap();
        let (events, torn) = Reader::open_checked(&p).unwrap();
        assert!(torn, "a v4 trace must be rejected, not misparsed");
        assert!(events.is_empty());
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn signal_delivery_round_trips_through_the_writer_and_reader() {
        let p = named_tempfile("sigdeliv");
        let ev = Event::SignalDelivery {
            sig: 11,
            si_code: 1,
            si_addr: 0xdead_0000,
            handler: 0x1_0000,
            resume_pc: 0x2_0000,
            writes: vec![Region { ipa: 0x7000_0000, bytes: vec![1, 2, 3, 4] }],
        };
        {
            let mut w = Writer::create(&p).unwrap();
            w.append(&ev).unwrap();
        }
        let (evs, torn) = Reader::open_checked(&p).unwrap();
        assert!(!torn);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0], ev);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn magic_bumped_for_the_syscall_thread_tag() {
        // M15 adds `thread` to Event::Syscall — a shape change, so old traces MUST be rejected
        // whole rather than misparsed.
        assert_eq!(TRACE_MAGIC, *b"RT\x00\x07");
    }

    #[test]
    fn a_trace_written_with_the_old_magic_is_rejected_whole() {
        let p = named_tempfile("v6magic");
        std::fs::write(&p, b"RT\x00\x06rest-of-a-v6-trace").unwrap();
        let (evs, rejected) = Reader::open_checked(&p).unwrap();
        assert!(evs.is_empty() && rejected, "a v6 trace must be rejected, not half-read");
        std::fs::remove_file(&p).ok();
    }

    fn named_tempfile(tag: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        // Deterministic per-test name; no clock/RNG (deny-list). The tag keeps these from
        // colliding with `tempfile()`, which reuses one path per process.
        p.push(format!("retrace-trace-test-{tag}-{}.bin", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn tempfile() -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        // Deterministic per-test name; no clock/RNG (deny-list).
        p.push(format!("retrace-trace-test-{}.bin", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }
}
