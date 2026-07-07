//! Pure decoder for the dyld shared-cache "v5" slide-info pointer format.
//!
//! No VM, no file I/O — this module only decodes the 64-bit slot value found in a v5
//! slide-info fixup chain and computes the target VA / auth modifier arithmetic needed to
//! rebase (and, for auth slots, re-sign) a cache pointer. Bit layout verified byte-for-byte
//! against real cache bytes in `spikes/cacheprobe.c`; see `.superpowers/sdd/m2cache-spike-findings.md`.
//!
//! `dyld_cache_slide_pointer5` bit layout (bit 63 is MSB):
//! ```text
//! auth    (bit63==1): runtimeOffset[33:0] diversity[49:34] addrDiv[50] keyIsData[51] next[62:52] auth[63]
//! regular (bit63==0): runtimeOffset[33:0] high8[41:34]     unused[49:42]            next[62:52] auth[63]
//! ```
//! (`high8` occupies the low 8 bits of the same 16-bit span that `diversity` uses in the auth
//! case — bits [49:42] are unused padding in the regular case. Confirmed against
//! `spikes/cacheprobe.c`'s `decode_v5`, which reads `high8 = (raw >> 34) & 0xFF`.)
//!
//! Task 1 of the M2-cache sub-milestone landed the pure bit-layout decoder above (no I/O).
//! Task 2 (this addition) adds `CacheMeta`: it *does* do read-only file I/O — it parses the
//! on-disk `dyld_cache_header`/`mapping_and_slide_info[]` across the main cache file and its
//! subcache files, and builds an IPA -> (subcache file, page, slide-info) routing table. Still
//! no VM: this is pure host file parsing, consumed later by the lazy per-page pager, which will
//! walk `page_starts[]` chains and call `decode5`/`target_va`/`modifier` per slot. Allow
//! dead_code until that pager lands and wires these up.
#![allow(dead_code)]

use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

/// A decoded v5 shared-cache slide-info pointer slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlidePtr5 {
    /// bit 63: this slot is an authenticated (PAC-signed) pointer, not a plain rebase.
    pub auth: bool,
    /// bits [33:0]: offset from `value_add` (the cache's unslid base) to the pointer's target.
    pub runtime_offset: u64,
    /// bits [49:34], auth slots only: the 16-bit diversifier data.
    pub diversity: u16,
    /// bit 50, auth slots only: whether the modifier is blended with the slot's own address.
    pub addr_div: bool,
    /// bit 51, auth slots only: which A-family key to use (`true` => DA, `false` => IA).
    pub key_is_data: bool,
    /// bits [41:34], regular slots only: the top byte to OR into the final pointer's bits [63:56].
    pub high8: u8,
    /// bits [62:52]: offset (in 8-byte units) to the next slot in this page's fixup chain; 0 ends it.
    pub next: u16,
}

/// Decode a raw v5 slide-info slot value.
pub fn decode5(slot: u64) -> SlidePtr5 {
    let auth = (slot >> 63) & 1 != 0;
    let next = ((slot >> 52) & 0x7FF) as u16;
    let runtime_offset = slot & 0x3_FFFF_FFFF;
    let sixteen = ((slot >> 34) & 0xFFFF) as u16;
    SlidePtr5 {
        auth,
        runtime_offset,
        diversity: if auth { sixteen } else { 0 },
        addr_div: auth && (slot >> 50) & 1 != 0,
        key_is_data: auth && (slot >> 51) & 1 != 0,
        high8: if auth { 0 } else { (sixteen & 0xFF) as u8 },
        next,
    }
}

/// The final (unsigned) target VA this slot points at, given the cache's `value_add`
/// (unslid base) and the chosen cache `slide`. Valid for both auth and regular slots — for
/// regular slots the caller must still OR in `high8 << 56` to get the final on-disk pointer
/// bits; for auth slots this VA is what gets PAC-signed with `modifier`/`key`.
pub fn target_va(p: &SlidePtr5, value_add: u64, slide: u64) -> u64 {
    value_add.wrapping_add(p.runtime_offset).wrapping_add(slide)
}

/// ptrauth ABI blend of a discriminator address and 16-bit diversity into a 64-bit modifier.
pub fn blend(addr: u64, diversity: u16) -> u64 {
    (addr & 0x0000_FFFF_FFFF_FFFF) | ((diversity as u64) << 48)
}

/// The PAC signing modifier for an auth slot: `diversity` alone, or blended with the slot's
/// own (slid) VA when `addr_div` is set. `slot_slid_va` is the fixup slot's own address at the
/// chosen cache slide (i.e. `slotUnslidVA + slide`), not the pointer's target.
pub fn modifier(p: &SlidePtr5, slot_slid_va: u64) -> u64 {
    if p.addr_div {
        blend(slot_slid_va, p.diversity)
    } else {
        p.diversity as u64
    }
}

// ---- cache metadata loader + IPA routing table (Task 2) ----
//
// Layout verified against real bytes in `spikes/cacheprobe.c` / `.superpowers/sdd/m2cache-spike-findings.md`:
//   dyld_cache_header: sharedRegionStart@0xE0 (u64), sharedRegionSize@0xE8 (u64),
//                       mappingWithSlideOffset@0x138 (u32), mappingWithSlideCount@0x13C (u32).
//   dyld_cache_mapping_and_slide_info (56 bytes): address@0, size@8, fileOffset@16,
//                       slideInfoFileOffset@24, slideInfoFileSize@32, flags@40 (u64),
//                       maxProt@44 (u32), initProt@48 (u32).
//   slide_info5 (v5 only): version@0 (u32), page_size@4 (u32), page_starts_count@8 (u32),
//                       value_add@0x10 (u64), page_starts[]@0x18 (u16 each, 0xFFFF = no rebase).
//
// Every subcache file (main + numbered `.NN[.suffix]` subcaches) is its own full
// `dyld_cache_header`; all map into one contiguous VA window starting at `sharedRegionStart`.
// TEXT mappings (r-x) have `slideInfoFileOffset == 0` (no fixups, `slide_info: None`); DATA
// mappings (in the `.NN.dylddata` subcaches) carry a v5 slide-info blob.

fn u32le(b: &[u8], o: usize) -> u32 { u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) }
fn u64le(b: &[u8], o: usize) -> u64 { u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) }

/// The known on-disk cache slide-info page size (16 KiB), used as the page granularity for
/// mappings that carry no slide-info (TEXT) since those still route/page at this stride.
const CACHE_PAGE_SIZE: u64 = 0x4000;

/// A parsed v5 slide-info blob (`dyld_cache_slide_info5`) for one DATA mapping.
#[derive(Debug, Clone)]
pub struct SlideInfo5 {
    pub page_size: u32,
    pub value_add: u64,
    pub page_starts: Vec<u16>,
}

/// One `dyld_cache_mapping_and_slide_info` region within a subcache file, plus its parsed
/// slide-info (`None` for TEXT mappings, which carry no fixups).
#[derive(Debug, Clone)]
struct Mapping {
    address: u64,
    size: u64,
    file_offset: u64,
    is_exec: bool,
    slide_info: Option<SlideInfo5>,
}

/// A subcache file (the main cache file, or one of its numbered `.NN[.suffix]` companions) and
/// its parsed mappings.
#[derive(Debug)]
struct Subcache {
    path: PathBuf,
    mappings: Vec<Mapping>,
}

/// Where a faulting cache IPA lives: which subcache file backs it, the file offset of the page
/// containing it, its v5 slide-info (DATA only), whether it's executable, and its page index
/// within that mapping.
#[derive(Debug)]
pub struct CacheRegion<'a> {
    pub subcache_path: &'a Path,
    pub file_offset_of_page: u64,
    pub slide_info: Option<&'a SlideInfo5>,
    pub is_exec: bool,
    pub page_index: usize,
}

/// Parsed metadata for the whole dyld shared cache (main file + subcache files): enough to
/// route a faulting cache IPA to the right subcache file, page, and (for DATA) slide-info.
#[derive(Debug)]
pub struct CacheMeta {
    base: u64,
    size: u64,
    subcaches: Vec<Subcache>,
}

/// Read one subcache file's `dyld_cache_header` + `mapping_and_slide_info[]` (and each
/// mapping's slide-info blob, if any). Panics loudly if a slide-info blob isn't version 5 —
/// this loader only understands v5 and must never silently mis-load an unsupported format.
fn read_subcache(path: &Path) -> io::Result<(u64, u64, Vec<Mapping>)> {
    let file = File::open(path)?;

    let mut hdr = [0u8; 0x140];
    file.read_exact_at(&mut hdr, 0)?;
    assert_eq!(&hdr[0..7], b"dyld_v1", "{}: not a dyld shared-cache header (bad magic)", path.display());

    let shared_region_start = u64le(&hdr, 0xE0);
    let shared_region_size = u64le(&hdr, 0xE8);
    let mapping_slide_offset = u32le(&hdr, 0x138) as u64;
    let mapping_slide_count = u32le(&hdr, 0x13C);

    let mut mappings = Vec::with_capacity(mapping_slide_count as usize);
    for i in 0..mapping_slide_count as u64 {
        let mut m = [0u8; 56];
        file.read_exact_at(&mut m, mapping_slide_offset + i * 56)?;
        let address = u64le(&m, 0);
        let size = u64le(&m, 8);
        let file_offset = u64le(&m, 16);
        let slide_info_file_offset = u64le(&m, 24);
        let slide_info_file_size = u64le(&m, 32);
        let init_prot = u32le(&m, 48);
        let is_exec = init_prot & 0x4 != 0; // VM_PROT_EXECUTE

        let slide_info = if slide_info_file_offset != 0 && slide_info_file_size != 0 {
            Some(read_slide_info5(&file, path, slide_info_file_offset)?)
        } else {
            None
        };

        mappings.push(Mapping { address, size, file_offset, is_exec, slide_info });
    }

    Ok((shared_region_start, shared_region_size, mappings))
}

/// Read one `dyld_cache_slide_info5` blob at file offset `offset`. Asserts `version == 5`
/// (fails loudly on anything else — no silent mis-load of an unhandled slide format).
fn read_slide_info5(file: &File, path: &Path, offset: u64) -> io::Result<SlideInfo5> {
    let mut prefix = [0u8; 0x18];
    file.read_exact_at(&mut prefix, offset)?;
    let version = u32le(&prefix, 0x00);
    assert_eq!(
        version, 5,
        "{}: unsupported slide-info version {version} at file offset 0x{offset:x} (only v5 is handled)",
        path.display()
    );
    let page_size = u32le(&prefix, 0x04);
    let page_starts_count = u32le(&prefix, 0x08) as u64;
    let value_add = u64le(&prefix, 0x10);

    let mut raw = vec![0u8; (page_starts_count * 2) as usize];
    file.read_exact_at(&mut raw, offset + 0x18)?;
    let page_starts = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();

    Ok(SlideInfo5 { page_size, value_add, page_starts })
}

impl CacheMeta {
    /// Parse the main cache file plus its numbered subcache files (`<main_path>.NN[.suffix]`,
    /// contiguous from `01`, each a full `dyld_cache_header`) into a routing table.
    ///
    /// Subcache enumeration: per `spikes/cacheprobe.c` / the spike findings, subcache files sit
    /// alongside the main file as `.NN` (TEXT) or `.NN.<suffix>` for a small, fixed suffix set
    /// (`dylddata`, `dyldreadonly`, `dyldlinkedit`). We probe `NN = 1, 2, ...` against each known
    /// suffix and stop at the first `NN` with no match — this cache's subcaches are contiguous
    /// (`.01` .. `.12` on the verified host, no gaps).
    pub fn load(main_path: impl AsRef<Path>) -> io::Result<CacheMeta> {
        let main_path = main_path.as_ref();

        let (base, size, main_mappings) = read_subcache(main_path)?;
        let mut subcaches = vec![Subcache { path: main_path.to_path_buf(), mappings: main_mappings }];

        const SUFFIXES: [&str; 4] = ["", ".dylddata", ".dyldreadonly", ".dyldlinkedit"];
        let mut n = 1u32;
        loop {
            let found = SUFFIXES
                .iter()
                .map(|suf| PathBuf::from(format!("{}.{n:02}{suf}", main_path.display())))
                .find(|p| p.is_file());
            let Some(path) = found else { break };
            let (_, _, mappings) = read_subcache(&path)?;
            subcaches.push(Subcache { path, mappings });
            n += 1;
        }

        Ok(CacheMeta { base, size, subcaches })
    }

    /// The cache's VA window: `(sharedRegionStart, sharedRegionSize)` from the main header.
    pub fn window(&self) -> (u64, u64) {
        (self.base, self.size)
    }

    /// Route a cache IPA to its containing subcache/mapping, if any.
    pub fn region_of(&self, ipa: u64) -> Option<CacheRegion<'_>> {
        for sc in &self.subcaches {
            for m in &sc.mappings {
                if ipa < m.address || ipa >= m.address + m.size {
                    continue;
                }
                let page_size = m.slide_info.as_ref().map_or(CACHE_PAGE_SIZE, |si| si.page_size as u64);
                let page_index = ((ipa - m.address) / page_size) as usize;
                let file_offset_of_page = m.file_offset + page_index as u64 * page_size;
                return Some(CacheRegion {
                    subcache_path: &sc.path,
                    file_offset_of_page,
                    slide_info: m.slide_info.as_ref(),
                    is_exec: m.is_exec,
                    page_index,
                });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_spike_auth_slot() {
        // From cacheprobe.c: .02.dylddata DATA page1 off 0x22d0
        let p = decode5(0x801dab846c2f15c8);
        assert!(p.auth && p.key_is_data /*DA*/ && p.addr_div);
        assert_eq!(p.runtime_offset, 0x6c2f15c8);
        assert_eq!(p.diversity, 0x6ae1);
        assert_eq!(p.next, 1);
        assert_eq!(target_va(&p, 0x180000000, 0), 0x1ec2f15c8);
        // modifier = blend(slot_slid_va, diversity)
        let slot = 0x1ec06c2d0u64; // slotUnslidVA @ slide 0
        assert_eq!(modifier(&p, slot), (slot & 0x0000_FFFF_FFFF_FFFF) | (0x6ae1u64 << 48));
        // a regular slot
        let r = decode5(0x001000010f3bec00);
        assert!(!r.auth);
        assert_eq!(target_va(&r, 0x180000000, 0), 0x28f3bec00);
    }

    const CACHE: &str =
        "/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld/dyld_shared_cache_arm64e";

    #[test]
    fn cache_meta_routes_ipas_across_subcaches() {
        let meta = CacheMeta::load(CACHE).expect("load real dyld shared cache");

        // window() must report the main header's (sharedRegionStart, sharedRegionSize),
        // cross-checked independently by reading the main file's raw header bytes here.
        let raw = std::fs::read(CACHE).expect("read main cache header bytes");
        let expected_start = u64::from_le_bytes(raw[0xE0..0xE8].try_into().unwrap());
        let expected_size = u64::from_le_bytes(raw[0xE8..0xF0].try_into().unwrap());
        assert_eq!(expected_start, 0x1_8000_0000);
        assert_eq!(meta.window(), (0x1_8000_0000, expected_size));

        // A known exec VA routes to a TEXT subcache: is_exec, no slide-info.
        let text = meta.region_of(0x1_80cc_b568).expect("exec VA must route to a region");
        assert!(text.is_exec, "TEXT VA must be exec");
        assert!(text.slide_info.is_none(), "TEXT mapping must carry no slide-info");
        assert!(
            text.subcache_path.to_string_lossy().ends_with(".01"),
            "expected the .01 TEXT subcache, got {:?}",
            text.subcache_path
        );

        // Derive a DATA VA from the parsed mappings (don't hardcode a guess): the first mapping
        // anywhere in the cache carrying v5 slide-info.
        let data_va = meta
            .subcaches
            .iter()
            .flat_map(|sc| sc.mappings.iter())
            .find(|m| m.slide_info.is_some())
            .expect("cache must have at least one DATA mapping with slide-info")
            .address;

        let data = meta.region_of(data_va).expect("DATA VA must route to a region");
        assert!(!data.is_exec, "DATA mapping must not be exec");
        assert!(
            data.subcache_path.to_string_lossy().contains(".dylddata"),
            "expected a .dylddata subcache, got {:?}",
            data.subcache_path
        );
        let si = data.slide_info.expect("DATA mapping must carry a v5 SlideInfo5");
        assert_eq!(si.page_size, 16384);
        assert_eq!(si.value_add, 0x1_8000_0000);
    }
}
