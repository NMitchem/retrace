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
//! This is Task 1 of the M2-cache sub-milestone: a pure, unit-tested decoder with no caller
//! yet. The lazy per-page pager (a later M2c task) will walk `page_starts[]` chains and call
//! `decode5`/`target_va`/`modifier` per slot. Allow dead_code until that lands.
#![allow(dead_code)]

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
}
