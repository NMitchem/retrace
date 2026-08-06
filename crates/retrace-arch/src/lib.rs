#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ec { Svc, Hvc, SysReg, SoftStep, Breakpoint, Watchpoint, DataAbort, InstrAbort, Other(u8) }

pub const SYS_WRITE: u64 = 4;
/// `SYS_write_nocancel` (`sys/syscall.h:437`). Identical `(fd, buf, nbyte)` ABI to `write`; the
/// `_nocancel` variants only skip the pthread cancellation point. libc's **stdio** flush takes this
/// path, so any guest that uses `printf`/`fwrite` — `jq` among them — reaches the console through
/// 397 and never through 4. See `is_console_write`.
pub const SYS_WRITE_NOCANCEL: u64 = 397;
pub const SYS_EXIT: u64 = 1;
pub const SVC_IMM: u64 = 0x80;

/// Is this syscall the guest writing to the console (fd 1/2)?
///
/// Console writes are mirrored into the trace and faked — never forwarded — so the guest's output
/// belongs to the recording rather than to retrace's own stdout, and replay can reproduce it
/// without executing anything. Record and replay MUST agree on what counts (symmetry rule 1), so
/// they share this one predicate instead of each spelling out the condition: an arm that forgets a
/// variant does not diverge loudly, it silently forwards the write to the HOST — which still prints,
/// so a recording looks correct on a terminal while the trace holds no console bytes at all and
/// replay prints nothing. That is exactly how 397 stayed invisible until `jq` (M9).
pub fn is_console_write(num: u64, fd: u64) -> bool {
    (num == SYS_WRITE || num == SYS_WRITE_NOCANCEL) && (fd == 1 || fd == 2)
}

/// `SYS_close_nocancel` (`sys/syscall.h:439`).
pub const SYS_CLOSE_NOCANCEL: u64 = 399;

/// Is this the guest closing one of the three standard fds?
///
/// The guest's fd 0/1/2 ARE retrace's own — retrace never virtualized them, it just mirrors writes
/// to them. So forwarding this close hands the guest a live handle on RETRACE's descriptors and it
/// closes them for real: measured with `jq`, which closes fd 1 as it exits, after which every byte
/// retrace itself tried to print — including the mirrored recording — went nowhere, silently and
/// with a 0 exit status. Faked instead (see the record arm). fd > 2 is an ordinary file and still
/// forwards.
pub fn is_console_close(num: u64, fd: u64) -> bool {
    (num == SYS_CLOSE || num == SYS_CLOSE_NOCANCEL) && fd <= 2
}

pub const SYS_READ: u64 = 3;
pub const SYS_PREAD: u64 = 153;
pub const SYS_OPEN: u64 = 5;
pub const SYS_CLOSE: u64 = 6;
pub const SYS_MUNMAP: u64 = 73;
pub const SYS_MPROTECT: u64 = 74;
pub const SYS_FSTAT: u64 = 189;
pub const SYS_MMAP: u64 = 197;
pub const SYS_LSEEK: u64 = 199;
pub const SYS_SHARED_REGION_CHECK_NP: u64 = 294;
pub const SYS_SHARED_REGION_MAP_AND_SLIDE_2_NP: u64 = 536;

// M10: the rest of the fd surface, measured from a real `jq '.name' file.json` run (352 traps) and
// resolved against the MacOSX SDK's `sys/syscall.h`. See `fd_operands`.
pub const SYS_DUP: u64 = 41;
pub const SYS_IOCTL: u64 = 54;
pub const SYS_DUP2: u64 = 90;
pub const SYS_FCNTL: u64 = 92;
pub const SYS_SOCKET: u64 = 97;
pub const SYS_CONNECT: u64 = 98;
pub const SYS_SENDTO: u64 = 133;
pub const SYS_FGETATTRLIST: u64 = 228;
pub const SYS_SHM_OPEN: u64 = 266;
pub const SYS_FSTAT64: u64 = 339;
pub const SYS_READ_NOCANCEL: u64 = 396;
pub const SYS_OPEN_NOCANCEL: u64 = 398;
pub const SYS_FCNTL_NOCANCEL: u64 = 406;
pub const SYS_OPENAT: u64 = 463;
pub const SYS_FSTATAT64: u64 = 470;
/// `map_with_linking_np` — dyld's overmap-with-linking call. **Its fd is not in a register**: x0 is a
/// guest pointer to `struct mwl_region[]` (x1 = count) and the descriptor is the struct's first
/// field. `fd_operands` cannot express that; see `MWL_REGION_STRIDE` and the box's translation.
pub const SYS_MAP_WITH_LINKING_NP: u64 = 550;

/// `sizeof(struct mwl_region)` (`mach/dyld_pager.h`): `int mwlr_fd` + `vm_prot_t` + `uint64_t` +
/// `mach_vm_address_t` + `mach_vm_size_t` = 4+4+8+8+8. `mwlr_fd` is at offset 0.
pub const MWL_REGION_STRIDE: usize = 32;
/// `MWL_MAX_REGION_COUNT` (`mach/dyld_pager.h`) — data, const, data auth, auth const, objc const.
/// A bound on how much guest memory translation will ever copy for one call.
pub const MWL_MAX_REGION_COUNT: u64 = 5;

/// `AT_FDCWD` — `openat`/`fstatat64`'s "relative to cwd" sentinel. Negative, and NOT a descriptor:
/// translation must pass it through untouched rather than rejecting it as `EBADF`.
pub const AT_FDCWD: i64 = -2;

/// Which operand indices of `num` hold a **guest** file descriptor.
///
/// The M10 analogue of `is_console_write`: one shared table rather than a condition spelled out at
/// each call site, because a forgotten entry does not diverge loudly — it forwards a raw guest fd to
/// the host kernel, which then acts on RETRACE's descriptor of that number. A syscall absent here is
/// simply not translated, so absence must mean "provably takes no fd", never "not gotten to yet".
///
/// **`_nocancel` variants are listed beside their plain forms deliberately.** macOS libc routinely
/// takes ONLY the `_nocancel` path — measured in one `jq` run: `read`(3) is called zero times and
/// `read_nocancel`(396) twice; `fcntl_nocancel`(406) appears alongside `fcntl`(92). A plain-only
/// table fails *silently*, which is exactly how M9's console bug survived until `jq`.
pub fn fd_operands(num: u64) -> &'static [usize] {
    match num {
        SYS_CLOSE | SYS_CLOSE_NOCANCEL | SYS_READ | SYS_READ_NOCANCEL | SYS_PREAD
        | SYS_WRITE | SYS_WRITE_NOCANCEL | SYS_FCNTL | SYS_FCNTL_NOCANCEL
        | SYS_FSTAT | SYS_FSTAT64 | SYS_LSEEK | SYS_IOCTL | SYS_DUP
        | SYS_CONNECT | SYS_SENDTO | SYS_FGETATTRLIST
        // openat/fstatat64 take a *dirfd*; AT_FDCWD passes through translation untouched.
        | SYS_OPENAT | SYS_FSTATAT64 => &[0],
        SYS_DUP2 => &[0, 1],
        // The exception that makes a single choke point insufficient: mmap's fd is consumed by
        // guest_mmap_file, which never reaches forward_and_diff.
        SYS_MMAP => &[4],
        _ => &[],
    }
}

/// Does `num`'s RETURN value need binding to a fresh guest fd slot?
///
/// `socket` and `shm_open` are here for the same reason `open` is: guest fds are not files-only.
///
/// **`dup2` is deliberately absent.** It names its own target descriptor instead of taking the
/// lowest free one, so binding its return like the others would put the new mapping in the wrong
/// slot. No guest in the gate calls it (measured: zero in the `jq` run), so retrace-core asserts on
/// it rather than modelling it wrong — a silently mis-modelled `dup2` aliases the wrong file.
pub fn allocates_fd(num: u64) -> bool {
    matches!(num, SYS_OPEN | SYS_OPEN_NOCANCEL | SYS_OPENAT | SYS_DUP | SYS_SOCKET | SYS_SHM_OPEN)
}

pub const SYS_SYSCTL: u64 = 202;
pub const SYS_GETRLIMIT: u64 = 194;
/// sysctl top-level: `CTL_KERN` (`sys/sysctl.h`).
pub const CTL_KERN: u32 = 1;
/// `sys/sysctl.h:276` — "LP64 user stack query". Forwarding this hands the guest the HOST
/// process's ASLR'd stack address; retrace must answer it from the guest's own geometry.
pub const KERN_USRSTACK64: u32 = 59;

/// `sys/resource.h:446`.
pub const RLIMIT_STACK: u64 = 3;
/// `sys/resource.h:458` — libc ORs this in for strict-POSIX `getrlimit`; the guest is observed
/// passing `0x1003`, so the resource must be masked before comparison.
pub const RLIMIT_POSIX_FLAG: u64 = 0x1000;

/// BSD errno: an argument the kernel rejects (a MAP_FIXED address outside the address space).
pub const EINVAL: u64 = 22;

pub const LC_LOAD_DYLINKER: u32 = 0xe;
pub const FAT_MAGIC: u32 = 0xcafe_babe;      // big-endian on disk; read with from_be
pub const FAT_MAGIC_64: u32 = 0xcafe_babf;
pub const CPU_TYPE_ARM64: u32 = 0x0100_000c;
pub const CPU_SUBTYPE_ARM64E: u32 = 2;
pub const PSTATE_C: u64 = 1 << 29;           // carry bit in NZCV (SPSR_EL1 / CPSR)

pub fn ec_of(esr_el2: u64) -> Ec {
    match ((esr_el2 >> 26) & 0x3f) as u8 {
        0x15 => Ec::Svc,
        0x16 => Ec::Hvc,
        0x18 => Ec::SysReg,
        0x32 | 0x33 => Ec::SoftStep,
        0x30 | 0x31 => Ec::Breakpoint,
        0x34 | 0x35 => Ec::Watchpoint,
        0x24 | 0x25 => Ec::DataAbort,
        0x20 | 0x21 => Ec::InstrAbort,
        other => Ec::Other(other),
    }
}

/// If `insn` is an AArch64 pointer-authentication AUT* instruction whose authenticated result
/// lands in a destination register — the `AUTIA/AUTIB/AUTDA/AUTDB` register-modifier variants and
/// their `AUTIZA/AUTIZB/AUTDZA/AUTDZB` zero-modifier forms — return that register number (Rd).
/// Returns None otherwise. Used to emulate a B-family auth that FEAT_FPAC-faulted by stripping Rd
/// to canonical (see `Box_::try_emulate_fpac_auth`). Combined auth-and-{branch,load} forms
/// (`braab`/`ldrab`/…) have no Rd to fix and are intentionally NOT matched (they fail loud).
pub fn decode_aut_rd(insn: u32) -> Option<u32> {
    // "Data-processing (1 source)" PAC encodings: [31:10] fixed per op, [9:5] Rn, [4:0] Rd.
    match insn & 0xFFFF_FC00 {
        0xDAC1_1000 | 0xDAC1_1400 | 0xDAC1_1800 | 0xDAC1_1C00   // AUTIA/AUTIB/AUTDA/AUTDB Xd,Xn
        | 0xDAC1_3000 | 0xDAC1_3400 | 0xDAC1_3800 | 0xDAC1_3C00 // AUTIZA/AUTIZB/AUTDZA/AUTDZB Xd
            => Some(insn & 0x1F),
        _ => None,
    }
}

// ---- M11-signals ---------------------------------------------------------------------------
// Numbers resolved from $(xcrun --show-sdk-path)/usr/include/sys/syscall.h, never from memory.
// The `_nocancel` pairing rule (M10) was checked and yields nothing here: the only `_nocancel`
// signal syscalls are sigsuspend_nocancel(410) and __sigwait_nocancel(422), and both pair with
// calls M11 asserts on anyway — so no SERVICED call has a silent-fallthrough twin.
//
// Measured surface (Task 1 Step 0, RETRACE_TRACE=1 full histograms over hello_dyn/hello_rust/jq):
// of all twelve numbers below, ONLY sigaction(46) is exercised, 3x and by hello_rust alone. That
// zero-count is the evidence each assert in record's dispatch rests on.
pub const SYS_GETPID: u64 = 20;
pub const SYS_KILL: u64 = 37;
pub const SYS_SIGACTION: u64 = 46;
pub const SYS_SIGPROCMASK: u64 = 48;
pub const SYS_SIGPENDING: u64 = 52;
pub const SYS_SIGALTSTACK: u64 = 53;
pub const SYS_SIGSUSPEND: u64 = 111;
pub const SYS_SIGRETURN: u64 = 184;
pub const SYS_PTHREAD_KILL: u64 = 328;
pub const SYS_PTHREAD_SIGMASK: u64 = 329;
pub const SYS_SIGWAIT: u64 = 330;
pub const SYS_TERMINATE_WITH_PAYLOAD: u64 = 520;
pub const SYS_ABORT_WITH_PAYLOAD: u64 = 521;

/// `NSIG` from `sys/signal.h:76` — "counting 0; could be 33 (mask is 1-32)". Signal numbers run
/// 1..=31 in the table; index 0 is unused so indexing mirrors signal numbering.
pub const NSIG: usize = 32;
pub const SIGABRT: u64 = 6;
pub const SIG_DFL: u64 = 0;
pub const SIG_IGN: u64 = 1;
pub const SIG_BLOCK: u64 = 1;
pub const SIG_UNBLOCK: u64 = 2;
pub const SIG_SETMASK: u64 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultAction {
    Terminate,
    Ignore,
}

/// The kernel's default disposition for `sig` when the guest has installed nothing.
///
/// An arch fact, not policy — which is why it lives here beside `ec_of` rather than in the box.
/// Record's raise arm and replay's mirror both consult THIS function, and that shared call is what
/// keeps them from drifting (symmetry rule 1).
pub fn default_action(sig: u64) -> DefaultAction {
    match sig {
        16 | 20 | 28 => DefaultAction::Ignore, // SIGURG, SIGCHLD, SIGWINCH
        _ => DefaultAction::Terminate,
    }
}

/// Every syscall M11 intercepts — serviced against the guest's `SigTable` or asserted, but in no
/// case forwarded. This is the single place the correctness invariant ("no signal syscall is ever
/// issued in retrace's process") is expressed, so the record loop can assert it rather than restate
/// it. `getpid`(20) is deliberately absent: it keeps forwarding, and the raise arm's self-pid check
/// depends on that.
pub fn is_signal_syscall(num: u64) -> bool {
    matches!(
        num,
        SYS_KILL
            | SYS_SIGACTION
            | SYS_SIGPROCMASK
            | SYS_SIGPENDING
            | SYS_SIGALTSTACK
            | SYS_SIGSUSPEND
            | SYS_SIGRETURN
            | SYS_PTHREAD_KILL
            | SYS_PTHREAD_SIGMASK
            | SYS_SIGWAIT
            | SYS_TERMINATE_WITH_PAYLOAD
            | SYS_ABORT_WITH_PAYLOAD
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decode_hvc_and_svc() {
        // From the spike: ESR_EL2 = 0x5a000000 => EC = 0x16 (HVC).
        assert_eq!(ec_of(0x5a000000), Ec::Hvc);
        // EC 0x15 (SVC from AArch64) in bits [31:26].
        assert_eq!(ec_of(0x15 << 26), Ec::Svc);
        assert_eq!(ec_of(0x18 << 26), Ec::SysReg);
        assert_eq!(ec_of(0x32 << 26), Ec::SoftStep);
    }

    #[test]
    fn syscall_numbers() {
        assert_eq!((SYS_READ, SYS_WRITE, SYS_OPEN, SYS_CLOSE, SYS_EXIT), (3,4,5,6,1));
        assert_eq!((SYS_FSTAT, SYS_LSEEK, SYS_MMAP, SYS_MUNMAP, SYS_MPROTECT), (189,199,197,73,74));
        assert_eq!((SYS_SHARED_REGION_CHECK_NP, SYS_SHARED_REGION_MAP_AND_SLIDE_2_NP), (294, 536));
        assert_eq!((SYS_SYSCTL, SYS_GETRLIMIT), (202, 194));
        assert_eq!((CTL_KERN, KERN_USRSTACK64), (1, 59));
        assert_eq!((RLIMIT_STACK, RLIMIT_POSIX_FLAG), (3, 0x1000));
        assert_eq!((SYS_WRITE_NOCANCEL, SYS_CLOSE_NOCANCEL), (397, 399));
    }

    #[test]
    fn signal_syscall_numbers_match_the_sdk() {
        // Resolved from $(xcrun --show-sdk-path)/usr/include/sys/syscall.h on 2026-08-06.
        assert_eq!((SYS_GETPID, SYS_KILL, SYS_SIGACTION, SYS_SIGPROCMASK), (20, 37, 46, 48));
        assert_eq!((SYS_SIGPENDING, SYS_SIGALTSTACK, SYS_SIGSUSPEND, SYS_SIGRETURN), (52, 53, 111, 184));
        assert_eq!((SYS_PTHREAD_KILL, SYS_PTHREAD_SIGMASK, SYS_SIGWAIT), (328, 329, 330));
        assert_eq!((SYS_TERMINATE_WITH_PAYLOAD, SYS_ABORT_WITH_PAYLOAD), (520, 521));
        // sys/signal.h: NSIG == __DARWIN_NSIG == 32; sigset_t is __uint32_t (sys/_types.h:85).
        assert_eq!((NSIG, SIGABRT, SIG_DFL, SIG_IGN), (32, 6, 0, 1));
        assert_eq!((SIG_BLOCK, SIG_UNBLOCK, SIG_SETMASK), (1, 2, 3));
    }

    #[test]
    fn default_action_classifies_the_three_ignored_signals() {
        // SIGCHLD=20, SIGURG=16, SIGWINCH=28 default to ignore; everything else terminates.
        assert_eq!(default_action(20), DefaultAction::Ignore);
        assert_eq!(default_action(16), DefaultAction::Ignore);
        assert_eq!(default_action(28), DefaultAction::Ignore);
        assert_eq!(default_action(SIGABRT), DefaultAction::Terminate);
        assert_eq!(default_action(9), DefaultAction::Terminate);   // SIGKILL
        assert_eq!(default_action(11), DefaultAction::Terminate);  // SIGSEGV
    }

    #[test]
    fn is_signal_syscall_covers_every_intercepted_number_and_nothing_else() {
        for n in [37u64, 46, 48, 52, 53, 111, 184, 328, 329, 330, 520, 521] {
            assert!(is_signal_syscall(n), "{n} must be intercepted");
        }
        // getpid is NOT intercepted — it keeps forwarding, and the raise arm's self-check relies on
        // that: measured, the guest's getpid returns RETRACE's own pid (Task 1 Step 0, answer 2).
        for n in [20u64, 1, 3, 4, 5, 6, 197, 333] {
            assert!(!is_signal_syscall(n), "{n} must keep forwarding");
        }
    }

    #[test]
    fn fd_operands_covers_the_measured_surface() {
        for num in [SYS_CLOSE, SYS_CLOSE_NOCANCEL, SYS_READ, SYS_READ_NOCANCEL, SYS_PREAD,
                    SYS_WRITE, SYS_WRITE_NOCANCEL, SYS_FCNTL, SYS_FCNTL_NOCANCEL,
                    SYS_FSTAT, SYS_FSTAT64, SYS_LSEEK, SYS_IOCTL, SYS_DUP,
                    SYS_CONNECT, SYS_SENDTO, SYS_FGETATTRLIST, SYS_OPENAT, SYS_FSTATAT64] {
            assert_eq!(fd_operands(num), &[0], "syscall {num} holds its fd in x0");
        }
        assert_eq!(fd_operands(SYS_MMAP), &[4], "mmap's fd is x4, consumed by guest_mmap_file");
        assert_eq!(fd_operands(SYS_DUP2), &[0, 1]);
        // Path-only, fd-free, and fd-RETURNING calls must not have an operand translated.
        for num in [SYS_OPEN, SYS_OPEN_NOCANCEL, SYS_SOCKET, SYS_SHM_OPEN,
                    SYS_EXIT, SYS_MUNMAP, SYS_SYSCTL] {
            assert_eq!(fd_operands(num), &[] as &[usize], "syscall {num} has no fd operand");
        }
        // map_with_linking_np carries its fd INSIDE a guest struct, so it is deliberately absent
        // here — an arg index cannot name it. The box translates it separately.
        assert_eq!(fd_operands(SYS_MAP_WITH_LINKING_NP), &[] as &[usize]);
    }

    #[test]
    fn allocates_fd_covers_every_fd_producing_call() {
        for num in [SYS_OPEN, SYS_OPEN_NOCANCEL, SYS_OPENAT, SYS_DUP, SYS_SOCKET, SYS_SHM_OPEN] {
            assert!(allocates_fd(num), "syscall {num} returns a NEW fd");
        }
        for num in [SYS_CLOSE, SYS_READ, SYS_PREAD, SYS_MMAP, SYS_FCNTL, SYS_EXIT, SYS_IOCTL] {
            assert!(!allocates_fd(num), "syscall {num} does not return a new fd");
        }
        // dup2 names its own target slot, so it is NOT bound like the others — retrace-core
        // asserts on it instead of modelling it wrong. See allocates_fd's doc comment.
        assert!(!allocates_fd(SYS_DUP2), "dup2 is deliberately unmodelled, not silently bound");
    }

    /// The M9 defect generalized. `jq` reaches the kernel through 396/397/398/399/406 and never
    /// through 3/4/5/6 for those operations — a plain-only table forwards a raw guest fd silently.
    #[test]
    fn nocancel_variants_are_tabled_beside_their_plain_forms() {
        assert_eq!(fd_operands(SYS_READ), fd_operands(SYS_READ_NOCANCEL));
        assert_eq!(fd_operands(SYS_WRITE), fd_operands(SYS_WRITE_NOCANCEL));
        assert_eq!(fd_operands(SYS_CLOSE), fd_operands(SYS_CLOSE_NOCANCEL));
        assert_eq!(fd_operands(SYS_FCNTL), fd_operands(SYS_FCNTL_NOCANCEL));
        assert_eq!(allocates_fd(SYS_OPEN), allocates_fd(SYS_OPEN_NOCANCEL));
    }

    #[test]
    fn m10_syscall_numbers() {
        assert_eq!((SYS_DUP, SYS_IOCTL, SYS_DUP2, SYS_FCNTL), (41, 54, 90, 92));
        assert_eq!((SYS_SOCKET, SYS_CONNECT, SYS_SENDTO), (97, 98, 133));
        assert_eq!((SYS_FGETATTRLIST, SYS_SHM_OPEN, SYS_FSTAT64), (228, 266, 339));
        assert_eq!((SYS_READ_NOCANCEL, SYS_OPEN_NOCANCEL, SYS_FCNTL_NOCANCEL), (396, 398, 406));
        assert_eq!((SYS_OPENAT, SYS_FSTATAT64, SYS_MAP_WITH_LINKING_NP), (463, 470, 550));
        assert_eq!((MWL_REGION_STRIDE, MWL_MAX_REGION_COUNT, AT_FDCWD), (32, 5, -2));
    }

    #[test]
    fn console_close_covers_both_close_variants_on_the_standard_fds() {
        for num in [SYS_CLOSE, SYS_CLOSE_NOCANCEL] {
            for fd in 0..=2 {
                assert!(is_console_close(num, fd), "fd {fd} is retrace's own descriptor");
            }
            assert!(!is_console_close(num, 3), "an ordinary file fd is forwarded, not faked");
        }
        assert!(!is_console_close(SYS_WRITE, 1), "only close is faked; write is mirrored");
    }

    #[test]
    fn console_write_covers_both_write_variants_on_fd_1_and_2() {
        for num in [SYS_WRITE, SYS_WRITE_NOCANCEL] {
            assert!(is_console_write(num, 1), "fd 1 is the console");
            assert!(is_console_write(num, 2), "fd 2 is the console");
            assert!(!is_console_write(num, 0), "fd 0 is stdin, not a console write");
            assert!(!is_console_write(num, 3), "an ordinary file fd is forwarded, not mirrored");
        }
        // Anything else is a normal syscall even on fd 1 — only the write family is mirrored.
        assert!(!is_console_write(SYS_READ, 1));
        assert!(!is_console_write(SYS_CLOSE, 1));
    }

    #[test]
    fn decodes_aut_destination_register() {
        // AUTDB x16, x17 — the observed objc fault at addClassTableEntry+0x70.
        assert_eq!(decode_aut_rd(0xDAC1_1E30), Some(16));
        // Each register variant returns Rd (bits [4:0]).
        assert_eq!(decode_aut_rd(0xDAC1_1000 | (1 << 5)), Some(0));        // AUTIA x0, x1
        assert_eq!(decode_aut_rd(0xDAC1_1400 | (2 << 5) | 3), Some(3));    // AUTIB x3, x2
        assert_eq!(decode_aut_rd(0xDAC1_1800 | (10 << 5) | 9), Some(9));   // AUTDA x9, x10
        assert_eq!(decode_aut_rd(0xDAC1_3800 | 30), Some(30));            // AUTDZA x30 (Z form)
        // Not an AUT-with-Rd: NOP, and PACIA (a SIGN, base 0xDAC1_0000) must return None.
        assert_eq!(decode_aut_rd(0xD503_201F), None);                     // NOP
        assert_eq!(decode_aut_rd(0xDAC1_0000 | (1 << 5)), None);          // PACIA x0, x1 (sign)
    }

    #[test]
    fn decodes_instruction_and_data_aborts() {
        assert_eq!(ec_of(0x20u64 << 26), Ec::InstrAbort);
        assert_eq!(ec_of(0x21u64 << 26), Ec::InstrAbort);
        assert_eq!(ec_of(0x24u64 << 26), Ec::DataAbort);
    }
}
