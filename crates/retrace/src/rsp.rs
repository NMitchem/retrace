//! M43: the gdb-remote serial protocol, pure: framing, hex, the register description, stop replies
//! and the image list (spec `docs/superpowers/specs/2026-09-25-retrace-m43-lldb-design.md` §3a).
//! No VM and no socket: `gdbserver.rs` owns those, so everything here is unit-tested without either.
use retrace_core::{Outcome, ThreadCtx};

/// One unit off the wire.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    /// A packet's payload, checksum verified and `}` escapes undone.
    Packet(String),
    Ack,
    Nak,
    /// The raw 0x03 byte. §3h: ignored outside a motion.
    Interrupt,
    /// A packet whose checksum did not match.
    Bad,
}

/// Incremental framing: `push` whatever one read returned, then take frames until `next` says it
/// needs more bytes. One read may hold half a packet, or several (Review Focus 1).
#[derive(Default)]
pub(crate) struct Decoder { buf: Vec<u8> }

impl Decoder {
    pub(crate) fn push(&mut self, bytes: &[u8]) { self.buf.extend_from_slice(bytes); }

    /// The next whole frame, or None until more bytes arrive. Bytes that start no frame are dropped.
    pub(crate) fn next(&mut self) -> Option<Frame> {
        loop {
            let &b = self.buf.first()?;
            match b {
                b'+' => { self.buf.remove(0); return Some(Frame::Ack); }
                b'-' => { self.buf.remove(0); return Some(Frame::Nak); }
                0x03 => { self.buf.remove(0); return Some(Frame::Interrupt); }
                b'$' => {
                    // A `#` inside a payload is always escaped (`}` then 0x03), so the first raw `#`
                    // ends it.
                    let hash = self.buf.iter().position(|&c| c == b'#')?;
                    if self.buf.len() < hash + 3 { return None; }
                    let body = self.buf[1..hash].to_vec();
                    let cc = std::str::from_utf8(&self.buf[hash + 1..hash + 3]).ok()
                        .and_then(|s| u8::from_str_radix(s, 16).ok());
                    self.buf.drain(..hash + 3);
                    let sum = body.iter().fold(0u8, |a, &c| a.wrapping_add(c));
                    return Some(if cc == Some(sum) {
                        Frame::Packet(String::from_utf8_lossy(&unescape(&body)).into_owned())
                    } else {
                        Frame::Bad
                    });
                }
                _ => { self.buf.remove(0); }
            }
        }
    }
}

/// `$<payload>#<checksum>`, escaping `#`, `$`, `}` and `*` as `}` + (byte ^ 0x20). The JSON image
/// list is full of `}`, so every reply goes through this.
pub(crate) fn encode(payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(payload.len() + 8);
    for &c in payload {
        if matches!(c, b'#' | b'$' | b'}' | b'*') { body.push(b'}'); body.push(c ^ 0x20); } else { body.push(c); }
    }
    let sum = body.iter().fold(0u8, |a, &c| a.wrapping_add(c));
    let mut out = Vec::with_capacity(body.len() + 4);
    out.push(b'$');
    out.extend_from_slice(&body);
    out.push(b'#');
    out.extend_from_slice(format!("{sum:02x}").as_bytes());
    out
}

/// Undo `}` escapes.
fn unescape(p: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(p.len());
    let mut it = p.iter();
    while let Some(&c) = it.next() {
        if c == b'}' { if let Some(&n) = it.next() { out.push(n ^ 0x20); } } else { out.push(c); }
    }
    out
}

pub(crate) fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() }

pub(crate) fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) { return None; }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// §3g: target.xml's register order. x0-x28, fp, lr, sp, pc, cpsr (32 bits), v0-v31, fpsr, fpcr.
pub(crate) const NREGS: usize = 68;

/// §3g: register `n` of `ctx`, little-endian, in target.xml's order. `cpsr` is `regs.cpsr`, never
/// `spsr`: a blocked thread's `spsr` is raw exception-entry state (t0 R1).
pub(crate) fn reg_bytes(ctx: &ThreadCtx, n: usize) -> Option<Vec<u8>> {
    Some(match n {
        0..=30 => ctx.regs.x[n].to_le_bytes().to_vec(),
        31 => ctx.regs.sp_el0.to_le_bytes().to_vec(),
        32 => ctx.regs.pc.to_le_bytes().to_vec(),
        33 => (ctx.regs.cpsr as u32).to_le_bytes().to_vec(),
        34..=65 => ctx.fp[n - 34].to_le_bytes().to_vec(),
        66 => (ctx.fpsr as u32).to_le_bytes().to_vec(),
        67 => (ctx.fpcr as u32).to_le_bytes().to_vec(),
        _ => return None,
    })
}

/// `g`: every register, in order.
pub(crate) fn all_regs_hex(ctx: &ThreadCtx) -> String {
    (0..NREGS).map(|n| hex(&reg_bytes(ctx, n).expect("n < NREGS"))).collect()
}

/// §3g: the aarch64 register description, in the shape t0's stub served and lldb-2100 accepted (t0
/// L1: GPRs and FP, `generic` names, lldb derives the `w` registers itself).
pub(crate) fn target_xml() -> String {
    let mut x = vec![
        r#"<?xml version="1.0"?>"#.to_string(),
        r#"<!DOCTYPE target SYSTEM "gdb-target.dtd">"#.to_string(),
        r#"<target version="1.0">"#.to_string(),
        "<architecture>aarch64</architecture>".to_string(),
        r#"<feature name="org.gnu.gdb.aarch64.core">"#.to_string(),
    ];
    let mut off = 0;
    for i in 0..=33usize {
        let name = match i { 29 => "fp".to_string(), 30 => "lr".into(), 31 => "sp".into(),
                             32 => "pc".into(), 33 => "cpsr".into(), n => format!("x{n}") };
        let bits = if i == 33 { 32 } else { 64 };
        let ty = match i { 32 => "code_ptr", 29 | 31 => "data_ptr", _ => "int" };
        let alt = match i { 29 => r#" altname="x29""#, 30 => r#" altname="x30""#, _ => "" };
        let generic = match i { 0..=7 => format!(r#" generic="arg{}""#, i + 1), 29 => r#" generic="fp""#.into(),
                                30 => r#" generic="ra""#.into(), 31 => r#" generic="sp""#.into(),
                                32 => r#" generic="pc""#.into(), 33 => r#" generic="flags""#.into(),
                                _ => String::new() };
        x.push(format!(r#"<reg name="{name}"{alt} bitsize="{bits}" offset="{off}" regnum="{i}" type="{ty}" group="general"{generic}/>"#));
        off += bits / 8;
    }
    x.push("</feature>".into());
    x.push(r#"<feature name="org.gnu.gdb.aarch64.fpu">"#.into());
    for v in 0..32usize {
        x.push(format!(r#"<reg name="v{v}" bitsize="128" offset="{off}" regnum="{}" encoding="vector" format="vector-uint8" group="float"/>"#, 34 + v));
        off += 16;
    }
    for (i, name) in [(66, "fpsr"), (67, "fpcr")] {
        x.push(format!(r#"<reg name="{name}" bitsize="32" offset="{off}" regnum="{i}" type="int" group="float"/>"#));
        off += 4;
    }
    x.push("</feature>".into());
    x.push("</target>".into());
    x.join("\n")
}

/// A `qXfer:…:read` answer for `[off, off + len)` of `doc`: `m` + chunk while more remains, `l` +
/// the last chunk (possibly empty).
pub(crate) fn xfer_chunk(doc: &[u8], off: usize, len: usize) -> String {
    let start = off.min(doc.len());
    let end = off.saturating_add(len).min(doc.len());
    let tag = if end < doc.len() { 'm' } else { 'l' };
    format!("{tag}{}", String::from_utf8_lossy(&doc[start..end]))
}

/// §3c: what kind of stop a reply reports. Task 4 adds the step's `trace`, with its `signal()` and
/// `keys()` arms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StopKind {
    /// `replaylog:begin` with a description: lldb's reversible "history boundary" stop (t0 L4f).
    HistoryBegin(String),
    /// No reason: `qThreadStopInfo` for a thread that is not the one reporting (t0 L7).
    None,
    /// `reason:breakpoint` (t0 L4a).
    Breakpoint,
    /// `watch:<addr>`: lldb then reads memory NOW, which §3c's positions make right (t0 L4c).
    Watch(u64),
    /// `replaylog:end` with a description: the recording's end, reversible (t0 L4e).
    HistoryEnd(String),
    /// A Mach `EXC_BAD_ACCESS` (`metype:1`): lldb's native crash display, reversible (t0 L4d).
    MachBadAccess { code: u64, far: u64 },
    /// `reason:exception` with a description. Reversible, unlike a plain signal stop (t0 L4d).
    Exception { signal: u8, text: String },
}

impl StopKind {
    fn signal(&self) -> u8 {
        match self {
            StopKind::HistoryBegin(_) | StopKind::Breakpoint | StopKind::Watch(_) | StopKind::HistoryEnd(_) => 5,
            StopKind::None => 0,
            StopKind::MachBadAccess { .. } => 0x0b,
            StopKind::Exception { signal, .. } => *signal,
        }
    }
    fn keys(&self) -> String {
        match self {
            StopKind::HistoryBegin(d) => format!("replaylog:begin;description:{};", hex(d.as_bytes())),
            StopKind::None => String::new(),
            StopKind::Breakpoint => "reason:breakpoint;".into(),
            StopKind::Watch(a) => format!("watch:{a:x};"),
            StopKind::HistoryEnd(d) => format!("replaylog:end;description:{};", hex(d.as_bytes())),
            StopKind::MachBadAccess { code, far } => format!("metype:1;mecount:2;medata:{code:x};medata:{far:x};"),
            StopKind::Exception { text, .. } => format!("reason:exception;description:{};", hex(text.as_bytes())),
        }
    }
}

/// §3c's terminal list (t0 L4d, L4e): how the recording's end is reported. Every one is reversible.
/// - an exit is `replaylog:end`;
/// - a crash from an abort (EC `0x20`, `0x21`, `0x24`, `0x25`) is a Mach `EXC_BAD_ACCESS`, code 2
///   (`KERN_PROTECTION_FAILURE`) for a permission fault (FSC `0x0c..=0x0f`) and 1
///   (`KERN_INVALID_ADDRESS`) otherwise;
/// - any other crash, and a fatal signal, is `reason:exception` with the CLI's own line.
pub(crate) fn terminal_kind(o: &Outcome) -> StopKind {
    match *o {
        Outcome::Exit { code } => StopKind::HistoryEnd(format!("exited (code {code})")),
        Outcome::Crash { pc, esr, far } => match (esr >> 26) & 0x3f {
            0x20 | 0x21 | 0x24 | 0x25 => StopKind::MachBadAccess {
                code: if (0x0c..=0x0f).contains(&(esr & 0x3f)) { 2 } else { 1 }, far },
            _ => StopKind::Exception { signal: 0x0b,
                text: format!("guest crashed: pc={pc:#x} far={far:#x} esr={esr:#x}") },
        },
        Outcome::Signal { sig } => StopKind::Exception { signal: sig as u8,
            text: format!("guest terminated by signal {sig}") },
    }
}

/// §3c: `T<sig>thread:<tid>;threads:…;thread-pcs:…;<fp lr sp pc cpsr>;<kind>`. `tid` and
/// `threads` are RSP thread ids (retrace's + 1: tid 0 is unusable, t0 L7).
pub(crate) fn stop_reply(tid: u32, ctx: &ThreadCtx, threads: &[(u32, u64)], kind: &StopKind) -> String {
    let join = |f: &dyn Fn(&(u32, u64)) -> String| threads.iter().map(f).collect::<Vec<_>>().join(",");
    let mut s = format!("T{:02x}thread:{tid:x};threads:{};thread-pcs:{};", kind.signal(),
                        join(&|(t, _)| format!("{t:x}")), join(&|(_, pc)| format!("{pc:x}")));
    for r in 29..=33usize {
        s += &format!("{r:02x}:{};", hex(&reg_bytes(ctx, r).expect("an expedited register")));
    }
    s + &kind.keys()
}

/// §3g: `jGetLoadedDynamicLibrariesInfos`' answer for one image, in the shape lldb-2100 took
/// from t0's stub (t0 L2, `l2_jimg_exe`). `hdr` is the image's `mach_header_64` followed by its
/// load commands, read out of the recording. `path` must name the same binary on disk, or lldb
/// loads the wrong file.
pub(crate) fn image_json(hdr: &[u8], load_address: u64, path: &str) -> Result<String, String> {
    let u32at = |o: usize| hdr.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .ok_or_else(|| format!("Mach-O header truncated at {o:#x}"));
    let u64at = |o: usize| hdr.get(o..o + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()))
        .ok_or_else(|| format!("Mach-O header truncated at {o:#x}"));
    let magic = u32at(0)?;
    if magic != 0xfeed_facf { return Err(format!("not a 64-bit Mach-O header (magic {magic:#x})")); }
    let (cputype, cpusub, ftype, ncmds, flags) = (u32at(4)?, u32at(8)?, u32at(12)?, u32at(16)?, u32at(24)?);
    struct Seg { name: String, vmaddr: u64, vmsize: u64, fileoff: u64, filesize: u64, maxprot: i32 }
    let (mut segs, mut uuid, mut off) = (Vec::new(), None, 32usize);
    for _ in 0..ncmds {
        let (cmd, size) = (u32at(off)?, u32at(off + 4)? as usize);
        if size == 0 { return Err(format!("a zero-size load command at {off:#x}")); }
        match cmd {
            0x19 => { // LC_SEGMENT_64
                let raw = hdr.get(off + 8..off + 24).ok_or("segment name truncated")?;
                let name = String::from_utf8_lossy(raw).trim_end_matches('\0').to_string();
                segs.push(Seg { name, vmaddr: u64at(off + 24)?, vmsize: u64at(off + 32)?,
                                fileoff: u64at(off + 40)?, filesize: u64at(off + 48)?,
                                maxprot: u32at(off + 56)? as i32 });
            }
            0x1b => { // LC_UUID
                let u = hdr.get(off + 8..off + 24).ok_or("uuid truncated")?;
                let h = hex(u).to_uppercase();
                uuid = Some(format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]));
            }
            _ => {}
        }
        off += size;
    }
    let text = segs.iter().find(|s| s.name == "__TEXT").ok_or("no __TEXT segment")?;
    let slide = load_address.wrapping_sub(text.vmaddr);
    let uuid = uuid.ok_or("no LC_UUID")?;
    let seg_json: Vec<String> = segs.iter().map(|s| {
        let vmaddr = if s.name == "__PAGEZERO" { s.vmaddr } else { s.vmaddr.wrapping_add(slide) };
        format!(r#"{{"name":"{}","vmaddr":{vmaddr},"vmsize":{},"fileoff":{},"filesize":{},"maxprot":{}}}"#,
                json_str(&s.name), s.vmsize, s.fileoff, s.filesize, s.maxprot)
    }).collect();
    Ok(format!(concat!(r#"{{"images":[{{"load_address":{},"mod_date":0,"pathname":"{}","uuid":"{}","#,
                       r#""min_version_os_name":"macosx","min_version_os_sdk":"26.0","#,
                       r#""mach_header":{{"magic":{},"cputype":{},"cpusubtype":{},"filetype":{},"flags":{}}},"#,
                       r#""segments":[{}]}}]}}"#),
               load_address, json_str(path), uuid, magic, cputype as i32, cpusub, ftype, flags, seg_json.join(",")))
}

/// The inside of a JSON string: `\`, `"` and every control character below 0x20 escaped. The path
/// is the user's and a segment name is 16 bytes of the recording, so neither is trusted to be tame.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(p: &str) -> Vec<u8> { encode(p.as_bytes()) }

    #[test] fn a_packet_split_across_reads_and_two_in_one_read_both_frame() {
        let mut d = Decoder::default();
        let whole = packet("qSupported:xmlRegisters=aarch64");
        d.push(&whole[..7]);
        assert_eq!(d.next(), None, "half a packet is not a frame");
        d.push(&whole[7..]);
        assert_eq!(d.next(), Some(Frame::Packet("qSupported:xmlRegisters=aarch64".into())));
        let mut two = b"+".to_vec();
        two.extend(packet("?"));
        two.push(0x03);
        two.extend(packet("g"));
        d.push(&two);
        assert_eq!(d.next(), Some(Frame::Ack));
        assert_eq!(d.next(), Some(Frame::Packet("?".into())));
        assert_eq!(d.next(), Some(Frame::Interrupt));
        assert_eq!(d.next(), Some(Frame::Packet("g".into())));
        assert_eq!(d.next(), None);
    }

    #[test] fn a_bad_checksum_is_a_bad_frame_and_escapes_round_trip() {
        let mut d = Decoder::default();
        d.push(b"$g#00");
        assert_eq!(d.next(), Some(Frame::Bad));
        let tricky = r#"{"a":"$#*}"}"#;
        let wire = encode(tricky.as_bytes());
        assert!(!wire[1..wire.len() - 3].contains(&b'#'), "no raw # inside the body");
        d.push(&wire);
        assert_eq!(d.next(), Some(Frame::Packet(tricky.into())));
    }

    #[test] fn the_checksum_is_the_byte_sum_of_the_escaped_body() {
        assert_eq!(encode(b"OK"), b"$OK#9a", "0x4f + 0x4b");
        assert_eq!(encode(b"}"), b"$}]#da", "`}}` goes out as `}}` 0x5d, and 0x7d + 0x5d = 0xda");
    }

    #[test] fn garbage_before_a_packet_is_skipped_and_a_split_checksum_waits_for_its_second_digit() {
        let mut d = Decoder::default();
        d.push(b"xyz\r\n$g#67");
        assert_eq!(d.next(), Some(Frame::Packet("g".into())));
        assert_eq!(d.next(), None);
        d.push(b"$g#6");
        assert_eq!(d.next(), None, "one checksum digit is not a frame");
        d.push(b"7");
        assert_eq!(d.next(), Some(Frame::Packet("g".into())));
        assert_eq!(d.next(), None);
    }

    #[test] fn hex_round_trips_and_rejects_odd_input() {
        assert_eq!(hex(&[0x00, 0xab, 0xff]), "00abff");
        assert_eq!(unhex("00abff"), Some(vec![0x00, 0xab, 0xff]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
    }

    #[test] fn xfer_reads_in_chunks_and_past_the_end() {
        let doc = b"0123456789";
        assert_eq!(xfer_chunk(doc, 0, 4), "m0123");
        assert_eq!(xfer_chunk(doc, 8, 4), "l89");
        assert_eq!(xfer_chunk(doc, 20, 4), "l", "past the end: the last, empty chunk");
    }

    #[test] fn the_register_description_numbers_68_registers_in_gdb_order() {
        let x = target_xml();
        assert!(x.contains(r#"<reg name="x0" bitsize="64" offset="0" regnum="0""#), "{x}");
        assert!(x.contains(r#"<reg name="pc" bitsize="64" offset="256" regnum="32" type="code_ptr""#), "{x}");
        assert!(x.contains(r#"<reg name="cpsr" bitsize="32" offset="264" regnum="33""#), "{x}");
        assert!(x.contains(r#"<reg name="v0" bitsize="128" offset="268" regnum="34""#), "{x}");
        assert!(x.contains(r#"<reg name="fpcr" bitsize="32" offset="784" regnum="67""#), "{x}");
        let ctx = ThreadCtx::zeroed();
        assert_eq!(all_regs_hex(&ctx).len(), 788 * 2, "g is every register's bytes, in order");
        assert_eq!(reg_bytes(&ctx, NREGS), None);
    }

    /// Every register distinct, so a swapped, shifted or truncated slot cannot pass. The e2e row
    /// compares against a real session, but at (1, 0) x0–x30 are all zero there.
    #[test] fn every_register_lands_at_its_target_xml_offset_and_the_stop_reply_expedites_the_same_bytes() {
        let mut ctx = ThreadCtx::zeroed();
        for (i, x) in ctx.regs.x.iter_mut().enumerate() { *x = 0x1000 + i as u64; }
        ctx.regs.sp_el0 = 0x0000_7ff0_0000_1f00;
        ctx.regs.pc = 0x0000_0001_0000_4a40;
        ctx.regs.cpsr = 0xf000_03c5; // N Z C V set: the top of the 32-bit word must survive `as u32`
        for (i, v) in ctx.fp.iter_mut().enumerate() {
            // Distinct halves, so a swapped half fails too.
            *v = ((0xa0a0_0000_0000_0100u128 + i as u128) << 64) | (0x0b0b_0000_0000_0200u128 + i as u128);
        }
        ctx.fpsr = 0x0800_009f;
        ctx.fpcr = 0x0340_0000;
        let g = unhex(&all_regs_hex(&ctx)).unwrap();
        assert_eq!(g.len(), 788);
        let at = |off: usize, n: usize| g[off..off + n].to_vec();
        for i in 0..29 { assert_eq!(at(i * 8, 8), (0x1000 + i as u64).to_le_bytes(), "x{i}"); }
        assert_eq!(at(232, 8), 0x101du64.to_le_bytes(), "fp (x29)");
        assert_eq!(at(240, 8), 0x101eu64.to_le_bytes(), "lr (x30)");
        assert_eq!(at(248, 8), 0x0000_7ff0_0000_1f00u64.to_le_bytes(), "sp");
        assert_eq!(at(256, 8), 0x0000_0001_0000_4a40u64.to_le_bytes(), "pc");
        assert_eq!(at(264, 4), 0xf000_03c5u32.to_le_bytes(), "cpsr");
        for (v, want) in ctx.fp.iter().enumerate() { assert_eq!(at(268 + 16 * v, 16), want.to_le_bytes(), "v{v}"); }
        assert_eq!(at(780, 4), 0x0800_009fu32.to_le_bytes(), "fpsr");
        assert_eq!(at(784, 4), 0x0340_0000u32.to_le_bytes(), "fpcr");
        let s = stop_reply(1, &ctx, &[(1, ctx.regs.pc)], &StopKind::None);
        for r in 29..=33usize {
            let want = format!("{r:02x}:{};", hex(&reg_bytes(&ctx, r).unwrap()));
            assert!(s.contains(&want), "{want} in {s}");
        }
    }

    #[test] fn a_start_stop_names_its_thread_every_thread_and_the_expedited_registers() {
        let mut ctx = ThreadCtx::zeroed();
        ctx.regs.pc = 0x1_0000_0380;
        ctx.regs.sp_el0 = 0x1_c000;
        let s = stop_reply(1, &ctx, &[(1, 0x1_0000_0380)], &StopKind::HistoryBegin("start of recording".into()));
        assert!(s.starts_with("T05thread:1;threads:1;thread-pcs:100000380;"), "{s}");
        assert!(s.contains("1f:00c0010000000000;20:8003000001000000;21:00000000;"), "{s}");
        assert!(s.ends_with(&format!("replaylog:begin;description:{};", hex(b"start of recording"))), "{s}");
        assert_eq!(stop_reply(2, &ctx, &[(1, 0), (2, 0)], &StopKind::None).get(..3), Some("T00"));
    }

    #[test] fn every_stop_kind_carries_its_measured_signal_and_keys() {
        // The key shapes are t0's (L4a, L4c, L4d, L4e); lldb parses them, so they are pinned byte
        // for byte.
        let ctx = ThreadCtx::zeroed();
        let reply = |k: StopKind| stop_reply(1, &ctx, &[(1, 0)], &k);
        let bp = reply(StopKind::Breakpoint);
        assert!(bp.starts_with("T05") && bp.ends_with("reason:breakpoint;"), "{bp}");
        let w = reply(StopKind::Watch(0x1_0000_4140));
        assert!(w.starts_with("T05") && w.ends_with("watch:100004140;"), "{w}");
        let end = reply(StopKind::HistoryEnd("exited (code 0)".into()));
        assert!(end.starts_with("T05")
            && end.ends_with(&format!("replaylog:end;description:{};", hex(b"exited (code 0)"))), "{end}");
        let crash = reply(StopKind::MachBadAccess { code: 1, far: 0x4000_dead_0000 });
        assert!(crash.starts_with("T0b")
            && crash.ends_with("metype:1;mecount:2;medata:1;medata:4000dead0000;"), "{crash}");
        let exc = reply(StopKind::Exception { signal: 6, text: "guest terminated by signal 6".into() });
        assert!(exc.starts_with("T06")
            && exc.ends_with(&format!("reason:exception;description:{};", hex(b"guest terminated by signal 6"))), "{exc}");
    }

    #[test] fn each_terminal_outcome_maps_to_its_measured_stop() {
        // §3c's terminal list. An ESR is EC << 26 | IL | ISS; the fault status code is ISS[5:0].
        const IL: u64 = 1 << 25;
        let crash = |ec: u64, fsc: u64| Outcome::Crash { pc: 0x1_0000_0400, esr: (ec << 26) | IL | fsc, far: 0x4000_dead_0000 };
        let bad_access = |code| StopKind::MachBadAccess { code, far: 0x4000_dead_0000 };
        assert_eq!(terminal_kind(&Outcome::Exit { code: 3 }), StopKind::HistoryEnd("exited (code 3)".into()));
        assert_eq!(terminal_kind(&crash(0x24, 0x07)), bad_access(1), "translation fault, level 3");
        assert_eq!(terminal_kind(&crash(0x24, 0x0c)), bad_access(2), "permission fault, level 0");
        assert_eq!(terminal_kind(&crash(0x24, 0x0f)), bad_access(2), "permission fault, level 3");
        assert_eq!(terminal_kind(&crash(0x24, 0x10)), bad_access(1), "external abort: not a permission fault");
        assert_eq!(terminal_kind(&crash(0x20, 0x04)), bad_access(1), "an instruction abort is an abort too");
        let brk = (0x3c << 26) | IL;
        assert_eq!(terminal_kind(&Outcome::Crash { pc: 0x1_0000_0400, esr: brk, far: 0 }),
                   StopKind::Exception { signal: 0x0b,
                       text: format!("guest crashed: pc=0x100000400 far=0x0 esr={brk:#x}") });
        assert_eq!(terminal_kind(&Outcome::Signal { sig: 6 }),
                   StopKind::Exception { signal: 6, text: "guest terminated by signal 6".into() });
    }

    #[test] fn the_image_list_describes_a_real_binary_at_its_load_address() {
        let file = std::fs::read(retrace_guest::HELLO).expect("the hello fixture");
        let j = image_json(&file, 0x1_0000_0000, retrace_guest::HELLO).unwrap();
        assert!(j.starts_with(r#"{"images":[{"load_address":4294967296,"mod_date":0,"pathname":""#), "{j}");
        assert!(j.contains(r#""name":"__TEXT","vmaddr":4294967296"#), "{j}");
        let uuid = j.split(r#""uuid":""#).nth(1).and_then(|r| r.split('"').next()).unwrap();
        assert_eq!(uuid.len(), 36, "{uuid}");
        assert!(image_json(&file[..16], 0x1_0000_0000, "x").is_err(), "a truncated header is an Err, not a panic");
        let j = image_json(&file, 0x1_0000_0000, "/t/a\"b\\c\nd\u{1f}e").unwrap();
        assert!(j.contains(r#""pathname":"/t/a\"b\\c\u000ad\u001fe""#), "control characters are escaped: {j}");
    }
}
