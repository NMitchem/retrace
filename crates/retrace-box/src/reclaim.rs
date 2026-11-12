//! M59: the kernel's half of xzone malloc's deferred-reclamation ring (xnu `vm_reclaim`).
//!
//! libmalloc asks for the ring at startup (`mach_vm_deferred_reclamation_buffer_allocate`, MIG 4822)
//! and from then on queues freed large blocks in it instead of unmapping them: a FREE entry for a
//! chunk or span whose pages may be dropped, a DEALLOCATE entry for a whole cached segment. The
//! kernel consumes entries from `head` toward `tail` when it decides to reclaim, and libmalloc
//! cancels an entry it wants back before the kernel reaches it. Before M59 the box answered 4822
//! `KERN_NOT_SUPPORTED`, which libmalloc survives but reports, on a terminal as a line on stderr.
//!
//! The kernel here never reclaims on its own. xnu's own reclamation is conditional on memory pressure,
//! suspension, a memory limit or its sampler, and its user protocol never requires one to have
//! happened (brief §5, xnu-12377 `osfmk/vm/vm_reclaim.c`), so a kernel that waits to be asked is a
//! legal kernel. It reclaims only at the two calls that require it: a flush (4823) consumes up to
//! `n` entries and a resize (4826) consumes all of them, because libmalloc's full-ring loop resizes,
//! then flushes, then retries, and never ends if `head` stays put. Consuming a DEALLOCATE entry
//! unmaps its range; consuming a FREE entry does nothing, which `VM_BEHAVIOR_REUSABLE` (all the kernel
//! does for it) allows. Trap -63's accounting update answers 0 bytes reclaimed.
//!
//! Every answer is a function of the call's arguments and guest memory at the call, and record and
//! replay call these same methods (symmetry rule 1), so nothing here is recorded beyond the reply.
//! `next_deadline` is `NO_DEADLINE` everywhere: libsyscall asks for an accounting update only when
//! `mach_absolute_time()` reaches it, so the guest clock never enters the model.
//!
//! The ring's layout (`mach/vm_reclaim_private.h`, xnu-12377, matched against 27.2's libsystem_kernel
//! by disassembly): `head` @0x58, `busy` @0x60, `tail` @0x68 (u64 ids that only grow), entries from
//! 0x80, 16 bytes each (`address` u64, `size` u32, `behavior` u8), slot `id % len`.

use super::{Box_, Region, GRANULE};
use retrace_arch::{self as arch, WallCategory};

/// The kernel's record of a task's ring: its address, the size of its (max-length) mapping, and the
/// kernel's own copy of its length, which a resize sets. The length in guest memory (ring + 0x10) is
/// libsyscall's, and is never trusted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReclaimRing { pub addr: u64, pub size: u64, pub len: u32 }

/// `next_deadline` in every reply and from trap -63: a sampling period that never ends.
pub const NO_DEADLINE: u64 = u64::MAX;

const HEAD: u64 = 0x58;
const BUSY: u64 = 0x60;
const ENTRIES: u64 = 0x80;
const ENTRY: u64 = 16;
/// `VM_RECLAIM_MAX_BUFFER_SIZE`.
const MAX_BUFFER: u64 = 128 << 20;
/// `mach_vm_reclaim_action_t`.
const FREE: u8 = 1;
const DEALLOCATE: u8 = 2;

/// `vmdr_round_len_to_size`: the header and `len` entries, rounded up to the page.
fn ring_size(len: u32) -> u64 {
    let g = GRANULE as u64;
    (ENTRIES + ENTRY * u64::from(len) + g - 1) & !(g - 1)
}

fn wall(what: String) -> String { arch::wall(WallCategory::Memory, what) }

fn u64_at(b: &[u8], at: usize) -> u64 { u64::from_le_bytes(b[at..at + 8].try_into().unwrap()) }

impl Box_ {
    /// The task's ring, if it has one (tests and diagnostics).
    pub fn reclaim_ring(&self) -> Option<ReclaimRing> { self.reclaim }

    /// 4822: map a ring of `max_len` entries and answer its address, or the `kern_return_t` xnu answers
    /// (lengths 0 or out of order 4, a mapping over 128 MiB 3, a second ring `VM_RECLAIM_RESOURCE_SHORTAGE`).
    /// The mapping is a reservation, committed a page at a time as the guest touches it, as xnu's is
    /// zero-fill and lazily backed: a 4,195,320-entry ring is 64 MiB of address space and one page of
    /// memory. xnu also checks that the fresh ring's indices are zero, which a fresh reservation's are.
    pub fn guest_reclaim_allocate(&mut self, len: u32, max_len: u32) -> Result<u64, u64> {
        if len == 0 || max_len == 0 || max_len < len { return Err(arch::KERN_INVALID_ARGUMENT); }
        let size = ring_size(max_len);
        if size > MAX_BUFFER { return Err(arch::KERN_NO_SPACE); }
        if self.reclaim.is_some() { return Err(arch::VM_RECLAIM_RESOURCE_SHORTAGE); }
        let addr = self.guest_vm_reserve_masked(0, size, 0, arch::VM_FLAGS_ANYWHERE)?;
        self.reclaim = Some(ReclaimRing { addr, size, len });
        Ok(addr)
    }

    /// 4823: consume up to `n` entries. `Ok(Ok(bytes))` is the reply's bytes reclaimed, `Ok(Err(kr))` an
    /// error reply, `Err` a wall (a ring xnu would kill the process over).
    pub fn guest_reclaim_flush(&mut self, n: u32) -> Result<Result<u64, u64>, String> {
        if self.reclaim.is_none() { return Ok(Err(arch::KERN_INVALID_ARGUMENT)); }
        self.reclaim_drain(Some(n))
    }

    /// 4826: consume every entry, then take `new_len` as the ring's length. The ring stays where it is
    /// and nothing in it is moved or cleared, as in xnu; a length whose ring would outgrow the mapping
    /// is `KERN_NO_SPACE`.
    pub fn guest_reclaim_resize(&mut self, new_len: u32) -> Result<Result<u64, u64>, String> {
        if new_len == 0 { return Ok(Err(arch::KERN_INVALID_ARGUMENT)); }
        let Some(ring) = self.reclaim else { return Ok(Err(arch::KERN_INVALID_TASK)) };
        if ring_size(new_len) > ring.size { return Ok(Err(arch::KERN_NO_SPACE)); }
        let r = self.reclaim_drain(None)?;
        if r.is_ok() { self.reclaim = Some(ReclaimRing { len: new_len, ..ring }); }
        Ok(r)
    }

    /// Trap -63, `mach_vm_reclaim_update_kernel_accounting_trap(task, *bytes, *deadline)`: nothing is
    /// trimmed, so it answers 0 bytes and `NO_DEADLINE`. Returns the `kern_return_t` and the two
    /// writes, which xnu copies out only on success: a NULL pointer is 4, no ring `KERN_NOT_FOUND`, and a
    /// pointer the program couldn't write natively `KERN_MEMORY_ERROR`. The task port isn't checked,
    /// as `mach_vm_allocate`'s trap arm doesn't check its own.
    pub fn guest_reclaim_accounting(&mut self, bytes_ptr: u64, deadline_ptr: u64) -> (u64, Vec<Region>) {
        if bytes_ptr == 0 || deadline_ptr == 0 { return (arch::KERN_INVALID_ARGUMENT, vec![]); }
        if self.reclaim.is_none() { return (arch::KERN_NOT_FOUND, vec![]); }
        if !self.prepare_reply(bytes_ptr, 8) || !self.prepare_reply(deadline_ptr, 8) {
            return (arch::KERN_MEMORY_ERROR, vec![]);
        }
        (0, vec![Region { ipa: bytes_ptr, bytes: 0u64.to_le_bytes().to_vec() },
                 Region { ipa: deadline_ptr, bytes: NO_DEADLINE.to_le_bytes().to_vec() }])
    }

    /// Consume entries from `head`: at most `cap`, or all of them. Cancelled entries (address or size
    /// 0) are consumed and skipped. Ends with `head` and `busy` both past the last one consumed.
    fn reclaim_drain(&mut self, cap: Option<u32>) -> Result<Result<u64, u64>, String> {
        let ring = self.reclaim.expect("reclaim_drain is called only with a ring");
        let at = ring.addr + HEAD;
        let ix = self.read_va_checked(at, 24)
            .ok_or_else(|| wall(format!("the reclaim ring's indices at {at:#x} can't be read")))?;
        let (head, busy, tail) = (u64_at(&ix, 0), u64_at(&ix, 8), u64_at(&ix, 16));
        // One vCPU and no kernel thread: only this code moves `busy`, and it leaves it at `head`.
        if busy != head {
            return Err(wall(format!("the reclaim ring's busy index {busy} isn't its head {head}")));
        }
        if tail < head { return Ok(Err(arch::KERN_ABORTED)); }
        if tail - head > u64::from(ring.len) {
            return Err(wall(format!("the reclaim ring holds {} entries, more than its length {}", tail - head, ring.len)));
        }
        let k = cap.map_or(tail - head, |n| u64::from(n).min(tail - head));
        let g = GRANULE as u64;
        let mut bytes = 0u64;
        for id in head..head + k {
            let slot = ring.addr + ENTRIES + (id % u64::from(ring.len)) * ENTRY;
            let e = self.read_va_checked(slot, ENTRY as usize)
                .ok_or_else(|| wall(format!("the reclaim ring's entry at {slot:#x} can't be read")))?;
            let (addr, size, behavior) = (u64_at(&e, 0), u32::from_le_bytes(e[8..12].try_into().unwrap()), e[12]);
            if addr == 0 || size == 0 { continue; }
            match behavior {
                FREE => {}
                DEALLOCATE => {
                    let start = addr & !(g - 1);
                    let end = addr.checked_add(u64::from(size) + g - 1)
                        .ok_or_else(|| wall(format!("the reclaim ring's entry {addr:#x}+{size:#x} wraps")))? & !(g - 1);
                    if !self.range_mapped(start, end) {
                        return Err(wall(format!("the reclaim ring deallocates {start:#x}..{end:#x}, which isn't all mapped")));
                    }
                    self.guest_munmap(start, end - start);
                }
                other => return Err(wall(format!("the reclaim ring's entry at {slot:#x} has behavior {other}"))),
            }
            bytes += u64::from(size);
        }
        let new_head = (head + k).to_le_bytes();
        self.write_va_committing(ring.addr + HEAD, &new_head).map_err(wall)?;
        self.write_va_committing(ring.addr + BUSY, &new_head).map_err(wall)?;
        Ok(Ok(bytes))
    }

    /// Every page of `[lo, hi)` is backed or reserved: what xnu's deallocate of a ring entry needs, short
    /// of killing the process with `EXC_GUARD`.
    fn range_mapped(&self, lo: u64, hi: u64) -> bool {
        (lo..hi).step_by(GRANULE).all(|p| self.host_span(p).is_some()
            || self.reservations.iter().any(|&(s, l)| p >= s && p < s + l))
    }
}
