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

// Worked example, verified against real bytes (spikes/cacheprobe.c):
// dyld_shared_cache_arm64e.02.dylddata, DATA map[4] (addr=0x1ec468000), page 1
// (vmAddr=0x1ec46c000), page_starts[1]=0x22e0:
//   raw=0x801dab846c6f1a88 -> auth, key=DA, addrDiv=1, diversity=0x6ae1, runtimeOffset=0x6c6f1a88.
// At slide 0: slot IPA = 0x1ec46c000 + 0x22e0 = 0x1ec46e2e0; target = 0x180000000 + 0x6c6f1a88;
// modifier = blend(slotSlidVA, diversity) = blend(0x1ec46e2e0, 0x6ae1).
//
// These are CACHE-BUILD-SPECIFIC: they came from cache UUID 157E6D2E-2E5C-39B1-8F2A-8866EE228BED
// on macOS 26.5.2 / 25F84 (cache dated 2026-06-25). When the host's shared cache moves, the pinned
// slot no longer names an auth slot and this test FPAC-faults (`sign stub faulted at EL0: ESR_EL1
// EC=Other(28)`) — that is cache drift, not a re-signing regression. Re-derive with
// `spikes/cacheprobe.c` (see spikes/README.md): build it, run it, and read the `uuid=` line plus
// any `AUTH ... key=DA addrDiv=1` slot's `@slide0: slotVA=… targetVA=… modifier=blend(…)` line —
// those three numbers are exactly DATA_SLOT_IPA / DATA_TARGET / DATA_DIVERSITY. cacheprobe parses
// the cache file's own header + slide-info by hand, independently of `cache.rs`, which is what
// keeps the assertion below a real oracle rather than a restatement of the code under test.
const DATA_SLOT_IPA: u64 = 0x1_ec46_e2e0;
const DATA_TARGET: u64 = 0x1_ec6f_1a88;
const DATA_DIVERSITY: u16 = 0x6ae1;
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
    let mut b = fresh_box();
    b.install_cache_pager();

    assert!(b.page_in_cache(DATA_SLOT_IPA), "worked-example DATA cache page must page in");

    // The re-signed auth pointer now lives at the slot's IPA (regenerated from the file + guest
    // keys), not its plain unsigned target.
    let signed = b.read_u64(DATA_SLOT_IPA);
    assert_ne!(signed, DATA_TARGET, "PAC not engaged: slot still equals its unsigned target");

    // Authenticate in-guest with the slot's own modifier: `autda` recovers the target iff the
    // signature is valid under the guest keys (a wrong modifier/key FEAT_FPAC-faults, panicking).
    let modifier = blend(DATA_SLOT_IPA, DATA_DIVERSITY);
    let recovered = b.authenticate(&[(signed, modifier, true /* DA */)]);
    assert_eq!(recovered[0], DATA_TARGET, "autda did not recover target — re-sign modifier/keys wrong");
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
