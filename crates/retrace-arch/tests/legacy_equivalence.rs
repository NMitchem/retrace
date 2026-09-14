//! The five hand-written tables as they stood at `e13eb17`, copied VERBATIM as the equivalence
//! oracle for M33's unification. This is a test fixture, not production: `retrace_arch`'s views
//! derive from `arg_kinds` and these are what they must still answer, entry for entry, except
//! where `EXPECTED_DIFFS` says why not.
//!
//! The sweep checks BOTH directions: a difference with no entry fails, and an entry with no
//! difference fails. A one-directional check would let the difference list rot.
use retrace_arch::*;

// `CENSUS` lives in `tests/census.rs`, a separate integration-test binary; `#[path]` compiles that
// file into THIS binary too so `exercised_and_unexercised_match_the_census` can read it. The cost
// is that census.rs's own two `#[test]`s run twice across the two binaries — accepted, and said
// here so the duplicated names in a gate log are not mistaken for a copy of the file.
#[path = "census.rs"]
#[allow(dead_code)]
mod census;

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
/// Task 2 left this empty (the views WERE the legacy tables); Task 3 filled it; Task 5 added the
/// census rows' six. `unexercised` means the number is absent from `tests/census.rs` — no corpus
/// guest dispatches it, so the entry is header truth and not a measured fix — and
/// `exercised_and_unexercised_match_the_census` enforces that the word and the census agree,
/// entry by entry, in both directions.
pub const EXPECTED_DIFFS: &[(u64, View, &str)] = &[
    // M33 finding 1: M30 tabled these as readers from their prototypes, and every one takes a
    // descriptor in x0 that `fd_operands` never translated — the M10 class, present in the tree
    // since M30 and never hit because no corpus guest issues them. The row is header truth; the
    // legacy table was wrong.
    (154, View::FdOperands, "pwrite(fd, …) — unexercised"),
    (415, View::FdOperands, "pwrite_nocancel(fd, …) — unexercised"),
    (121, View::FdOperands, "writev(fd, …) — unexercised"),
    // The one exception to "never hit": 412 IS in the census, issued by `/bin/ed` — but the
    // descriptor it carries is fd 2, a console fd that `FdTable::new` maps to itself, so the fix
    // is live for the class and inert for the one guest that reaches it.
    (412, View::FdOperands, "writev_nocancel(fd, …) — exercised by the census (/bin/ed, on fd 2 — a console fd that translates to itself: live for the class, inert for that guest)"),
    (541, View::FdOperands, "pwritev(fd, …) — unexercised"),
    (413, View::FdOperands, "sendto_nocancel(s, …): the _nocancel trap a fourth time — unexercised"),
    (28,  View::FdOperands, "sendmsg(s, …) — unexercised"),
    (402, View::FdOperands, "sendmsg_nocancel(s, …) — unexercised"),
    (481, View::FdOperands, "sendmsg_x(s, …) — unexercised"),
    (337, View::FdOperands, "sendfile(fd, s, …): TWO descriptors — unexercised"),
    // M33 finding 1, moot half: the refused family. retrace-core's writes_via_nested_pointer
    // assert fires BEFORE translate_fds runs, so translation never happens; listed because the row
    // is header truth and the sweep must not be taught to lie.
    (120, View::FdOperands, "readv(fd, …) — refused upstream; moot — unexercised"),
    (411, View::FdOperands, "readv_nocancel(fd, …) — refused upstream; moot — unexercised"),
    (27,  View::FdOperands, "recvmsg(s, …) — refused upstream; moot — unexercised"),
    (401, View::FdOperands, "recvmsg_nocancel(s, …) — refused upstream; moot — unexercised"),
    (540, View::FdOperands, "preadv(fd, …) — refused upstream; moot — unexercised"),
    (480, View::FdOperands, "recvmsg_x(s, …) — refused upstream; moot — unexercised"),
    // M33 finding 2 (Task 5, the census rows): six load-bearing kinds the legacy tables had no
    // opinion on because they had no row at all, each classified from its kernel prototype under
    // spec §3b and each exercised by a named corpus guest. Four were pre-listed or ruled (13,
    // 362, 59, 244); two the prototypes forced (184, 550). `fchdir`'s is a live M10-class fix (an
    // untranslated descriptor); the other five change what the box does only in ways the guest
    // cannot observe today — and the row comment says why in each case.
    (13,  View::FdOperands, "fchdir(fd): a descriptor the legacy table never translated — exercised (/bin/ls)"),
    (362, View::AllocatesFd, "kqueue() returns a new descriptor — exercised (/bin/wait4path)"),
    (59,  View::ReadsGuestBuffer, "execve reads argv/envp strings through nested pointers — exercised (/bin/sh)"),
    // posix_spawn reads argv/envp strings (and adesc's members) through nested pointers — a reader
    // the M30 list never had. Exercised by the census (the CPython launcher). Its only destination
    // is the 4-byte *pid, so withholding the canary costs nothing.
    (244, View::ReadsGuestBuffer, "posix_spawn(pid, path, desc, argv, envp) — exercised (the CPython launcher)"),
    // sigreturn follows uctx->uc_mcontext64 for the mcontext (rule 1). Serviced above the trace
    // (M12) and asserted off the forward path by `is_signal_syscall`, so the view is consulted
    // for it by nothing — documentation, listed because the sweep must not be taught to lie.
    (184, View::ReadsGuestBuffer, "sigreturn(uctx, …): a nested read, serviced above the trace (M12) — exercised (altstack, sigframe, segvy, …)"),
    // map_with_linking_np reads link_info for a caller-chosen link_info_size whose only kernel
    // cap is 64 MiB (rule 4) — the first Source row that is not a write/send family member. It
    // has no destination, so the withheld canary costs nothing; measured sizes are ≤ 2920 bytes.
    (550, View::ReadsGuestBuffer, "map_with_linking_np(regions, count, link_info, link_info_size): link_info is caller-sized, capped only at 64 MiB — exercised (every dynamic guest)"),
    // M34: the two `Dest` rows the charter's destgaps entry owed (its third, getattrlist, stayed
    // Ptr on a cited kernel bound — Ruling 1 — and so does not differ). Both exercised by every
    // dynamic guest in the census; both measured inert for the window on landing (corpus maximum
    // 368 and 1032 bytes against a 64 KiB flat window) and live for the forwarded-count clamp.
    (336, View::DestBuffer, "proc_info(callnum, pid, flavor, arg, buffer, buffersize): buffer is a Dest of x5 bytes — exercised (every dynamic guest; corpus max 368)"),
    (169, View::DestBuffer, "csops(pid, ops, useraddr, usersize): useraddr is a Dest of x3 bytes (CS_OPS_BLOB is unbounded below the window) — exercised (every dynamic guest; corpus max 1032)"),
    (170, View::DestBuffer, "csops_audittoken(…): csops's shape plus a 32-byte token copyin — exercised (every dynamic guest; corpus max 1032)"),
    // M37: dup2 is modelled in the box. Its second operand is the guest's own target slot number
    // — translating it would forward a HOST descriptor as the target and overwrite retrace's own.
    (90, View::FdOperands, "dup2(fd, fd2): fd2 is the guest's target slot, not a descriptor to translate — M37; exercised (/bin/csh, /bin/tcsh: dup2(0,16) (1,17) (2,18) (16,19))"),
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

/// `unexercised` was an unenforced label until M33 t5. Now: an entry for a census number must
/// NOT say it, and an entry for a non-census number MUST — so a row that a guest starts to
/// exercise cannot keep calling itself header truth, and a reason cannot claim a measurement the
/// census does not hold.
#[test]
fn exercised_and_unexercised_match_the_census() {
    for (n, v, why) in EXPECTED_DIFFS {
        let in_census = census::CENSUS.contains(&(*n as i64));
        let says_unexercised = why.contains("unexercised");
        assert!(in_census != says_unexercised,
            "EXPECTED_DIFFS entry {} {v:?} ({why}): in census = {in_census}, says unexercised = {says_unexercised}",
            *n as i64);
    }
}

#[test]
fn expected_diffs_name_only_numbers_in_the_domain() {
    let dom: Vec<u64> = domain().collect();
    for (n, v, why) in EXPECTED_DIFFS {
        assert!(dom.contains(n), "EXPECTED_DIFFS entry {n:#x} {v:?} ({why}) is outside the sweep domain");
        assert!(!why.is_empty(), "EXPECTED_DIFFS entry {n:#x} {v:?} has no reason");
    }
}
