// M2-cache Task 5: cache pager integration. Proves the pager MECHANISM (not the full dyld run —
// that is Task 6): install the pager, directly `page_in_cache` the demand-paged page, and check
// the two paths.
//
//   DATA: page in the worked-example DATA page, read the re-signed auth pointer back at its slot
//   IPA, and authenticate it in-guest (`autda`). Recovering the original target with NO FEAT_FPAC
//   fault (EC=0x1C) proves the pager re-signed it with the guest's fixed keys under the correct
//   modifier — a wrong modifier/key makes `autda` fault, which `run_sign_stub` turns into a panic.
//
//   TEXT: page in a cache __TEXT page and assert it lands RO+executable (ATTR_CODE) at stage-1, so
//   a guest instruction fetch there is a pure translation (no permission fault).
//
// Run under a bounded process-group timeout — a W^X / stub mistake can hang hv_vcpu_run:
//   perl -e '$p=fork;if(!$p){setpgrp;exec@ARGV or exit 127}$SIG{ALRM}=sub{kill"-KILL",$p;exit 124};alarm 90;wait;exit($?>>8)' \
//     cargo test -p retrace-box --test cache_pager -- --test-threads=1
use retrace_box::Box_;
use retrace_guest::{parse_macho, HELLO};

// ptrauth ABI blend (mirrors `cache::blend`): low 48 bits of the address, diversity in bits [63:48].
fn blend(addr: u64, diversity: u16) -> u64 {
    (addr & 0x0000_FFFF_FFFF_FFFF) | ((diversity as u64) << 48)
}

// The DATA slot is derived from the host's cache on every run (M57). History: on macOS 26.5.2 / 25F84
// (cache UUID 157E6D2E-2E5C-39B1-8F2A-8866EE228BED) the pinned worked example was
// dyld_shared_cache_arm64e.02.dylddata, DATA map[4], page 1, page_starts[1]=0x22e0:
//   raw=0x801dab846c6f1a88 -> auth, key=DA, addrDiv=1, diversity=0x6ae1, runtimeOffset=0x6c6f1a88,
// slot IPA 0x1ec46e2e0. On 27.2 that slot is no longer an auth pointer and the test FPAC-faulted.
// `first_da_addr_div_slot` parses the cache's own header + v5 slide info by hand, independently of
// `cache.rs`, which keeps the assertion a real oracle rather than a restatement of the code under test.
const CACHE: &str = retrace_box::DEFAULT_CACHE_PATH;

fn rd<const N: usize>(f: &[u8], off: usize) -> [u8; N] {
    f[off..off + N].try_into().unwrap()
}
fn u16at(f: &[u8], o: usize) -> u64 { u16::from_le_bytes(rd(f, o)) as u64 }
fn u32at(f: &[u8], o: usize) -> u64 { u32::from_le_bytes(rd(f, o)) as u64 }
fn u64at(f: &[u8], o: usize) -> u64 { u64::from_le_bytes(rd(f, o)) }

// (slot IPA, unsigned target, diversity) of the first DA, address-diversified auth slot at slide 0.
fn first_da_addr_div_slot() -> (u64, u64, u16) {
    let main = std::fs::read(CACHE).unwrap();
    let mut files = vec![main.clone()];
    let (sc_off, sc_n) = (u32at(&main, 0x188) as usize, u32at(&main, 0x18C) as usize);
    for i in 0..sc_n {
        let e = sc_off + i * 56 + 24;
        let suffix: Vec<u8> = main[e..e + 32].iter().copied().take_while(|&c| c != 0).collect();
        files.push(std::fs::read(format!("{CACHE}{}", String::from_utf8(suffix).unwrap())).unwrap());
    }
    for f in &files {
        let (m_off, m_n) = (u32at(f, 0x138) as usize, u32at(f, 0x13C) as usize);
        for k in 0..m_n {
            let m = m_off + k * 56;
            let (address, file_off) = (u64at(f, m), u64at(f, m + 16) as usize);
            let (si_off, si_size) = (u64at(f, m + 24) as usize, u64at(f, m + 32));
            if si_size == 0 {
                continue;
            }
            assert_eq!(u32at(f, si_off), 5, "slide info is v5");
            let page_size = u32at(f, si_off + 4);
            let n_pages = u32at(f, si_off + 8) as usize;
            let value_add = u64at(f, si_off + 0x10);
            for i in 0..n_pages {
                let start = u16at(f, si_off + 0x18 + 2 * i);
                if start == 0xFFFF {
                    continue;
                }
                let mut off = start;
                loop {
                    let raw = u64at(f, file_off + (i as u64 * page_size + off) as usize);
                    if raw >> 63 == 1 && (raw >> 51) & 1 == 1 && (raw >> 50) & 1 == 1 {
                        return (
                            address + i as u64 * page_size + off,
                            value_add + (raw & 0x3_ffff_ffff),
                            ((raw >> 34) & 0xffff) as u16,
                        );
                    }
                    let next = (raw >> 52) & 0x7ff;
                    if next == 0 {
                        break;
                    }
                    off += next * 8;
                }
            }
        }
    }
    panic!("no DA address-diversified auth slot in the host's cache");
}
// A known cache __TEXT VA in the .01 exec subcache (see cache.rs's routing test).
const TEXT_IPA: u64 = 0x1_80cc_b568;

// Load a static guest to get a live vCPU with the MMU on and the fixed PAC keys set (exactly the
// state the pager signs from), without needing to run it first. HELLO is plain arm64, so `load`'s
// derived posture leaves PAC off; the pager's re-signing is exactly what this test exercises, so
// it must ask for PAC explicitly via `load_with_pac(.., true)`. This box is never recorded/
// replayed, so the posture override cannot create a record/replay mismatch.
fn fresh_box() -> Box_ {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    Box_::load_with_pac(&loaded, true)
}

#[test]
fn page_in_cache_data_resigns_auth_pointer_that_authenticates() {
    let (data_slot_ipa, data_target, data_diversity) = first_da_addr_div_slot();
    let mut b = fresh_box();
    b.install_cache_pager();

    assert!(b.page_in_cache(data_slot_ipa), "the derived DATA cache page must page in");

    // The re-signed auth pointer now lives at the slot's IPA (regenerated from the file + guest
    // keys), not its plain unsigned target.
    let signed = b.read_u64(data_slot_ipa);
    assert_ne!(signed, data_target, "PAC not engaged: slot still equals its unsigned target");

    // Authenticate in-guest with the slot's own modifier: `autda` recovers the target iff the
    // signature is valid under the guest keys (a wrong modifier/key FEAT_FPAC-faults, panicking).
    let modifier = blend(data_slot_ipa, data_diversity);
    let recovered = b.authenticate(&[(signed, modifier, true /* DA */)]);
    assert_eq!(recovered[0], data_target, "autda did not recover target — re-sign modifier/keys wrong");
}

#[test]
fn page_in_cache_text_maps_executable() {
    let mut b = fresh_box();
    b.install_cache_pager();

    // Before: the cache window is a default RW/non-exec data block, and unmapped.
    assert!(!b.ipa_is_exec(TEXT_IPA), "cache TEXT page must not be executable before paging in");

    assert!(b.page_in_cache(TEXT_IPA), "cache TEXT page must page in");

    // After: staged RO+exec (ATTR_CODE) at stage-1 — an instruction fetch is a pure translation.
    assert!(b.ipa_is_exec(TEXT_IPA), "cache TEXT page must fault in as executable");
    // And the page is mapped/readable (pristine file bytes, no fixups on TEXT).
    let _ = b.read_u64(TEXT_IPA & !0x3fff);
}
