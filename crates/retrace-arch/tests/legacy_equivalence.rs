//! The five hand-written tables as they stood at `e13eb17`, copied VERBATIM as the equivalence
//! oracle for M33's unification. This is a test fixture, not production: `retrace_arch`'s views
//! derive from `arg_kinds` and these are what they must still answer, entry for entry, except
//! where `EXPECTED_DIFFS` says why not.
//!
//! The sweep checks BOTH directions: a difference with no entry fails, and an entry with no
//! difference fails. A one-directional check would let the difference list rot.
use retrace_arch::*;

pub fn legacy_fd_operands(num: u64) -> &'static [usize] {
    match num {
        SYS_CLOSE | SYS_CLOSE_NOCANCEL | SYS_READ | SYS_READ_NOCANCEL | SYS_PREAD
        | SYS_PREAD_NOCANCEL
        | SYS_WRITE | SYS_WRITE_NOCANCEL | SYS_FCNTL | SYS_FCNTL_NOCANCEL
        | SYS_FSTAT | SYS_FSTAT64 | SYS_LSEEK | SYS_IOCTL | SYS_DUP
        | SYS_CONNECT | SYS_SENDTO | SYS_RECVFROM | SYS_RECVFROM_NOCANCEL | SYS_FGETATTRLIST
        | SYS_OPENAT | SYS_FSTATAT64
        | SYS_FSTATFS64 | SYS_GETDIRENTRIES64 => &[0],
        SYS_DUP2 => &[0, 1],
        SYS_MMAP => &[4],
        _ => &[],
    }
}

pub fn legacy_allocates_fd(num: u64) -> bool {
    matches!(num, SYS_OPEN | SYS_OPEN_NOCANCEL | SYS_OPENAT | SYS_DUP | SYS_SOCKET | SYS_SHM_OPEN)
}

pub fn legacy_dest_buffer(num: u64) -> Option<(usize, DestLen)> {
    match num {
        SYS_READ | SYS_READ_NOCANCEL | SYS_PREAD | SYS_PREAD_NOCANCEL => Some((1, DestLen::Reg(2))),
        SYS_SYSCTL => Some((2, DestLen::DerefU64(3))),
        SYS_GETDIRENTRIES64 => Some((1, DestLen::Reg(2))),
        SYS_GETFSSTAT64 => Some((0, DestLen::Reg(1))),
        SYS_RECVFROM | SYS_RECVFROM_NOCANCEL => Some((1, DestLen::Reg(2))),
        SYS_SYSCTLBYNAME => Some((2, DestLen::DerefU64(3))),
        _ => None,
    }
}

pub fn legacy_writes_via_nested_pointer(num: u64) -> bool {
    matches!(num, 120 | 411 | 27 | 401 | 540 | 480)
}

pub fn legacy_reads_guest_buffer(num: u64) -> bool {
    matches!(num,
        SYS_WRITE | SYS_WRITE_NOCANCEL | 154 | 415
        | 121 | 412 | 541
        | SYS_SENDTO | 413 | 28 | 402 | 481
        | 337
        | 65 | 405
        | 0xffff_ffff_ffff_ffd1 // -47
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View { FdOperands, AllocatesFd, DestBuffer, NestedPointer, ReadsGuestBuffer }
const ALL_VIEWS: [View; 5] =
    [View::FdOperands, View::AllocatesFd, View::DestBuffer, View::NestedPointer, View::ReadsGuestBuffer];

/// Every `(num, view)` where the derived view is KNOWN to disagree with its legacy table, and why.
/// Task 2 leaves this empty (the views ARE the legacy tables); Task 3 fills it.
pub const EXPECTED_DIFFS: &[(u64, View, &str)] = &[];

/// The BSD numbers, the mach traps (negative, as the two's-complement `u64` the trap carries), and
/// the `MAC_SYSCALL_MAGIC` band retrace-core recognises.
fn domain() -> impl Iterator<Item = u64> {
    (0..=1023u64)
        .chain((1..=128i64).map(|n| (-n) as u64))
        .chain(0x8000_0000u64..=0x8000_000f)
}

fn differs(num: u64, view: View) -> bool {
    match view {
        View::FdOperands => fd_operands(num) != legacy_fd_operands(num),
        View::AllocatesFd => allocates_fd(num) != legacy_allocates_fd(num),
        View::DestBuffer => dest_buffer(num) != legacy_dest_buffer(num),
        View::NestedPointer => writes_via_nested_pointer(num) != legacy_writes_via_nested_pointer(num),
        View::ReadsGuestBuffer => reads_guest_buffer(num) != legacy_reads_guest_buffer(num),
    }
}

#[test]
fn every_view_reproduces_its_legacy_table() {
    let mut unlisted = Vec::new();
    let mut stale = Vec::new();
    for num in domain() {
        for view in ALL_VIEWS {
            let d = differs(num, view);
            let listed = EXPECTED_DIFFS.iter().any(|(n, v, _)| *n == num && *v == view);
            if d && !listed { unlisted.push((num as i64, view)); }
            if !d && listed { stale.push((num as i64, view)); }
        }
    }
    assert!(unlisted.is_empty(),
        "views disagree with the legacy tables and no EXPECTED_DIFFS entry says why: {unlisted:?}");
    assert!(stale.is_empty(),
        "EXPECTED_DIFFS entries that no longer differ (stale — delete or explain): {stale:?}");
}

#[test]
fn expected_diffs_name_only_numbers_in_the_domain() {
    let dom: Vec<u64> = domain().collect();
    for (n, v, why) in EXPECTED_DIFFS {
        assert!(dom.contains(n), "EXPECTED_DIFFS entry {n:#x} {v:?} ({why}) is outside the sweep domain");
        assert!(!why.is_empty(), "EXPECTED_DIFFS entry {n:#x} {v:?} has no reason");
    }
}
