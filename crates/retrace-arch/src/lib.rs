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
/// `bsdthread_create(func, func_arg, stack, pthread, flags)`. **Never forwarded** — the host would
/// create a real thread inside retrace's own process, starting at a GUEST address. M14 emulates it.
pub const SYS_BSDTHREAD_CREATE: u64 = 360;
/// `bsdthread_terminate(stackaddr, freesize, port, sem)` — a guest thread's exit.
pub const SYS_BSDTHREAD_TERMINATE: u64 = 361;
/// `bsdthread_register(threadstart, wqthread, pthsize, …)`. Already fires on EVERY dynamic guest
/// since M7, unremarked; `threadstart` is the address a new thread must be entered at.
pub const SYS_BSDTHREAD_REGISTER: u64 = 366;
/// `thread_selfid()` — already fires and already survives.
pub const SYS_THREAD_SELFID: u64 = 372;
/// `__ulock_wait(operation, addr, value, timeout_us)` — the primitive `__pthread_join`'s retry
/// loop blocks on (M14 Task 1, pinned by disassembly of `___ulock_wait`'s `mov x16, #0x203; svc
/// #0x80` and cross-checked against the SDK's `sys/syscall.h`). `psynch_cvwait` and Mach
/// `semaphore_wait` do not appear anywhere in `__pthread_join`; `___semwait_signal_nocancel` does,
/// but strictly downstream of this call's retry loop, gated on state a plain join never sets.
pub const SYS_ULOCK_WAIT: u64 = 515;
/// `__ulock_wake(operation, addr, wake_value)` — the other half of the pair `SYS_ULOCK_WAIT`
/// services, pinned the same way (disassembly of `___ulock_wake`'s `mov x16, #0x204; svc #0x80`
/// — M14 Task 8 fix round 1, I-1). While re-pinning it, the same method also caught a stale claim
/// in this constant's neighbour's doc and in Task 1's report: their "candidate list" labels 516 as
/// `__ulock_wait2` — but `___ulock_wait2` actually disassembles to `mov x16, #0x220` (544), not
/// 516. 516 is `__ulock_wake`, confirmed directly, not inferred.
///
/// UNMEASURED beyond the number: Task 1 measured only the WAIT side of `__pthread_join`; nothing
/// in this milestone has measured what address the EXITING thread wakes on, or even confirmed it
/// calls this at all. Deliberately unmodelled — asserted in `retrace-core`'s record dispatch
/// rather than forwarded, which would issue a real `__ulock_wake` from retrace's own process
/// against a guest address, the exact hazard `SYS_ULOCK_WAIT`'s own doc cites, applied to its
/// pair. Assigned to M14 Task 9 (measure the exit-side wake address first).
pub const SYS_ULOCK_WAKE: u64 = 516;
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

// ---- M12-signal-delivery ---------------------------------------------------------------------
// Signal numbers and si_codes from sys/signal.h; SA_*/SS_* from the same header. Every value here
// was read out of the live SDK by spikes/sigabi.c, not from memory.
pub const SIGILL: u64 = 4;
pub const SIGTRAP: u64 = 5;
pub const SIGFPE: u64 = 8;
pub const SIGBUS: u64 = 10;
pub const SIGSEGV: u64 = 11;

pub const SEGV_MAPERR: u64 = 1;
pub const SEGV_ACCERR: u64 = 2;
pub const BUS_ADRALN: u64 = 1;
pub const BUS_ADRERR: u64 = 2;
pub const BUS_OBJERR: u64 = 3;
pub const ILL_ILLOPC: u64 = 1;
pub const TRAP_BRKPT: u64 = 1;
/// `si_code` a kernel-synthesized signal never has: it marks one raised by `kill`/`pthread_kill`
/// instead of a hardware trap. Not yet consumed by `signal_of_esr` (that function only ever sees a
/// fault ESR), but belongs beside its `SEGV_`/`BUS_`/`ILL_`/`TRAP_` siblings rather than being added
/// piecemeal later.
pub const SI_USER: u64 = 0x10001;

pub const SA_ONSTACK: u32 = 0x1;
pub const SA_RESTART: u32 = 0x2;
pub const SA_RESETHAND: u32 = 0x4;
pub const SA_NODEFER: u32 = 0x10;
pub const SA_SIGINFO: u32 = 0x40;

pub const SS_ONSTACK: u64 = 0x1;
pub const SS_DISABLE: u64 = 0x4;

/// The `infostyle` the kernel passes in `x1` on `sa_tramp` entry for an `SA_SIGINFO` handler.
/// Measured as `0x1e` by `spikes/sigtramp.c`; `UC_FLAVOR` is xnu's name for it.
pub const UC_FLAVOR: u64 = 30;

/// Classify a guest fault into the `(signal, si_code)` a real kernel would deliver.
///
/// Pure: a function of the ESR alone. The DFSC (`ISS[5:0]`) distinguishes "nothing is mapped there"
/// (translation fault → `SEGV_MAPERR`) from "mapped, but not for that" — and that second case splits
/// again on Darwin: an access-flag fault still reads as `SEGV_ACCERR`, but a permission fault
/// (M13, measured by `spikes/protnone.c`) is `SIGBUS`/`BUS_ADRALN` — Darwin's `ux_exception` maps
/// `KERN_PROTECTION_FAILURE` to `SIGBUS`, not `SIGSEGV`.
///
/// **A deliberate divergence from one host observation.** `spikes/sigtramp.c` recorded the host
/// delivering `SEGV_ACCERR` for a store to a wholly unmapped address, where the DFSC says
/// `MAPERR`. The host's answer reflects its own VM regime (a Mach protection failure on a submap
/// retrace does not reproduce); the guest's fault is described completely by its ESR, so the ESR is
/// what retrace derives from. Nothing in the gate set depends on the choice — libstd keys on
/// `si_addr` — which is exactly why it is made deliberately here rather than by accident.
pub fn signal_of_esr(esr: u64) -> (u64, u64) {
    let ec = (esr >> 26) & 0x3f;
    match ec {
        // Instruction / data abort from a lower EL: the guest touched something it could not.
        0x20 | 0x24 => match esr & 0x3f {
            0x04..=0x07 => (SIGSEGV, SEGV_MAPERR), // translation fault, levels 0..3
            0x08..=0x0b => (SIGSEGV, SEGV_ACCERR), // access-flag fault
            // M13, MEASURED (spikes/protnone.c): Darwin's ux_exception translates EXC_BAD_ACCESS by
            // code — KERN_INVALID_ADDRESS to SIGSEGV, and everything else, including
            // KERN_PROTECTION_FAILURE, to SIGBUS. libstd's install_main_guard comment says the same
            // of its own guard page. The previous SIGSEGV here was the Linux answer and had never
            // been reached by a running guest: every fault M6/M11/M12 recorded was a TRANSLATION
            // fault (0x04..0x07), whose row is unchanged and still SIGSEGV.
            0x0c..=0x0f => (SIGBUS, BUS_ADRALN),   // permission fault
            0x10..=0x13 => (SIGBUS, BUS_OBJERR),   // synchronous external abort
            0x21 => (SIGBUS, BUS_ADRALN),          // alignment fault
            // Deliberately NOT a panic, unlike the outer EC match below — and that asymmetry is
            // the point, not an inconsistency. EC alone already told us this is an abort, so the
            // SIGNAL is settled (SIGSEGV); an unenumerated DFSC only leaves `si_code` uncertain,
            // and `si_code` is a field nothing in the gate set reads (libstd's SIGSEGV handling
            // keys on `si_addr`, never `si_code`). Once a later task wires this into every stage-1
            // guest fault — including ones that would otherwise record as an uncaught
            // `Event::Crash` — panicking here would crash the RECORDER over an exotic-but-still-
            // recordable guest fault, to buy precision in a field nothing consumes. So: SIGSEGV
            // with the closest access-error code, not a fail-loud abort.
            _ => (SIGSEGV, SEGV_ACCERR),
        },
        0x26 => (SIGBUS, BUS_ADRALN),  // SP alignment fault
        0x00 | 0x0e => (SIGILL, ILL_ILLOPC), // unknown reason / illegal execution state
        0x3c => (SIGTRAP, TRAP_BRKPT), // BRK instruction
        // Unlike the DFSC fallback above, THIS one stays fail-loud: an unmodelled EC means retrace
        // cannot even name which signal this is, not merely which si_code — there is nothing
        // "closest" to default to without risking a plausible lie about the signal itself.
        _ => panic!(
            "signal_of_esr: EC {ec:#x} (esr={esr:#x}) has no modelled signal mapping. It reached \
             the fault path, so it is a real guest fault retrace cannot name — add the class here \
             deliberately rather than defaulting it to SIGSEGV, which would be a plausible lie."),
    }
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

    #[test]
    fn signal_of_esr_maps_the_fault_classes_by_dfsc() {
        // EC 0x24 = data abort from a lower EL. DFSC lives in ISS[5:0].
        // 0b0001LL (0x04..0x07) = translation fault  -> SEGV_MAPERR (nothing is mapped there)
        assert_eq!(signal_of_esr(0x9200_0006), (SIGSEGV, SEGV_MAPERR), "translation fault, level 2");
        assert_eq!(signal_of_esr(0x9200_0005), (SIGSEGV, SEGV_MAPERR), "translation fault, level 1");
        // 0b0011LL (0x0C..0x0F) = permission fault -> SIGBUS/BUS_ADRALN on Darwin (M13, measured by
        // spikes/protnone.c) rather than the Linux-shaped SEGV_ACCERR this row used to assert.
        assert_eq!(signal_of_esr(0x9200_000f), (SIGBUS, BUS_ADRALN), "permission fault, level 3");
        // 0b0010LL (0x08..0x0B) = access-flag fault -> also an access error
        assert_eq!(signal_of_esr(0x9200_0009), (SIGSEGV, SEGV_ACCERR), "access flag fault");
        // 0x21 = alignment fault -> SIGBUS
        assert_eq!(signal_of_esr(0x9200_0021), (SIGBUS, BUS_ADRALN), "alignment fault");
        // 0x10..0x13 = synchronous external abort -> SIGBUS/BUS_OBJERR
        assert_eq!(signal_of_esr(0x9200_0010), (SIGBUS, BUS_OBJERR), "external abort");
    }

    // M13: the permission-fault row, MEASURED (spikes/protnone.c) rather than assumed. Every fault
    // M6/M11/M12 ever recorded was a TRANSLATION fault, so this row shipped unexercised for six
    // milestones. Darwin's ux_exception maps KERN_PROTECTION_FAILURE to SIGBUS, not to what the
    // Linux-shaped table said.
    #[test]
    fn a_permission_fault_takes_the_darwin_signal() {
        // DFSC 0x0f = permission fault, level 3. Bit 6 of the ISS is WnR: 0 = load, 1 = store.
        assert_eq!(signal_of_esr(0x9200_000f), (SIGBUS, BUS_ADRALN), "permission fault, load");
        assert_eq!(signal_of_esr(0x9200_004f), (SIGBUS, BUS_ADRALN), "permission fault, store");
        // The control that must NOT move: an unmapped address is a TRANSLATION fault and stays
        // SIGSEGV/SEGV_MAPERR, which is what crashy_e2e and segv_rust_e2e rest on.
        assert_eq!(signal_of_esr(0x9200_0006), (SIGSEGV, SEGV_MAPERR), "translation fault, level 2");
    }

    #[test]
    fn signal_of_esr_maps_instruction_aborts_the_same_way() {
        // EC 0x20 = instruction abort from a lower EL; same DFSC encoding.
        assert_eq!(signal_of_esr(0x8200_0006), (SIGSEGV, SEGV_MAPERR));
        assert_eq!(signal_of_esr(0x8200_000f), (SIGBUS, BUS_ADRALN));
    }

    #[test]
    fn signal_of_esr_maps_the_non_abort_classes() {
        assert_eq!(signal_of_esr(0x9800_0000), (SIGBUS, BUS_ADRALN), "EC 0x26: SP alignment");
        assert_eq!(signal_of_esr(0x0000_0000), (SIGILL, ILL_ILLOPC), "EC 0x00: unknown/undefined");
        assert_eq!(signal_of_esr(0x3800_0000), (SIGILL, ILL_ILLOPC), "EC 0x0e: illegal execution state");
        assert_eq!(signal_of_esr(0xf000_0000), (SIGTRAP, TRAP_BRKPT), "EC 0x3c: BRK");
    }

    // The measured ESR from spikes/sigtramp.c, end to end. A store to an unmapped page.
    #[test]
    fn signal_of_esr_classifies_the_measured_probe_esr() {
        assert_eq!(signal_of_esr(0x9200_0046), (SIGSEGV, SEGV_MAPERR),
            "0x92000046 is what the host kernel put in the probe's mcontext: EC 0x24, WnR set, DFSC 0x06");
    }

    /// Covers the outer match's fail-loud fallback. EC 0x01 (trapped WFI/WFE) is a real AArch64
    /// exception class but one `signal_of_esr` deliberately does not model — an unmodelled EC means
    /// retrace cannot name even the SIGNAL, so it panics rather than guess.
    #[test]
    #[should_panic(expected = "EC 0x1")]
    fn signal_of_esr_panics_on_an_unmodelled_ec() {
        signal_of_esr(0x0400_0000); // EC 0x01 << 26
    }

    /// Covers the inner match's silent fallback: an abort EC (so the SIGNAL is settled) paired
    /// with a DFSC outside every enumerated range. `0x00` ("address size fault, level 0" in the
    /// real DFSC table) is not translation/access-flag/permission/external-abort/alignment, so it
    /// falls to the default arm. Unlike the EC fallback above, this one must NOT panic — see the
    /// comment on that arm for why the two fallbacks deliberately differ.
    #[test]
    fn signal_of_esr_defaults_an_unenumerated_dfsc_on_a_known_abort() {
        assert_eq!(signal_of_esr(0x9200_0000), (SIGSEGV, SEGV_ACCERR),
            "EC 0x24 (data abort) with DFSC 0x00: signal is settled, si_code defaults");
    }

    #[test]
    fn signal_constants_match_the_sdk() {
        assert_eq!((SIGILL, SIGTRAP, SIGFPE, SIGBUS, SIGSEGV), (4, 5, 8, 10, 11));
        assert_eq!((SEGV_MAPERR, SEGV_ACCERR), (1, 2));
        assert_eq!((BUS_ADRALN, BUS_ADRERR, BUS_OBJERR), (1, 2, 3));
        assert_eq!((SA_ONSTACK, SA_RESTART, SA_RESETHAND, SA_NODEFER, SA_SIGINFO),
                   (0x1, 0x2, 0x4, 0x10, 0x40));
        assert_eq!((SS_ONSTACK, SS_DISABLE), (0x1, 0x4));
        assert_eq!(UC_FLAVOR, 30, "measured in spikes/sigtramp.c as x1 on trampoline entry");
        assert_eq!(SI_USER, 0x10001, "measured by spikes/sigabi.c");
    }

    #[test]
    fn thread_syscall_numbers_are_the_darwin_ones() {
        // Measured on macOS 26 (M14 Task 2): a NON-threading Rust guest already issues 366 and 372.
        assert_eq!(
            (SYS_BSDTHREAD_CREATE, SYS_BSDTHREAD_TERMINATE, SYS_BSDTHREAD_REGISTER, SYS_THREAD_SELFID,
             SYS_ULOCK_WAIT),
            (360, 361, 366, 372, 515)
        );
    }
}
