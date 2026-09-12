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
/// Task 2 left this empty (the views WERE the legacy tables); Task 3 filled it. `unexercised`
/// means the number is absent from `tests/census.rs` — no corpus guest dispatches it, so the
/// entry is header truth and not a measured fix. One entry says `exercised` instead.
pub const EXPECTED_DIFFS: &[(u64, View, &str)] = &[
    // M33 finding 1: M30 tabled these as readers from their prototypes, and every one takes a
    // descriptor in x0 that `fd_operands` never translated — the M10 class, present in the tree
    // since M30 and never hit because no corpus guest issues them. The row is header truth; the
    // legacy table was wrong.
    (154, View::FdOperands, "pwrite(fd, …) — unexercised"),
    (415, View::FdOperands, "pwrite_nocancel(fd, …) — unexercised"),
    (121, View::FdOperands, "writev(fd, …) — unexercised"),
    // The one exception to "never hit": 412 IS in the census, issued by `/bin/ed`, a binary in
    // the Apple-sweep PASS set — so this row is a live untranslated-fd fix, not header truth alone.
    (412, View::FdOperands, "writev_nocancel(fd, …) — exercised by the census (/bin/ed): a live M10-class fix"),
    (541, View::FdOperands, "pwritev(fd, …) — unexercised"),
    (413, View::FdOperands, "sendto_nocancel(s, …): the _nocancel trap a fourth time — unexercised"),
    (28,  View::FdOperands, "sendmsg(s, …) — unexercised"),
    (402, View::FdOperands, "sendmsg_nocancel(s, …) — unexercised"),
    (481, View::FdOperands, "sendmsg_x(s, …) — unexercised"),
    (337, View::FdOperands, "sendfile(fd, s, …): TWO descriptors — unexercised"),
    // M33 finding 1, moot half: the refused family. retrace-core's writes_via_nested_pointer
    // assert fires BEFORE translate_fds runs, so translation never happens; listed because the row
    // is header truth and the sweep must not be taught to lie.
    (120, View::FdOperands, "readv(fd, …) — refused upstream; moot"),
    (411, View::FdOperands, "readv_nocancel(fd, …) — refused upstream; moot"),
    (27,  View::FdOperands, "recvmsg(s, …) — refused upstream; moot"),
    (401, View::FdOperands, "recvmsg_nocancel(s, …) — refused upstream; moot"),
    (540, View::FdOperands, "preadv(fd, …) — refused upstream; moot"),
    (480, View::FdOperands, "recvmsg_x(s, …) — refused upstream; moot"),
];

/// The BSD numbers, the mach traps (negative, as the two's-complement `u64` the trap carries), and
/// the `MAC_SYSCALL_MAGIC` band retrace-core recognises.
fn domain() -> impl Iterator<Item = u64> {
    (0..=1023u64)
        .chain((1..=128i64).map(|n| (-n) as u64))
        .chain(0x8000_0000u64..=0x8000_000f)
}

fn differs(num: u64, view: View) -> bool {
    match view {
        View::FdOperands => fd_operands(num).collect::<Vec<_>>() != legacy_fd_operands(num),
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
