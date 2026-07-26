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
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileExt, MetadataExt};
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

/// A collected **auth** (PAC-signed) pointer slot from a page's v5 fixup chain, awaiting
/// re-signing by the guest's own PAC keys. That signing is a later guest-side oracle's job (it
/// needs the guest's key material); this module only walks the chain and computes the
/// arithmetic (`target_va`/`modifier`) the oracle will need — it never signs anything itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthSlot {
    /// Byte offset of this slot within its 16 KiB page.
    pub offset: usize,
    /// The (unsigned) VA this slot's pointer targets, per `target_va`.
    pub target_va: u64,
    /// Which A-family key to sign with (`true` => DA, `false` => IA).
    pub key_is_data: bool,
    /// The PAC signing modifier for this slot, per `modifier`.
    pub modifier: u64,
}

/// Walk one 16 KiB DATA page's v5 chained-fixup list and rebase it in place.
///
/// `si.page_starts[page_index]` is the byte offset of the chain's first slot within the page
/// (`0xFFFF` => no fixups on this page, returns empty and leaves `page` untouched). Each 8-byte
/// slot's `next` field (8-byte units, `0` ends the chain) advances the walk; per
/// `spikes/cacheprobe.c` the chain is always self-contained within one page.
///
/// **Regular** slots are rewritten in place to their final on-disk pointer value —
/// `(value_add + runtime_offset + slide) | (high8 << 56)` — pure host arithmetic, no PAC key
/// needed. **Auth** slots are left untouched in `page` and instead collected into the returned
/// `Vec`, because signing them needs the guest's PAC keys (a later task's guest-side oracle, not
/// this one).
///
/// `mapping_base` is the VA of the DATA mapping this page belongs to (`page_index` is relative
/// to that mapping's own page array, not the whole cache) — combined with `page_index` and each
/// slot's byte offset, it gives the slot's own unslid VA, which an auth slot's `modifier` blends
/// in when `addr_div` is set (see `modifier`'s doc comment for `slot_slid_va`).
pub fn walk_page(page: &mut [u8; 16384], si: &SlideInfo5, page_index: usize, slide: u64, mapping_base: u64) -> Vec<AuthSlot> {
    let mut auth_slots = Vec::new();

    let start = si.page_starts[page_index];
    if start == 0xFFFF {
        return auth_slots;
    }

    let mut off = start as usize;
    loop {
        assert!(off + 8 <= page.len(), "v5 fixup chain left its page (page_index {page_index}, offset 0x{off:x})");
        let raw = u64::from_le_bytes(page[off..off + 8].try_into().unwrap());
        let p = decode5(raw);
        let target = target_va(&p, si.value_add, slide);

        if p.auth {
            let slot_unslid_va = mapping_base.wrapping_add(page_index as u64 * si.page_size as u64).wrapping_add(off as u64);
            let slot_slid_va = slot_unslid_va.wrapping_add(slide);
            auth_slots.push(AuthSlot {
                offset: off,
                target_va: target,
                key_is_data: p.key_is_data,
                modifier: modifier(&p, slot_slid_va),
            });
        } else {
            let final_ptr = target | ((p.high8 as u64) << 56);
            page[off..off + 8].copy_from_slice(&final_ptr.to_le_bytes());
        }

        if p.next == 0 {
            break;
        }
        off += p.next as usize * 8;
    }

    auth_slots
}

// ---- cache metadata loader + IPA routing table (Task 2) ----
//
// Layout verified against real bytes in `spikes/cacheprobe.c` / `.superpowers/sdd/m2cache-spike-findings.md`:
//   dyld_cache_header: sharedRegionStart@0xE0 (u64), sharedRegionSize@0xE8 (u64),
//                       mappingWithSlideOffset@0x138 (u32), mappingWithSlideCount@0x13C (u32).
//   dyld_cache_mapping_and_slide_info (56 bytes): address@0, size@8, fileOffset@16,
//                       slideInfoFileOffset@24, slideInfoFileSize@32, flags@40 (u64),
//                       maxProt@48 (u32), initProt@52 (u32). (Confirmed via
//                       `offsetof`/`sizeof` on the real struct in `spikes/covprobe.c`: flags is
//                       8 bytes, so the two trailing u32 prot fields sit at 48/52, not 44/48.)
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

/// The system dyld shared cache for this architecture (arm64e); `Box_::install_cache_pager`
/// loads this file (plus its subcaches) into a [`CacheMeta`] routing table.
pub const DEFAULT_CACHE_PATH: &str =
    "/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld/dyld_shared_cache_arm64e";

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
/// its parsed mappings. `file` is kept open (the pager `pread`s pristine cache pages from it on
/// demand — SPTM forbids ever `hv_vm_map`ing a file page); `None` only for synthetic
/// (test-constructed) subcaches that carry no backing file.
#[derive(Debug)]
struct Subcache {
    path: PathBuf,
    file: Option<File>,
    mappings: Vec<Mapping>,
}

/// Where a faulting cache IPA lives: which subcache file backs it, the file offset of the page
/// containing it, its v5 slide-info (DATA only), whether it's executable, its page index within
/// that mapping, and that mapping's base VA (the `mapping_base` a v5 auth slot's `addrDiv`
/// modifier blends in — see [`walk_page`]).
#[derive(Debug)]
pub struct CacheRegion<'a> {
    pub subcache_path: &'a Path,
    pub file_offset_of_page: u64,
    pub slide_info: Option<&'a SlideInfo5>,
    pub is_exec: bool,
    pub page_index: usize,
    pub mapping_base: u64,
}

/// Parsed metadata for the whole dyld shared cache (main file + subcache files): enough to
/// route a faulting cache IPA to the right subcache file, page, and (for DATA) slide-info.
#[derive(Debug)]
pub struct CacheMeta {
    base: u64,
    size: u64,
    /// The per-process dynamic data region the kernel creates at the top of the shared region
    /// (`base + dynamicDataOffset`, header field at 0x1f0) and its max size (header field at
    /// 0x1f8). NOT a file mapping — dyld reads a `"dyld_data"` magic header here to fetch the
    /// cache's file id. `(0, 0)` if the header carries no dynamic data region.
    dynamic_data_addr: u64,
    dynamic_data_size: u64,
    /// The cache file's `(st_dev, st_ino)` — the `FileIdTuple` (fsid, fsobjid) the kernel writes
    /// into the dynamic data region so dyld's `getDyldCacheFileID` succeeds (both must be
    /// non-zero). Deterministic function of the on-disk cache file.
    file_dev: u64,
    file_ino: u64,
    subcaches: Vec<Subcache>,
}

/// Read one subcache file's `dyld_cache_header` + `mapping_and_slide_info[]` (and each
/// mapping's slide-info blob, if any). Panics loudly if a slide-info blob isn't version 5 —
/// this loader only understands v5 and must never silently mis-load an unsupported format.
fn read_subcache(path: &Path) -> io::Result<(u64, u64, Vec<Mapping>, File)> {
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
        let init_prot = u32le(&m, 52);
        let is_exec = init_prot & 0x4 != 0; // VM_PROT_EXECUTE

        let slide_info = if slide_info_file_offset != 0 && slide_info_file_size != 0 {
            Some(read_slide_info5(&file, path, slide_info_file_offset)?)
        } else {
            None
        };

        mappings.push(Mapping { address, size, file_offset, is_exec, slide_info });
    }

    Ok((shared_region_start, shared_region_size, mappings, file))
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

/// Assert that the subcaches discovered by [`CacheMeta::load`]'s suffix-probe loop actually
/// cover the cache's whole VA window `[base, base + size)`, so an enumeration that stopped
/// early (a missing/renamed subcache file) is a loud load-time panic instead of a silent
/// `region_of` -> `None` for the un-enumerated tail.
///
/// Calibrated against the real cache (`spikes/covprobe.c`, run against
/// `dyld_shared_cache_arm64e` + its 12 subcaches on this host): mappings are **not** gapless
/// byte-for-byte — a subcache's own mapping list can have multi-megabyte gaps between segments
/// (e.g. unmapped guard space between `.02.dylddata`'s `__DATA_CONST` and `__AUTH_CONST`
/// mappings), so we don't require that. But every subcache *file* boundary is exactly
/// contiguous with the next (main -> `.01` -> `.02.dylddata` -> ... -> `.12.dyldlinkedit`, each
/// one's lowest mapping address equals the previous file's highest mapping end, verified on the
/// real cache), and the whole chain ends within one `CACHE_PAGE_SIZE` of the window's end (the
/// real cache's last 0x4000 bytes are an unmapped trailing guard page). So: assert each
/// subcache file's mappings pick up exactly where the previous file's left off, and that the
/// last one reaches within a page of the window end.
fn assert_covers_window(base: u64, size: u64, subcaches: &[Subcache]) {
    let window_end = base + size;
    let mut cover_end = base;
    for sc in subcaches {
        if sc.mappings.is_empty() {
            continue;
        }
        let sc_min = sc.mappings.iter().map(|m| m.address).min().unwrap();
        let sc_max = sc.mappings.iter().map(|m| m.address + m.size).max().unwrap();
        assert_eq!(
            sc_min, cover_end,
            "cache routing table incomplete: {} subcache file(s) found, but {} starts at 0x{sc_min:x} \
             while the previous subcache(s) only covered up to 0x{cover_end:x} — first uncovered \
             address 0x{cover_end:x}. Subcache enumeration likely stopped early or skipped a file.",
            subcaches.len(),
            sc.path.display(),
        );
        cover_end = cover_end.max(sc_max);
    }
    assert!(
        cover_end + CACHE_PAGE_SIZE >= window_end,
        "cache routing table incomplete: {} subcache file(s) found, covering only up to 0x{cover_end:x}, \
         but the cache window is [0x{base:x}, 0x{window_end:x}) — first uncovered address 0x{cover_end:x}. \
         Subcache enumeration likely stopped before the last subcache.",
        subcaches.len(),
    );
}

impl CacheMeta {
    /// Parse the main cache file plus its numbered subcache files (`<main_path>.NN[.suffix]`,
    /// contiguous from `01`, each a full `dyld_cache_header`) into a routing table.
    ///
    /// Subcache enumeration: per `spikes/cacheprobe.c` / the spike findings, subcache files sit
    /// alongside the main file as `.NN` (TEXT) or `.NN.<suffix>` for a small, fixed suffix set
    /// (`dylddata`, `dyldreadonly`, `dyldlinkedit`). We probe `NN = 1, 2, ...` against each known
    /// suffix and stop at the first `NN` with no match. This is a heuristic (no documented
    /// "subcache count" header field to parse instead), so `load` asserts afterwards
    /// (`assert_covers_window`) that the discovered mappings actually cover the whole
    /// `[sharedRegionStart, sharedRegionStart + sharedRegionSize)` window — if the probe stopped
    /// before the real last subcache (e.g. a future suffix we don't know about), that is a
    /// load-time panic naming the gap, not a silent `region_of(ipa) -> None` later.
    pub fn load(main_path: impl AsRef<Path>) -> io::Result<CacheMeta> {
        let main_path = main_path.as_ref();

        let (base, size, main_mappings, main_file) = read_subcache(main_path)?;
        // dyld_cache_header.dynamicDataOffset (0x1f0) / dynamicDataMaxSize (0x1f8): the kernel-made
        // per-process region at the top of the shared region (see `dynamic_data_region`).
        let mut dd = [0u8; 16];
        main_file.read_exact_at(&mut dd, 0x1f0)?;
        let dyn_off = u64le(&dd, 0);
        let dyn_size = u64le(&dd, 8);
        let dynamic_data_addr = if dyn_off != 0 { base.wrapping_add(dyn_off) } else { 0 };
        // The cache file's (dev, ino): the kernel's FileIdTuple in the dynamic data region.
        let cmeta = std::fs::metadata(main_path)?;
        let (file_dev, file_ino) = (cmeta.dev(), cmeta.ino());
        let mut subcaches = vec![Subcache { path: main_path.to_path_buf(), file: Some(main_file), mappings: main_mappings }];

        const SUFFIXES: [&str; 4] = ["", ".dylddata", ".dyldreadonly", ".dyldlinkedit"];
        let mut n = 1u32;
        loop {
            let found = SUFFIXES
                .iter()
                .map(|suf| PathBuf::from(format!("{}.{n:02}{suf}", main_path.display())))
                .find(|p| p.is_file());
            let Some(path) = found else { break };
            let (_, _, mappings, file) = read_subcache(&path)?;
            subcaches.push(Subcache { path, file: Some(file), mappings });
            n += 1;
        }

        assert_covers_window(base, size, &subcaches);

        Ok(CacheMeta { base, size, dynamic_data_addr, dynamic_data_size: dyn_size, file_dev, file_ino, subcaches })
    }

    /// The `dyld_cache_dynamic_data_header` (plus the cache-path string) the kernel writes at the
    /// start of the dynamic data region. dyld reads several fields here during launch:
    /// - `+0x00` magic `"dyld_data    v3"` (16 bytes) — `dynamicRegion()` checks it, else returns
    ///   null → fatal "mapped cache does not contain dynamic config data".
    /// - `+0x10` fsid, `+0x18` fsobjid — the `FileIdTuple` (`getDyldCacheFileID` requires both
    ///   non-zero); we use the cache file's `(st_dev, st_ino)`.
    /// - `+0x24` cachePathOffset (u32) — byte offset from the region base to a NUL-terminated cache
    ///   path string (`DynamicRegion::cachePath()`; used to build the process-info atlas —
    ///   `gatherAtlasProcessInfo` `strlen`s it, so it must be a real non-null string).
    ///
    /// We lay the path at `+0x30`. Deterministic function of the on-disk cache file.
    pub fn dynamic_data_header(&self) -> Vec<u8> {
        const PATH_OFF: usize = 0x30;
        let path = self.subcaches[0].path.as_os_str().as_bytes();
        let mut h = vec![0u8; PATH_OFF + path.len() + 1]; // +1: NUL terminator (stays 0)
        h[0..16].copy_from_slice(b"dyld_data    v3\0");
        h[16..24].copy_from_slice(&self.file_dev.to_le_bytes());
        h[24..32].copy_from_slice(&self.file_ino.to_le_bytes());
        h[0x24..0x28].copy_from_slice(&(PATH_OFF as u32).to_le_bytes());
        h[PATH_OFF..PATH_OFF + path.len()].copy_from_slice(path);
        h
    }

    /// The cache's VA window: `(sharedRegionStart, sharedRegionSize)` from the main header.
    pub fn window(&self) -> (u64, u64) {
        (self.base, self.size)
    }

    /// Every executable cache mapping as `(address, size)` at the unslid base (slide 0). The loader
    /// pre-sets these ranges' stage-1 to RO+exec (`ATTR_CODE`) BEFORE the guest translates them, so
    /// a cache TEXT page faults in as a pure stage-2 translation fault with no runtime stage-1
    /// change — avoiding the stale-block-TLB gap that a runtime data→exec promotion would leave
    /// (a page first translated as a default 32 MiB RW/UXN block would keep executing under the
    /// stale non-exec entry with no way for the VMM to issue a guest TLBI).
    pub fn exec_mappings(&self) -> Vec<(u64, u64)> {
        self.subcaches
            .iter()
            .flat_map(|sc| sc.mappings.iter())
            .filter(|m| m.is_exec && m.size > 0)
            .map(|m| (m.address, m.size))
            .collect()
    }

    /// The kernel-created per-process **dynamic data region** `(addr, size)` at the top of the
    /// shared region (`base + dynamicDataOffset`), or `None` if the header declares none. This is
    /// NOT one of the cache's file mappings — dyld reads a `"dyld_data"` magic header here to fetch
    /// the cache file id, so the pager must stage it (we zero it → magic mismatch → dyld's
    /// `dynamicRegion()` returns null, i.e. "no dynamic data", which it tolerates).
    pub fn dynamic_data_region(&self) -> Option<(u64, u64)> {
        (self.dynamic_data_addr != 0 && self.dynamic_data_size != 0)
            .then_some((self.dynamic_data_addr, self.dynamic_data_size))
    }

    /// Route a cache IPA to its containing subcache/mapping and page index (relative to that
    /// mapping's own page array), plus the page granularity for that mapping. Shared by
    /// [`region_of`](Self::region_of) and [`stage_page`](Self::stage_page).
    fn locate(&self, ipa: u64) -> Option<(&Subcache, &Mapping, usize, u64)> {
        for sc in &self.subcaches {
            for m in &sc.mappings {
                if ipa < m.address || ipa >= m.address + m.size {
                    continue;
                }
                let page_size = m.slide_info.as_ref().map_or(CACHE_PAGE_SIZE, |si| si.page_size as u64);
                let page_index = ((ipa - m.address) / page_size) as usize;
                return Some((sc, m, page_index, page_size));
            }
        }
        None
    }

    /// Route a cache IPA to its containing subcache/mapping, if any (metadata only, no I/O).
    pub fn region_of(&self, ipa: u64) -> Option<CacheRegion<'_>> {
        let (sc, m, page_index, page_size) = self.locate(ipa)?;
        Some(CacheRegion {
            subcache_path: &sc.path,
            file_offset_of_page: m.file_offset + page_index as u64 * page_size,
            slide_info: m.slide_info.as_ref(),
            is_exec: m.is_exec,
            page_index,
            mapping_base: m.address,
        })
    }

    /// Route `ipa` to its cache page and `pread` that page's pristine bytes from the backing
    /// subcache file into `page`, returning the routing metadata. The bytes are the on-disk,
    /// slide-info-encoded page (DATA still needs [`walk_page`] + re-signing before use; TEXT is
    /// final). SPTM-safe: the caller stages these bytes into an anonymous guest page — a file page
    /// is never mapped into the guest. Returns `None` if `ipa` is outside every cache mapping.
    pub fn stage_page(&self, ipa: u64, page: &mut [u8; 16384]) -> Option<CacheRegion<'_>> {
        let (sc, m, page_index, page_size) = self.locate(ipa)?;
        let file_offset_of_page = m.file_offset + page_index as u64 * page_size;
        let file = sc.file.as_ref().expect("stage_page: subcache has no open file (synthetic CacheMeta?)");
        file.read_exact_at(page, file_offset_of_page).expect("stage_page: pread cache page");
        Some(CacheRegion {
            subcache_path: &sc.path,
            file_offset_of_page,
            slide_info: m.slide_info.as_ref(),
            is_exec: m.is_exec,
            page_index,
            mapping_base: m.address,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_subcache(name: &str, ranges: &[(u64, u64)]) -> Subcache {
        Subcache {
            path: PathBuf::from(name),
            file: None,
            mappings: ranges
                .iter()
                .map(|&(address, size)| Mapping { address, size, file_offset: 0, is_exec: false, slide_info: None })
                .collect(),
        }
    }

    #[test]
    fn coverage_assertion_accepts_contiguous_subcaches_and_trailing_guard_page() {
        // Mirrors the real cache's shape: subcache files are exactly contiguous with each
        // other, and the window's last CACHE_PAGE_SIZE bytes are an unmapped trailing guard
        // page with no backing mapping at all.
        let base = 0x1_8000_0000u64;
        let subcaches = vec![
            fake_subcache("main", &[(base, 0x1000)]),
            fake_subcache(".01", &[(base + 0x1000, 0x2000)]),
        ];
        let covered_end = base + 0x1000 + 0x2000;
        let size = (covered_end - base) + CACHE_PAGE_SIZE; // window extends one guard page past coverage
        assert_covers_window(base, size, &subcaches); // must not panic
    }

    #[test]
    #[should_panic(expected = "Subcache enumeration likely stopped before the last subcache")]
    fn coverage_assertion_rejects_incomplete_enumeration() {
        // Simulates a suffix-probe loop that stopped early: the discovered subcaches only
        // cover the first slice of the window, well beyond one guard page short of the end.
        let base = 0x1_8000_0000u64;
        let subcaches = vec![fake_subcache("main", &[(base, 0x1000)])];
        let size = 0x10_0000; // window is far larger than what got discovered
        assert_covers_window(base, size, &subcaches);
    }

    #[test]
    #[should_panic(expected = "Subcache enumeration likely stopped early or skipped a file")]
    fn coverage_assertion_rejects_gap_between_subcache_files() {
        // A gap between two discovered subcache files' mapping ranges: real subcache-file
        // boundaries are always exactly contiguous, so this signals a skipped/misrouted file.
        let base = 0x1_8000_0000u64;
        let subcaches = vec![
            fake_subcache("main", &[(base, 0x1000)]),
            fake_subcache(".01", &[(base + 0x2000, 0x1000)]), // gap: [base+0x1000, base+0x2000)
        ];
        let size = 0x3000;
        assert_covers_window(base, size, &subcaches);
    }

    /// Hand-encode a raw v5 slide-info slot value from its decoded fields (inverse of
    /// `decode5`), for building synthetic test pages.
    fn encode5(auth: bool, runtime_offset: u64, diversity: u16, addr_div: bool, key_is_data: bool, high8: u8, next: u16) -> u64 {
        let mut v = (runtime_offset & 0x3_FFFF_FFFF) | ((next as u64) << 52);
        if auth {
            v |= 1u64 << 63;
            v |= (diversity as u64) << 34;
            if addr_div {
                v |= 1u64 << 50;
            }
            if key_is_data {
                v |= 1u64 << 51;
            }
        } else {
            v |= (high8 as u64) << 34;
        }
        v
    }

    #[test]
    fn walk_page_rebases_regular_in_place_and_collects_auth_slots() {
        let value_add = 0x1_8000_0000u64;
        let page_size = 0x4000u32;
        let mapping_base = 0x1_ec00_0000u64;
        let page_index = 3usize;
        let slide = 0u64;

        let mut page = [0u8; 16384];
        let start_off = 0x100usize;

        // slot 0: regular — chains to slot 1.
        let reg_runtime_offset = 0x0012_3456u64;
        let reg_high8 = 0xABu8;
        let reg_raw = encode5(false, reg_runtime_offset, 0, false, false, reg_high8, 1);
        page[start_off..start_off + 8].copy_from_slice(&reg_raw.to_le_bytes());

        // slot 1: auth, addr_div=1 (blended modifier), DA — chains to slot 2.
        let auth1_off = start_off + 8;
        let auth1_runtime_offset = 0x0abc_def0u64;
        let auth1_div = 0x6ae1u16;
        let auth1_raw = encode5(true, auth1_runtime_offset, auth1_div, true, true, 0, 1);
        page[auth1_off..auth1_off + 8].copy_from_slice(&auth1_raw.to_le_bytes());

        // slot 2: auth, addr_div=0 (plain diversity modifier), DA — chains to slot 3.
        // [folds in the T1 coverage gap: T1's own tests only covered addr_div=1 + DA]
        let auth2_off = auth1_off + 8;
        let auth2_runtime_offset = 0x0011_2233u64;
        let auth2_div = 0x1234u16;
        let auth2_raw = encode5(true, auth2_runtime_offset, auth2_div, false, true, 0, 1);
        page[auth2_off..auth2_off + 8].copy_from_slice(&auth2_raw.to_le_bytes());

        // slot 3: auth, addr_div=1, IA (key_is_data=0) — ends the chain.
        // [folds in the T1 coverage gap: T1's own tests never exercised key_is_data=0]
        let auth3_off = auth2_off + 8;
        let auth3_runtime_offset = 0x0099_8877u64;
        let auth3_div = 0x5566u16;
        let auth3_raw = encode5(true, auth3_runtime_offset, auth3_div, true, false, 0, 0);
        page[auth3_off..auth3_off + 8].copy_from_slice(&auth3_raw.to_le_bytes());

        let mut page_starts = vec![0xFFFFu16; page_index + 1];
        page_starts[page_index] = start_off as u16;
        let si = SlideInfo5 { page_size, value_add, page_starts };

        let orig_auth_bytes: Vec<([u8; 8], usize)> = [auth1_off, auth2_off, auth3_off]
            .iter()
            .map(|&o| (page[o..o + 8].try_into().unwrap(), o))
            .collect();

        let auth_slots = walk_page(&mut page, &si, page_index, slide, mapping_base);

        // Regular slot rebased in place: (value_add + runtime_offset + slide) | (high8 << 56).
        let expected_reg = value_add.wrapping_add(reg_runtime_offset).wrapping_add(slide) | ((reg_high8 as u64) << 56);
        let got_reg = u64::from_le_bytes(page[start_off..start_off + 8].try_into().unwrap());
        assert_eq!(got_reg, expected_reg);

        // Auth slots are NOT written — bytes at their offsets are untouched.
        for (orig, off) in &orig_auth_bytes {
            assert_eq!(&page[*off..*off + 8], &orig[..]);
        }

        assert_eq!(auth_slots.len(), 3);

        // Auth slot 1: addr_div=1 => modifier = blend(slot_slid_va, diversity); DA.
        let slot1_va = mapping_base + page_index as u64 * page_size as u64 + auth1_off as u64 + slide;
        assert_eq!(auth_slots[0].offset, auth1_off);
        assert_eq!(auth_slots[0].target_va, value_add.wrapping_add(auth1_runtime_offset).wrapping_add(slide));
        assert!(auth_slots[0].key_is_data);
        assert_eq!(auth_slots[0].modifier, blend(slot1_va, auth1_div));

        // Auth slot 2: addr_div=0 => modifier = diversity alone; DA.
        assert_eq!(auth_slots[1].offset, auth2_off);
        assert_eq!(auth_slots[1].target_va, value_add.wrapping_add(auth2_runtime_offset).wrapping_add(slide));
        assert!(auth_slots[1].key_is_data);
        assert_eq!(auth_slots[1].modifier, auth2_div as u64);

        // Auth slot 3: IA (key_is_data=0), addr_div=1 => modifier still blends.
        let slot3_va = mapping_base + page_index as u64 * page_size as u64 + auth3_off as u64 + slide;
        assert_eq!(auth_slots[2].offset, auth3_off);
        assert_eq!(auth_slots[2].target_va, value_add.wrapping_add(auth3_runtime_offset).wrapping_add(slide));
        assert!(!auth_slots[2].key_is_data);
        assert_eq!(auth_slots[2].modifier, blend(slot3_va, auth3_div));
    }

    #[test]
    fn walk_page_no_rebase_returns_empty_and_leaves_page_unchanged() {
        let si = SlideInfo5 { page_size: 0x4000, value_add: 0x1_8000_0000, page_starts: vec![0xFFFF] };
        let mut page = [0u8; 16384];
        // Poison the page with non-zero bytes so an accidental write would be detectable.
        for (i, b) in page.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        let before = page;

        let auth_slots = walk_page(&mut page, &si, 0, 0, 0x1_ec00_0000);

        assert!(auth_slots.is_empty());
        assert_eq!(page, before);
    }

    #[test]
    fn decodes_spike_auth_slot() {
        // From cacheprobe.c: .02.dylddata DATA page1 off 0x22d0 — historic worked example from an
        // older cache build (see cache_pager.rs's provenance comment for the live one). This test
        // decodes a fixed raw descriptor literal and checks arithmetic on it, cache-independent,
        // so it does not need re-deriving.
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
