//! M19: address → symbol name, read out of the recording's own snapshot.
//!
//! **This module is presentation-only and that is its defining property.** It is a pure function of
//! bytes that are already in the trace plus the fixed IPA layout constants, so it never touches
//! `record_box`, `ReplaySession::advance`, or `Box_::run()`. Neither symmetry rule is engaged, no
//! landmark is added, and the divergence oracle cannot see it: **nothing here can make a recording
//! diverge.** That is what made M19 safe to build in one pass, and it is the invariant to preserve
//! if this module ever grows.
//!
//! The symbols come from the snapshot rather than from the binary on disk, which is not an
//! optimisation but the whole design. `Event` carries no path, UUID, or image identity (measurements
//! M6), so the alternatives were a trace-format break or an `--exe <path>` flag — and a path can name
//! a *different build* than the one recorded, which mis-symbolicates silently and is worse than not
//! symbolicating at all. `parse_macho` maps every `LC_SEGMENT_64` except `__PAGEZERO`, so
//! `__LINKEDIT` — holding the `nlist_64` array and the string table — is mapped into guest memory
//! (M4); and `Box_::snapshot` captures every backing in full (M5). The symbol table is therefore
//! already inside every recording made in the current format, and `TRACE_MAGIC` does not move.
//!
//! See `docs/superpowers/specs/2026-08-25-retrace-m19-symbols-measurements.md` for the numbers this
//! file is built on; they are cited by tag (M1–M7, P1–P3) rather than restated.

use retrace_trace::Region;

/// The image bases M19 routes on (M7), re-exported so a consumer can name them without taking a
/// direct dependency on `retrace-box` — which the `retrace` crate and its tests do not have.
///
/// Re-exported rather than re-declared **on purpose**: a second copy of a layout constant is drift
/// waiting to happen, and the failure it produces is R3's — a confidently wrong name at a plausible
/// address — not a compile error. There is exactly one definition of each, in
/// `crates/retrace-box/src/lib.rs`.
pub use retrace_box::{DYLD_BASE, EXE_BASE};

// ---------------------------------------------------------------------------------------------
// Mach-O and nlist constants.
//
// The `N_*` values are MEASURED from this machine's SDK header (P2):
// /Applications/Xcode.app/.../MacOSX.sdk/usr/include/mach-o/nlist.h lines 117-135.
// ---------------------------------------------------------------------------------------------

const MH_MAGIC_64: u32 = 0xfeed_facf;
const LC_SEGMENT_64: u32 = 0x19;
const LC_SYMTAB: u32 = 0x2;

/// If any of these bits are set the entry is a symbolic **debugging** entry (a stab), whose
/// `n_value` is not an address. Must be skipped or the table fills with source-file records.
const N_STAB: u8 = 0xe0;
/// Mask for the type bits of `n_type`.
const N_TYPE: u8 = 0x0e;
/// Defined in section `n_sect` — the only kind M19 keeps.
///
/// **Footgun, and the reason this is spelled out rather than inlined (P2):** `N_SECT` (`0xe`) is
/// numerically equal to the `N_TYPE` mask (`0x0e`). The correct test is
/// `n_type & N_TYPE == N_SECT`. A reader that slips into `n_type & N_SECT != 0` compiles, reads
/// plausibly, and silently accepts `N_PBUD` (`0xc`) and `N_INDR` (`0xa`) — neither of which has an
/// address in `n_value`. `an_indirect_symbol_is_dropped` pins this.
const N_SECT: u8 = 0x0e;

const MACH_HEADER_64_LEN: usize = 32;
const NLIST_64_LEN: usize = 16;

fn u32le(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn u64le(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

/// Gather `len` bytes starting at guest address `addr`, spanning regions if the range crosses a
/// boundary. `None` when any byte of the range is not present in the snapshot.
///
/// P1 measured that `__LINKEDIT` is exactly one `Region` for both images M19 reads, because the
/// loader's `map` closure pushes one `Backing` per segment. The spanning loop is kept anyway, and
/// deliberately: *other* backings in the same snapshot genuinely are per-page (the mmap and
/// demand-commit paths), so a reader that assumed "one region per lookup" would be correct today and
/// wrong the first time anything else is read. It costs ~10 lines to not have the assumption.
fn read(mem: &[Region], addr: u64, len: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(len);
    let mut cur = addr;
    while out.len() < len {
        let r = mem
            .iter()
            .find(|r| r.ipa <= cur && cur < r.ipa + r.bytes.len() as u64)?;
        let off = (cur - r.ipa) as usize;
        let take = (r.bytes.len() - off).min(len - out.len());
        out.extend_from_slice(&r.bytes[off..off + take]);
        cur += take as u64;
    }
    Some(out)
}

/// One image's defined text symbols, sorted, with the `__TEXT` end that bounds resolution.
pub struct SymbolTable {
    /// Sorted by `(addr, name)`. The name tie-break is not cosmetic: aliases share an address, and a
    /// debugger that renames a function between two runs of the same query is worse than one that
    /// prints hex.
    syms: Vec<(u64, String)>,
    /// `__TEXT` vmaddr + vmsize + slide. Resolution stops here so a pc past the last symbol reports
    /// nothing rather than `last + something_enormous`.
    text_end: u64,
}

impl SymbolTable {
    /// Build the table for the image whose Mach-O header sits at `header_addr`.
    ///
    /// `None` means **no usable symbols**, which is normal and not an error: a static guest has no
    /// dyld mapped at all, and a stripped binary has no `LC_SYMTAB` worth reading (M3 measured `jq`
    /// at 7 defined text symbols). Callers degrade to raw hex.
    ///
    /// **Absence is data; malformation returns `None` too.** The plan called for asserting on a
    /// malformed table. This returns `None` instead, deliberately: a debugger that panics mid-session
    /// costs the user their session, whereas one that prints hex when it could have printed a name
    /// costs almost nothing. A *reader* bug does not hide behind that, because the unit tests below
    /// assert on specific resolved names — a reader that silently produced nothing would fail them.
    pub fn for_image(mem: &[Region], header_addr: u64) -> Option<SymbolTable> {
        let hdr = read(mem, header_addr, MACH_HEADER_64_LEN)?;
        if u32le(&hdr, 0)? != MH_MAGIC_64 {
            return None;
        }
        let ncmds = u32le(&hdr, 16)?;
        let sizeofcmds = u32le(&hdr, 20)? as usize;
        let cmds = read(mem, header_addr + MACH_HEADER_64_LEN as u64, sizeofcmds)?;

        let (mut text_vmaddr, mut text_vmsize) = (None, 0u64);
        let (mut le_vmaddr, mut le_fileoff) = (None, 0u64);
        let mut symtab = None;

        let mut off = 0usize;
        for _ in 0..ncmds {
            let cmd = u32le(&cmds, off)?;
            let cmdsize = u32le(&cmds, off + 4)? as usize;
            if cmdsize == 0 {
                return None; // a zero-size command would loop forever
            }
            match cmd {
                LC_SEGMENT_64 => {
                    let name = cmds.get(off + 8..off + 24)?;
                    let vmaddr = u64le(&cmds, off + 24)?;
                    let vmsize = u64le(&cmds, off + 32)?;
                    let fileoff = u64le(&cmds, off + 40)?;
                    if name.starts_with(b"__TEXT\0") {
                        text_vmaddr = Some(vmaddr);
                        text_vmsize = vmsize;
                    } else if name.starts_with(b"__LINKEDIT\0") {
                        le_vmaddr = Some(vmaddr);
                        le_fileoff = fileoff;
                    }
                }
                LC_SYMTAB => {
                    symtab = Some((
                        u32le(&cmds, off + 8)? as u64,  // symoff
                        u32le(&cmds, off + 12)?,        // nsyms
                        u32le(&cmds, off + 16)? as u64, // stroff
                        u32le(&cmds, off + 20)? as u64, // strsize
                    ));
                }
                _ => {}
            }
            off += cmdsize;
        }

        let text_vmaddr = text_vmaddr?;
        let le_vmaddr = le_vmaddr?;
        let (symoff, nsyms, stroff, strsize) = symtab?; // no LC_SYMTAB: the stripped case, normal

        // The slide is the additive constant the loader applied, and the header sits at the start of
        // `__TEXT`, so `header_addr == text_vmaddr + slide`. Deriving it this way rather than taking
        // it as a parameter is what keeps it right for both images: 0 for the main executable
        // (M2, EXE_BASE equals its own __TEXT vmaddr) and DYLD_BASE for dyld (P3, whose __TEXT vmaddr
        // is 0). Never hardcode it — that is R3's named failure mode, whose symptom is confidently
        // WRONG names rather than missing ones.
        let slide = header_addr.wrapping_sub(text_vmaddr);

        // A LC_SYMTAB file offset becomes a guest VA through __LINKEDIT's own (fileoff → vmaddr)
        // mapping, plus the slide (M4). Worked example, crashthread: symoff 32960, __LINKEDIT at
        // vmaddr 0x100008000 / fileoff 32768, slide 0 → 0x1000080c0.
        let file_to_va = |fo: u64| -> Option<u64> {
            le_vmaddr
                .checked_add(fo.checked_sub(le_fileoff)?)?
                .checked_add(slide)
        };
        let symva = file_to_va(symoff)?;
        let strva = file_to_va(stroff)?;

        let raw = read(mem, symva, (nsyms as usize).checked_mul(NLIST_64_LEN)?)?;
        let strs = read(mem, strva, strsize as usize)?;

        let text_end = text_vmaddr.checked_add(text_vmsize)?.checked_add(slide)?;

        let mut syms = Vec::new();
        for i in 0..nsyms as usize {
            let e = i * NLIST_64_LEN;
            let n_strx = u32le(&raw, e)? as usize;
            let n_type = *raw.get(e + 4)?;
            let n_sect = *raw.get(e + 5)?;
            let n_value = u64le(&raw, e + 8)?;

            if n_type & N_STAB != 0 {
                continue; // debugging entry; n_value is not an address
            }
            if n_type & N_TYPE != N_SECT || n_sect == 0 {
                continue; // undefined, absolute, prebound-undefined, indirect
            }
            let Some(name) = cstr_at(&strs, n_strx) else {
                continue; // n_strx past the string table: skip the entry, keep the table
            };
            if name.is_empty() {
                continue;
            }
            syms.push((n_value.wrapping_add(slide), name));
        }
        if syms.is_empty() {
            return None;
        }
        syms.sort_unstable();
        syms.dedup();
        Some(SymbolTable { syms, text_end })
    }

    /// `(name, offset)` for the nearest preceding symbol, or `None` at/past `text_end`.
    pub fn resolve(&self, addr: u64) -> Option<(&str, u64)> {
        if addr >= self.text_end {
            return None;
        }
        // `end` is the first index whose addr > `addr`, so the nearest preceding symbol is at
        // `end - 1`. That entry is the LAST of its tied group, though, and the contract is the
        // FIRST — so take its address and back up to where that address starts. Ties are aliases at
        // one address; `syms` is sorted by `(addr, name)`, which makes "first" mean the
        // alphabetically smallest name and therefore stable across builds and across repeated
        // queries. Without this second step the winner would be whichever alias sorted last, which
        // is equally deterministic but is not what the design specifies and reads as arbitrary.
        let end = self.syms.partition_point(|(a, _)| *a <= addr);
        let sym_addr = self.syms.get(end.checked_sub(1)?)?.0;
        let first = self.syms.partition_point(|(a, _)| *a < sym_addr);
        let (a, n) = self.syms.get(first)?;
        Some((n.as_str(), addr - a))
    }
}

impl SymbolTable {
    /// Every address this image defines under `name`, sorted ascending and deduped.
    ///
    /// **Returns a `Vec`, not an `Option`, and that is the point (M20 S4).** Address → name is a
    /// function; name → address is not. `threadrust` binds **19** names to more than one address and
    /// dyld's arm64e slice 14 — with `___Block_byref_object_copy_` alone at **13 distinct addresses**
    /// — because compiler-generated locals repeat per translation unit
    /// and Mach-O keeps every one. Collapsing that here — "return the first" — would put a
    /// confidently wrong address behind a plausible name, which is R3's failure mode aimed at a
    /// breakpoint instead of at a printed string. Deciding what to do about ambiguity is the
    /// caller's job; this layer's job is not to hide it.
    ///
    /// Exact match only. Substring or suffix matching would manufacture ambiguity rather than
    /// report it, which is the opposite of the above.
    ///
    /// **Not clamped to `__TEXT`**, unlike [`SymbolTable::resolve`]: `for_image` keeps any defined
    /// section, so a `__DATA` symbol is here and is reachable by name even though no pc will ever
    /// resolve *to* it (S6).
    pub fn addrs_of(&self, name: &str) -> Vec<u64> {
        let mut out: Vec<u64> = self
            .syms
            .iter()
            .filter(|(_, n)| n == name)
            .map(|(a, _)| *a)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Read a NUL-terminated string at `off` in the string table.
fn cstr_at(strs: &[u8], off: usize) -> Option<String> {
    let rest = strs.get(off..)?;
    let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    Some(String::from_utf8_lossy(&rest[..end]).into_owned())
}

/// Every image M19 knows how to symbolicate, resolved together.
#[derive(Default)]
pub struct Symbols {
    images: Vec<SymbolTable>,
}

impl Symbols {
    /// Build from a snapshot's regions. Never panics and never fails: an image that is absent or
    /// stripped simply contributes nothing, so a static guest and a `jq` recording both yield an
    /// empty `Symbols` that formats every address as bare hex.
    ///
    /// Image routing is a range check over the fixed IPA layout (M7). The shared-cache window
    /// (`SHARED_REGION_START..SHARED_REGION_END`) is deliberately absent: cache images carry no
    /// `LC_SYMTAB` in the mapped region, and the cache's local-symbol area lives in the on-disk cache
    /// file that `cache.rs` demand-pages but never stages into guest memory. Reading it would
    /// reintroduce the external-file dependency this design exists to avoid. That is the wall, and
    /// `cache_symbol_e2e` is parked at it.
    pub fn from_snapshot(mem: &[Region]) -> Symbols {
        let images = [retrace_box::EXE_BASE, retrace_box::DYLD_BASE]
            .into_iter()
            .filter_map(|base| SymbolTable::for_image(mem, base))
            .collect();
        Symbols { images }
    }

    /// `(name, offset)` from whichever image claims the address.
    pub fn resolve(&self, addr: u64) -> Option<(&str, u64)> {
        self.images.iter().find_map(|t| t.resolve(addr))
    }

    /// Every address defined under `name`, searching the **executable first** and consulting dyld
    /// only if the executable defines nothing (M20).
    ///
    /// The precedence is a decided rule, not an artifact of `images` happening to be built in
    /// `[EXE_BASE, DYLD_BASE]` order — a later reader reordering that array must break a test, not
    /// silently change which symbol a breakpoint lands on. A guest symbol shadows a dyld symbol of
    /// the same name because the guest is what the user is debugging.
    ///
    /// Matches are *not* merged across images. Returning the union would report a name as ambiguous
    /// whenever dyld happened to define it too, which would refuse breakpoints the user is entitled
    /// to set. Measured mitigation for the common case: dyld does not define `_main` (S4).
    pub fn addrs_of(&self, name: &str) -> Vec<u64> {
        for t in &self.images {
            let hits = t.addrs_of(name);
            if !hits.is_empty() {
                return hits;
            }
        }
        Vec::new()
    }

    /// `"0x10000050c (_child+0x30)"` — or `"0x100000460 (_main)"` exactly at a symbol, or bare
    /// `"0x1c0004000"` when nothing claims it.
    ///
    /// **The raw address is always present**, never replaced. Every existing assertion in the tree
    /// that greps for a hex address keeps matching, and a reader can still paste the number into
    /// `x` or `break`.
    pub fn format(&self, addr: u64) -> String {
        match self.resolve(addr) {
            Some((n, 0)) => format!("{addr:#x} ({n})"),
            Some((n, off)) => format!("{addr:#x} ({n}+{off:#x})"),
            None => format!("{addr:#x}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic Mach-O image and split it into regions the way the loader does — **one
    /// Region per segment** (P1), so the tests exercise the real shape rather than one flat buffer.
    struct Img {
        text_vmaddr: u64,
        slide: u64,
        syms: Vec<(u64, &'static str, u8, u8)>, // (addr, name, n_type, n_sect)
        text_vmsize: u64,
    }

    impl Img {
        fn new(text_vmaddr: u64, slide: u64) -> Img {
            Img { text_vmaddr, slide, syms: Vec::new(), text_vmsize: 0x4000 }
        }
        /// A normal defined text symbol.
        fn sym(mut self, addr: u64, name: &'static str) -> Img {
            self.syms.push((addr, name, N_SECT, 1));
            self
        }
        /// An entry with an explicit `n_type`/`n_sect`, for the filtering tests.
        fn raw_sym(mut self, addr: u64, name: &'static str, n_type: u8, n_sect: u8) -> Img {
            self.syms.push((addr, name, n_type, n_sect));
            self
        }
        fn build(&self) -> Vec<Region> {
            let text_size = self.text_vmsize as usize; // file [0, text_size)
            let le_fileoff = text_size as u64;

            // String table: index 0 is the conventional empty string.
            let mut strs = vec![0u8];
            let mut offs = Vec::new();
            for (_, n, _, _) in &self.syms {
                offs.push(strs.len() as u32);
                strs.extend_from_slice(n.as_bytes());
                strs.push(0);
            }
            let mut nlist = Vec::new();
            for (i, (addr, _, ty, sect)) in self.syms.iter().enumerate() {
                nlist.extend_from_slice(&offs[i].to_le_bytes());
                nlist.push(*ty);
                nlist.push(*sect);
                nlist.extend_from_slice(&0u16.to_le_bytes());
                nlist.extend_from_slice(&addr.to_le_bytes());
            }
            let symoff = le_fileoff;
            let stroff = symoff + nlist.len() as u64;

            // Load commands: __TEXT, __LINKEDIT, LC_SYMTAB.
            let mut lc = Vec::new();
            let seg = |name: &str, vmaddr: u64, vmsize: u64, fileoff: u64, filesize: u64| {
                let mut s = Vec::new();
                s.extend_from_slice(&LC_SEGMENT_64.to_le_bytes());
                s.extend_from_slice(&72u32.to_le_bytes());
                let mut nm = [0u8; 16];
                nm[..name.len()].copy_from_slice(name.as_bytes());
                s.extend_from_slice(&nm);
                s.extend_from_slice(&vmaddr.to_le_bytes());
                s.extend_from_slice(&vmsize.to_le_bytes());
                s.extend_from_slice(&fileoff.to_le_bytes());
                s.extend_from_slice(&filesize.to_le_bytes());
                s.extend_from_slice(&[0u8; 16]); // maxprot, initprot, nsects, flags
                s
            };
            let le_size = (nlist.len() + strs.len()) as u64;
            lc.extend(seg("__TEXT", self.text_vmaddr, self.text_vmsize, 0, text_size as u64));
            lc.extend(seg(
                "__LINKEDIT",
                self.text_vmaddr + self.text_vmsize,
                0x4000,
                le_fileoff,
                le_size,
            ));
            lc.extend_from_slice(&LC_SYMTAB.to_le_bytes());
            lc.extend_from_slice(&24u32.to_le_bytes());
            lc.extend_from_slice(&(symoff as u32).to_le_bytes());
            lc.extend_from_slice(&(self.syms.len() as u32).to_le_bytes());
            lc.extend_from_slice(&(stroff as u32).to_le_bytes());
            lc.extend_from_slice(&(strs.len() as u32).to_le_bytes());

            let mut hdr = Vec::new();
            hdr.extend_from_slice(&MH_MAGIC_64.to_le_bytes());
            hdr.extend_from_slice(&[0u8; 8]); // cputype, cpusubtype
            hdr.extend_from_slice(&2u32.to_le_bytes()); // filetype MH_EXECUTE
            hdr.extend_from_slice(&3u32.to_le_bytes()); // ncmds
            hdr.extend_from_slice(&(lc.len() as u32).to_le_bytes());
            hdr.extend_from_slice(&[0u8; 8]); // flags, reserved

            let mut text = vec![0u8; text_size];
            text[..hdr.len()].copy_from_slice(&hdr);
            text[hdr.len()..hdr.len() + lc.len()].copy_from_slice(&lc);

            let mut le = nlist.clone();
            le.extend_from_slice(&strs);

            vec![
                Region { ipa: self.text_vmaddr + self.slide, bytes: text },
                Region { ipa: self.text_vmaddr + self.text_vmsize + self.slide, bytes: le },
            ]
        }
    }

    fn table(img: &Img) -> SymbolTable {
        SymbolTable::for_image(&img.build(), img.text_vmaddr + img.slide)
            .expect("image should yield a table")
    }

    #[test]
    fn a_value_split_across_two_regions_is_gathered() {
        let mem = vec![
            Region { ipa: 0x1000, bytes: vec![0xaa, 0xbb] },
            Region { ipa: 0x1002, bytes: vec![0xcc, 0xdd] },
        ];
        assert_eq!(read(&mem, 0x1000, 4).unwrap(), vec![0xaa, 0xbb, 0xcc, 0xdd]);
        // Spanning is why this is a loop and not a slice: the value straddles the boundary.
        assert_eq!(read(&mem, 0x1001, 2).unwrap(), vec![0xbb, 0xcc]);
    }

    #[test]
    fn bytes_absent_from_the_snapshot_are_none_not_a_panic() {
        let mem = vec![Region { ipa: 0x1000, bytes: vec![0; 4] }];
        assert!(read(&mem, 0x2000, 4).is_none(), "a gap must be None");
        assert!(read(&mem, 0x1002, 8).is_none(), "running off the end must be None");
    }

    #[test]
    fn the_worked_example_from_the_measurements_resolves() {
        // M2: crashthread's recorded crash pc is _child+0x30, using nm's un-slid address directly
        // because the main executable's slide is 0.
        let img = Img::new(0x1_0000_0000, 0).sym(0x1_0000_0460, "_main").sym(0x1_0000_04dc, "_child");
        let t = table(&img);
        assert_eq!(t.resolve(0x1_0000_050c), Some(("_child", 0x30)));
        assert_eq!(t.resolve(0x1_0000_0460), Some(("_main", 0)));
    }

    #[test]
    fn a_dyld_style_slide_is_derived_not_assumed() {
        // P3: dyld's __TEXT vmaddr is 0 and the loader adds DYLD_BASE, so the slide IS DYLD_BASE.
        // R3's failure mode is a confidently WRONG name, so this asserts the name, not just Some.
        let img = Img::new(0, retrace_box::DYLD_BASE).sym(0x1000, "_dyld_start");
        let t = table(&img);
        assert_eq!(t.resolve(retrace_box::DYLD_BASE + 0x1008), Some(("_dyld_start", 8)));
    }

    #[test]
    fn a_stab_entry_is_dropped() {
        // N_STAB entries are debugging records whose n_value is not an address.
        let img = Img::new(0x1_0000_0000, 0)
            .sym(0x1_0000_0100, "_real")
            .raw_sym(0x1_0000_0200, "/src/foo.c", 0x64 /* N_SO */, 0);
        let t = table(&img);
        assert_eq!(t.resolve(0x1_0000_0208), Some(("_real", 0x108)),
            "a stab at 0x200 must not become the nearest preceding symbol");
    }

    #[test]
    fn an_indirect_symbol_is_dropped() {
        // Pins the P2 footgun: N_SECT (0xe) == the N_TYPE mask (0x0e), so `n_type & N_SECT != 0`
        // would wrongly accept N_INDR (0xa) and N_PBUD (0xc). The correct test is
        // `n_type & N_TYPE == N_SECT`, and this fails loudly if someone "simplifies" it.
        let img = Img::new(0x1_0000_0000, 0)
            .sym(0x1_0000_0100, "_real")
            .raw_sym(0x1_0000_0200, "_indirect", 0x0a /* N_INDR */, 1)
            .raw_sym(0x1_0000_0300, "_prebound", 0x0c /* N_PBUD */, 1);
        let t = table(&img);
        assert_eq!(t.resolve(0x1_0000_0208), Some(("_real", 0x108)));
        assert_eq!(t.resolve(0x1_0000_0308), Some(("_real", 0x208)));
    }

    #[test]
    fn an_undefined_symbol_is_dropped() {
        let img = Img::new(0x1_0000_0000, 0)
            .sym(0x1_0000_0100, "_real")
            .raw_sym(0, "_write", 0x0 /* N_UNDF */, 0);
        let t = table(&img);
        assert_eq!(t.resolve(0x1_0000_0104), Some(("_real", 4)));
    }

    #[test]
    fn ties_resolve_to_the_same_name_every_time() {
        // Aliases share an address. Sorting by (addr, name) makes the winner deterministic; without
        // the name in the key it would depend on symbol table order and could differ between builds.
        let img = Img::new(0x1_0000_0000, 0)
            .sym(0x1_0000_0100, "_zzz_alias")
            .sym(0x1_0000_0100, "_aaa_alias");
        let t = table(&img);
        let first = t.resolve(0x1_0000_0100);
        assert_eq!(first, Some(("_aaa_alias", 0)));
        for _ in 0..8 {
            assert_eq!(t.resolve(0x1_0000_0100), first, "the same query must give the same name");
        }
    }

    #[test]
    fn an_address_past_text_end_resolves_to_nothing() {
        // The clamp exists so a pc past the last symbol reports NOTHING rather than
        // `last + something_enormous`, which would be a confident lie.
        let img = Img::new(0x1_0000_0000, 0).sym(0x1_0000_0100, "_only");
        let t = table(&img);
        assert_eq!(t.resolve(0x1_0000_0100), Some(("_only", 0)));
        assert!(t.resolve(0x1_0000_4000).is_none(), "__TEXT ends at vmaddr+vmsize");
        assert!(t.resolve(0x9_9999_9999).is_none());
    }

    #[test]
    fn an_address_below_the_first_symbol_resolves_to_nothing() {
        let img = Img::new(0x1_0000_0000, 0).sym(0x1_0000_0100, "_only");
        assert!(table(&img).resolve(0x1_0000_0000).is_none());
    }

    #[test]
    fn an_image_with_no_symtab_is_none_not_an_error() {
        // The stripped case (M3: jq has 7 defined text symbols). Absence is DATA.
        let img = Img::new(0x1_0000_0000, 0); // no symbols at all
        assert!(SymbolTable::for_image(&img.build(), 0x1_0000_0000).is_none());
    }

    #[test]
    fn no_macho_header_at_the_base_is_none_not_a_panic() {
        // A static guest has no dyld mapped at DYLD_BASE. This must be silent.
        let mem = vec![Region { ipa: 0x1_0000_0000, bytes: vec![0u8; 64] }];
        assert!(SymbolTable::for_image(&mem, 0x1_0000_0000).is_none());
        assert!(SymbolTable::for_image(&[], 0x1_0000_0000).is_none());
    }

    #[test]
    fn format_always_contains_the_raw_address() {
        let img = Img::new(0x1_0000_0000, 0).sym(0x1_0000_0460, "_main");
        let s = Symbols { images: vec![table(&img)] };
        assert_eq!(s.format(0x1_0000_0460), "0x100000460 (_main)");
        assert_eq!(s.format(0x1_0000_0470), "0x100000470 (_main+0x10)");
        assert_eq!(s.format(0x9_9999_9999), "0x999999999");
        // The property that matters to every existing hex-matching assertion in the tree.
        for a in [0x1_0000_0460u64, 0x1_0000_0470, 0x9_9999_9999] {
            assert!(s.format(a).contains(&format!("{a:#x}")), "raw address must survive");
        }
    }

    #[test]
    fn an_empty_symbols_formats_everything_as_bare_hex() {
        let s = Symbols::default();
        assert_eq!(s.format(0x1_0000_050c), "0x10000050c");
    }

    // ---------------------------------------------------------------------------------------
    // M20: the reverse direction, name -> address.
    //
    // S4 is why these exist and why none of them assert `Some(one_address)`: name -> address is
    // NOT a function. threadrust binds 19 names to more than one address and dyld's arm64e slice
    // 14, one of them at 13 addresses, so the
    // return type is a Vec and "how many" is the caller's problem to refuse, not this layer's to
    // guess.
    // ---------------------------------------------------------------------------------------

    /// Build a `Symbols` holding an exe image and a dyld image, the way `from_snapshot` routes them.
    fn both(exe: &Img, dyld: &Img) -> Symbols {
        let mut mem = exe.build();
        mem.extend(dyld.build());
        Symbols::from_snapshot(&mem)
    }

    #[test]
    fn a_unique_name_yields_exactly_one_address() {
        let img = Img::new(0x1_0000_0000, 0).sym(0x1_0000_0460, "_main").sym(0x1_0000_04dc, "_child");
        let t = table(&img);
        assert_eq!(t.addrs_of("_child"), vec![0x1_0000_04dc]);
        assert_eq!(t.addrs_of("_main"), vec![0x1_0000_0460]);
    }

    #[test]
    fn an_absent_name_yields_nothing_not_a_guess() {
        let img = Img::new(0x1_0000_0000, 0).sym(0x1_0000_0460, "_main");
        // Empty, NOT the nearest name and not a fallback: a typo must never become a breakpoint at
        // a wrong-but-valid address.
        assert!(table(&img).addrs_of("_mian").is_empty());
    }

    #[test]
    fn a_name_at_two_addresses_yields_both_sorted() {
        // S4's case, and the one that must not regress into silently returning one.
        // `_OUTLINED_FUNCTION_0` is a real repeated name from threadrust, not an invention.
        let img = Img::new(0x1_0000_0000, 0)
            .sym(0x1_0000_0800, "_OUTLINED_FUNCTION_0")
            .sym(0x1_0000_0100, "_OUTLINED_FUNCTION_0")
            .sym(0x1_0000_0460, "_main");
        assert_eq!(table(&img).addrs_of("_OUTLINED_FUNCTION_0"),
                   vec![0x1_0000_0100, 0x1_0000_0800]);
    }

    #[test]
    fn one_name_at_one_address_is_not_reported_twice() {
        // `syms` is deduped at build, but a caller reading "2 matches" would print an ambiguity
        // error for a name that is not ambiguous, so pin it.
        let img = Img::new(0x1_0000_0000, 0).sym(0x1_0000_0460, "_main").sym(0x1_0000_0460, "_main");
        assert_eq!(table(&img).addrs_of("_main"), vec![0x1_0000_0460]);
    }

    #[test]
    fn a_name_only_in_dyld_still_resolves() {
        let exe = Img::new(0x1_0000_0000, 0).sym(0x1_0000_0460, "_main");
        let dyld = Img::new(0, retrace_box::DYLD_BASE).sym(0x1000, "_dyld_start");
        assert_eq!(both(&exe, &dyld).addrs_of("_dyld_start"),
                   vec![retrace_box::DYLD_BASE + 0x1000]);
    }

    #[test]
    fn the_executable_shadows_dyld_for_a_shared_name() {
        // The precedence rule is DECIDED (exe first), not inherited from the order of a Vec field.
        // Measured mitigation: dyld does not define _main (S4), so this collision is synthetic —
        // which is exactly why it needs a test rather than a measurement.
        let exe = Img::new(0x1_0000_0000, 0).sym(0x1_0000_0460, "_shared");
        let dyld = Img::new(0, retrace_box::DYLD_BASE).sym(0x1000, "_shared");
        assert_eq!(both(&exe, &dyld).addrs_of("_shared"), vec![0x1_0000_0460],
            "the guest's own symbol must win, and dyld's must not be appended to it");
    }

    #[test]
    fn a_data_symbol_is_reachable_by_name_but_not_by_address() {
        // Confirms S6, which the measurements document recorded as source-read rather than measured:
        // `for_image` keeps any defined section, so a __DATA symbol IS in the table, while
        // `resolve`'s text_end clamp means address -> name can never return it.
        //
        // This does NOT license `watch <name>`: S5 blocks that independently, because nlist_64
        // carries no size and a watch of invented width silently misses writes.
        let past_text = 0x1_0000_0000u64 + 0x8000; // text_vmsize is 0x4000, so this is past text_end
        let img = Img::new(0x1_0000_0000, 0)
            .sym(0x1_0000_0460, "_main")
            .raw_sym(past_text, "_a_global", N_SECT, 2);
        let t = table(&img);
        assert_eq!(t.addrs_of("_a_global"), vec![past_text], "name -> address reaches data");
        assert_eq!(t.resolve(past_text), None, "address -> name must still stop at text_end");
    }
}
