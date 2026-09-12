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
/// `pread_nocancel`. Header-derived (`sys/syscall.h`: `#define SYS_pread_nocancel 414`).
pub const SYS_PREAD_NOCANCEL: u64 = 414;
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
/// `fstatfs64(int, struct statfs64 *)` — SDK `sys/mount.h:444`, header-declared like its M10
/// siblings.
pub const SYS_FSTATFS64: u64 = 346;
/// `getdirentries64` — **not in the SDK at all** (libc calls it privately from `opendir`/
/// `readdir`, so no `sys/syscall.h`-adjacent header declares its prototype). Its fd-in-`x0`
/// position is therefore MEASURED, not header-derived: M25-cpython Finding 3 captured the trap
/// arguments `[fd=0x4, buf, 0x2000, &basep]` from CPython's stdlib-directory listing.
pub const SYS_GETDIRENTRIES64: u64 = 344;
/// `recvfrom(int s, void *buf, size_t len, int flags, struct sockaddr *from, socklen_t *fromlen)`.
/// SDK `sys/syscall.h`. Its `x0` is a socket fd, so it belongs in `fd_operands` too — it was in
/// neither table before M29, while its `sendto` counterpart was already in `fd_operands`.
pub const SYS_RECVFROM: u64 = 29;
/// The `_nocancel` spelling of `recvfrom`. Every `_nocancel` variant this repo has met so far was
/// missing from a table its plain sibling was in (M9's console bug, M10's `read_nocancel`, M27's
/// `pread_nocancel`); adding both together is the only way that trap stops repeating.
pub const SYS_RECVFROM_NOCANCEL: u64 = 403;
/// `sysctlbyname(const char *name, void *oldp, size_t *oldlenp, void *newp, size_t newlen)`.
pub const SYS_SYSCTLBYNAME: u64 = 274;
/// `getfsstat64(struct statfs64 *buf, int bufsize, int flags)`. `bufsize` is in BYTES.
pub const SYS_GETFSSTAT64: u64 = 347;

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

/// `ioctl` request-code decode, `sys/ioccom.h:74-85`: the parameter length lives in bits 16..29
/// of the request (`IOCPARM_LEN`), capped by `IOCPARM_MASK` (0x1fff, 8191) — the cited bound on
/// `ioctl`'s DIRECT parameter (see its `arg_kinds` row); `IOC_IN`/`IOC_OUT` say which way the
/// kernel copies it.
pub const IOCPARM_MASK: u64 = 0x1fff;
pub const IOC_OUT: u64 = 0x4000_0000;
pub const IOC_IN: u64 = 0x8000_0000;
/// `IOCPARM_LEN(x)`: `((x) >> 16) & IOCPARM_MASK`.
pub fn iocparm_len(request: u64) -> u64 { (request >> 16) & IOCPARM_MASK }

/// Where a destination buffer's byte length lives for a given syscall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestLen {
    /// The length is the value of register `x{n}`.
    Reg(usize),
    /// The length is a `u64` in GUEST MEMORY at the address in `x{n}` — `sysctl`'s `*oldlenp`.
    DerefU64(usize),
}

/// What the kernel does with ONE register argument of a syscall.
///
/// M33's unification. Five functions used to answer "what does this syscall do with each of its
/// arguments" in five incompatible shapes — `fd_operands` and `dest_buffer` keyed by argument
/// index, `reads_guest_buffer` and `writes_via_nested_pointer` by whole syscall (so they lost the
/// index), `allocates_fd` by return value — and nothing could check one against another. M30
/// tabled `pwrite`, `writev`, `sendmsg` and `sendfile` as readers from their prototypes while
/// `fd_operands` still said each "takes no fd". They are VIEWS over this table now
/// (`Shape::fd_operands` etc.), and `tests/legacy_equivalence.rs` proves each still answers what
/// it answered at `e13eb17`, entry for entry, except where its `EXPECTED_DIFFS` says why.
///
/// **Load-bearing kinds:** `Fd`, `Source`, `NestedSource`, `Dest`, `NestedDest` and `Ret::Fd`
/// each change what the box does. **`Scalar`, `Path` and `Ptr` change nothing at runtime** —
/// `forward_and_diff` probes `host_span` on all eight registers regardless — and are
/// documentation until a later milestone consults them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    /// Not a memory reference: a count, a flag word, an offset, a signal number, a port name.
    Scalar,
    /// A GUEST file descriptor — `translate_fds` rewrites it to the host's before forwarding.
    ///
    /// The M10 analogue of `is_console_write`: one shared table rather than a condition spelled
    /// out at each call site, because a forgotten `Fd` does not diverge loudly — it forwards a raw
    /// guest fd to the host kernel, which then acts on RETRACE's descriptor of that number. A
    /// position that is not `Fd` is simply not translated, so a non-`Fd` position must mean
    /// "provably not a descriptor", never "not gotten to yet". (Before M33 the same silence
    /// covered a whole syscall absent from the table; an absent ROW is loud now — `forwarded_shape`
    /// panics on it at the forward point — and only the per-position silence remains.)
    ///
    /// `dirfd` positions (`openat`, `fstatat64`) are `Fd` too; `AT_FDCWD` is negative and passes
    /// through translation untouched.
    Fd,
    /// A NUL-terminated path. **Why every path-taking call is absent from the readers**, since
    /// they plainly read guest memory: the kernel stops at `PATH_MAX` (1024), which sits far
    /// inside the 64 KiB production window, so no band can be in reach. That bound is the
    /// argument — not "paths are short" — and it is why a test that shrinks `window_cap` below
    /// `PATH_MAX` can make the kernel read a canary as path bytes while production cannot.
    Path,
    /// The kernel READS a caller-sized buffer through it, and no kernel-side cap below the window
    /// can be cited. (M30's membership rule — in full below.)
    ///
    /// The M30 guard-band canary is written into guest memory just past each argument's diff
    /// window and restored before the vCPU resumes, so no *guest* can observe it. The kernel can.
    /// When a syscall READS more than its diff window through a pointer, it consumes those canary
    /// bytes as data: the guest's externally visible output is silently wrong on record, while
    /// record and replay stay bit-identical (the bytes are restored, so nothing diverges) — the
    /// one failure class a determinism oracle cannot see. `forward_and_diff` therefore skips the
    /// fill entirely for a call with any `Source` or `NestedSource` argument, falling back to
    /// M27's original before/after band comparison.
    ///
    /// **REPRODUCED** at M30 Task 4 fix round 1: a guest writing a 128 KiB buffer of `'A'`
    /// produced 64 corrupt bytes at offset `0x10080`, matching `canary_byte` exactly. The
    /// corrupting entry was a **stale register** holding `buf + 128`, whose own band therefore
    /// landed 64 KiB downstream — inside the region the kernel read. So the trigger is not "an
    /// input buffer bigger than the window": ANY register pointing into a large buffer plants a
    /// canary 64 KiB past itself, and the band shrink cannot help (that same stale window had
    /// shrunk the real buffer's band to zero). That is why the exclusion is per CALL, not per
    /// declared argument: `reads_guest_buffer` answers for the whole syscall and the caller must
    /// skip all eight registers, not just the arguments the call actually declares.
    ///
    /// **Membership rule**: the call's documented contract has the kernel read a caller-supplied
    /// buffer whose length the CALLER chooses, and which no `Dest` entry widens the window to
    /// cover. That is checkable against the man page and `sys/syscall.h`, not guessed. A bound
    /// that cannot be cited is not a bound — then the argument is `Source`, not `Ptr`.
    ///
    /// **Deliberately asymmetric.** Under-including corrupts the guest's output silently, so
    /// where the two errors compete, listing wins. Every `_nocancel` spelling shares its plain
    /// form's row: that pairing is the trap M9, M10 and M27 each hit separately. Whatever is
    /// listed still gets M27's before/after band comparison, which `forward_and_diff` runs
    /// unchanged on an unfilled band, so nothing here is left weaker than it was before M30.
    ///
    /// **But over-including is NOT free.** The claim that a listed call "has no destination
    /// buffer worth canarying" holds for `write`, `writev`, `sendto` and `msync`. It is FALSE for
    /// `sendfile` (337) and `mach_msg2_trap` (-47) — each row says why — so for those two the
    /// exclusion costs real destination-side canary coverage. They stay listed anyway: both
    /// genuinely read guest memory, dropping either risks the reproduced Critical, and a decision
    /// keyed on `num` alone has no way to say "fill past argument 3 but not argument 4".
    /// Recovering that coverage needed a per-ARGUMENT direction notion the old predicate could not
    /// express; this table IS that notion, but the stale-register reproduction above is why the
    /// fill decision still cannot use it per argument — recovering the coverage stays owed.
    ///
    /// **The residual gap, stated plainly**: an unlisted syscall that reads past its window still
    /// corrupts, in exactly the way the reproduction did and with exactly as little noise. The
    /// rows are the contract-derived family of guest→kernel transfers, not a proof of
    /// exhaustiveness; `ioctl`'s nested pointers are the clearest admitted hole (see its row).
    Source,
    /// The kernel reads through pointers INSIDE the pointed-to struct (`iovec.iov_base`,
    /// `msghdr.msg_iov`, `sf_hdtr`, `posix_spawn`'s `argv`). The TOTAL across an iovec vector is
    /// unbounded even though the iovec array itself is small. Not translated — a guest IPA reaches
    /// the kernel as a host address, so the read returns wrong bytes or `EFAULT`. A fidelity
    /// hazard, not a wild write; forwarded exactly as before M33, when `writes_via_nested_pointer`
    /// covered the nested shape for the WRITE side only and this side was listed among the readers
    /// as a separate hazard. Counts as a reader for the canary decision.
    NestedSource,
    /// The kernel WRITES through it and the length is where `DestLen` says.
    ///
    /// **The forwarded-count clamp and the diff window must both consult this**, which is why it
    /// is one table rather than a predicate per shape. The clamp decides how many bytes the host
    /// kernel may write into the guest buffer; the window decides how many are looked at
    /// afterwards and captured as `Event::Syscall` writes. A disagreement between them is the M26
    /// defect: the kernel writes past what the diff inspects, the excess lands in guest memory on
    /// record and in no `Event`, and replay restores stale bytes there — invisibly, because
    /// `(num, args)` still match. No row has more than one `Dest`: `dest_buffer` returns ONE and
    /// the clamp/window consult it, so a second would be picked silently — the schema test forbids
    /// it.
    ///
    /// **Seeded only with what is measured or SDK-verified.** M29 added `getdirentries64`,
    /// `recvfrom` (both spellings) and `getfsstat64`/`sysctlbyname` (sysctl's own shape); each of
    /// those rows names its own second destination where it has one, so a later reader can see it
    /// was considered and dismissed on a number rather than overlooked. Other syscalls are still
    /// structurally capable of overrunning (`proc_info`, `getattrlist`, `csops`) and remain
    /// deliberately `Ptr`: none has been measured to do so, and the M27 guard band exists
    /// precisely so they announce themselves instead of being guessed at. `Ptr` there means "not
    /// measured", the guard band is what makes that safe, and M34 is to measure each (spec §7).
    Dest(DestLen),
    /// The kernel writes through pointers INSIDE the pointed-to struct (`iovec.iov_base`,
    /// `msghdr.msg_iov`).
    ///
    /// M27: `forward_and_diff` translates only top-level register arguments, so a guest IPA would
    /// reach the host kernel AS A HOST ADDRESS. That is not a fidelity gap like the truncation
    /// class — it is a potential wild write into retrace's own process. The reading that they
    /// would merely `EFAULT` (guest IPAs being unlikely to be mapped in retrace's process) is an
    /// INFERENCE, and the downside of it being wrong is severe. So they are refused by value in
    /// retrace-core rather than tested or translated, the way `guest_workq_kernreturn` refuses an
    /// unenumerated opcode — never forwarded. Translating them properly needs the
    /// `translate_mwl_regions` treatment plus its own measurement of the struct layout.
    ///
    /// The six rows are the MEASURED family, not a proof of exhaustiveness — all header-derived
    /// from `sys/syscall.h`. `aio_read` (216, via `aiocb.aio_buf`) has the same nested-write shape
    /// and no row, because no guest this repo runs is measured to call it (`forwarded_shape` names
    /// it if one does); `sendfile` (337, via `sf_hdtr`'s iovecs) has the nested shape on the READ
    /// side and is `NestedSource` on its row.
    NestedDest,
    /// A pointer modelled no further than the flat window and the guard band: a read the kernel
    /// itself bounds far inside the window (a `sockaddr`, an `ioctl` parameter), a fixed struct it
    /// writes (`struct stat`), or an in/out scalar. **The row comment names the bound and its
    /// citation.** A bound that cannot be cited is not a bound — then the argument is `Source`.
    Ptr,
}

/// What the return value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ret {
    Plain,
    /// A NEW guest descriptor, bound to a fresh guest slot by `bind_returned_fd`. `socket` and
    /// `shm_open` are here for the same reason `open` is: guest fds are not files-only.
    ///
    /// **`dup2` is deliberately `Plain`.** It names its own target descriptor instead of taking
    /// the lowest free one, so binding its return like the others would put the new mapping in the
    /// wrong slot. No guest in the gate calls it (measured: zero in the `jq` run), so retrace-core
    /// asserts on it rather than modelling it wrong — a silently mis-modelled `dup2` aliases the
    /// wrong file.
    Fd,
    /// Two new descriptors, in x0 and x1 — `pipe`. Unmodelled: `allocates_fd` is false for it
    /// (binding one of two would alias), and retrace-core does not assert on it. What the guest
    /// actually gets today: `Box_::host_svc` captures only `x0` and the carry, and
    /// `apply_and_return` sets `x0` alone, so the guest receives retrace's own host READ-end,
    /// unbound, in `x0` and its own stale `x1` — the write-end never reaches the guest at all, and
    /// both host descriptors leak in the recorder. Every later use returns EBADF via
    /// `translate_fds` (`/bin/zsh` issues it and never uses the pair). Capturing `x1` is the
    /// actual successor work item, ahead of any binding model — M10-successor work, owed.
    FdPair,
}

/// One syscall's argument kinds, in register order, plus its return kind.
#[derive(Debug, PartialEq, Eq)]
pub struct Shape {
    pub args: &'static [ArgKind],
    pub ret: Ret,
}

impl Shape {
    /// Which operand indices hold a GUEST descriptor (view: the old `fd_operands`).
    pub fn fd_operands(&self) -> impl Iterator<Item = usize> + '_ {
        self.args.iter().enumerate().filter(|(_, k)| **k == ArgKind::Fd).map(|(i, _)| i)
    }
    /// The destination buffer as `(argument index, where its length lives)` (view: `dest_buffer`).
    pub fn dest_buffer(&self) -> Option<(usize, DestLen)> {
        self.args.iter().enumerate().find_map(|(i, k)| match k {
            ArgKind::Dest(len) => Some((i, *len)),
            _ => None,
        })
    }
    /// Does the kernel read guest memory through this call in an amount no window bounds?
    pub fn reads_guest_buffer(&self) -> bool {
        self.args.iter().any(|k| matches!(k, ArgKind::Source | ArgKind::NestedSource))
    }
    /// Does the kernel write through a pointer inside a guest struct? (Refused by value.)
    pub fn writes_via_nested_pointer(&self) -> bool { self.args.contains(&ArgKind::NestedDest) }
    /// Does the return value need binding to a fresh guest fd slot?
    pub fn allocates_fd(&self) -> bool { self.ret == Ret::Fd }
}

/// The table. `None` means UNENUMERATED — no guest in this repo's corpora has been measured to
/// dispatch `num` and no legacy table listed it. `forwarded_shape` turns that into a panic at the
/// forward point; the views below turn it into their empty answer, because they are also consulted
/// on replay for events of syscalls the box emulates above the trace.
///
/// Every row's comment is its C prototype. Rows are written from the prototype and never bent to
/// match a legacy table; `tests/legacy_equivalence.rs` lists each disagreement with its reason.
/// Mach traps are keyed by the two's-complement `u64` the trap carries (`-47` is mach_msg2).
pub fn arg_kinds(num: u64) -> Option<&'static Shape> {
    use ArgKind::*;
    use DestLen::{DerefU64, Reg};
    const P: Ret = Ret::Plain;
    const F: Ret = Ret::Fd;
    macro_rules! row {
        ($ret:expr, [$($k:expr),* $(,)?]) => { Some(&Shape { args: &[$($k),*], ret: $ret }) };
    }
    match num {
        // ---- the read/write families -------------------------------------------------------
        // read(int fd, void *buf, size_t nbyte) / read_nocancel. **`_nocancel` variants share
        // their plain form's row deliberately.** macOS libc routinely takes ONLY the `_nocancel`
        // path — measured in one `jq` run: `read`(3) is called zero times and `read_nocancel`(396)
        // twice; `fcntl_nocancel`(406) appears alongside `fcntl`(92). A plain-only table fails
        // *silently*, which is exactly how M9's console bug survived until `jq`.
        SYS_READ | SYS_READ_NOCANCEL => row!(P, [Fd, Dest(Reg(2)), Scalar]),
        // pread(int fd, void *buf, size_t nbyte, off_t offset) / pread_nocancel. 414 was missing
        // from THREE tables at once before M27 (fd, clamp, window), and the missing clamp was the
        // serious one: an unclamped forward lets the host kernel write past the guest buffer's
        // backing. One row cannot be missing from one view and not another.
        SYS_PREAD | SYS_PREAD_NOCANCEL => row!(P, [Fd, Dest(Reg(2)), Scalar, Scalar]),
        // write(int fd, const void *buf, size_t nbyte) / write_nocancel: x1 is read for x2 bytes,
        // and x2 is the caller's. This is M30's reproduced canary case.
        SYS_WRITE | SYS_WRITE_NOCANCEL => row!(P, [Fd, Source, Scalar]),
        // pwrite(int fd, const void *buf, size_t nbyte, off_t offset) / pwrite_nocancel. M30
        // tabled both as readers; fd_operands never had either (EXPECTED_DIFFS).
        154 | 415 => row!(P, [Fd, Source, Scalar, Scalar]),
        // writev(int fd, const struct iovec *iov, int iovcnt) / writev_nocancel: the kernel reads
        // each iov_base for iov_len — nested, caller-sized, untranslated (M30). 412 is the one
        // untranslated-fd row a corpus guest (/bin/ed) is measured to dispatch (tests/census.rs).
        121 | 412 => row!(P, [Fd, NestedSource, Scalar]),
        // pwritev(int fd, const struct iovec *iov, int iovcnt, off_t offset)
        541 => row!(P, [Fd, NestedSource, Scalar, Scalar]),
        // readv(int fd, struct iovec *iov, int iovcnt) / readv_nocancel: the kernel WRITES through
        // iov_base — refused by value in retrace-core before translate_fds ever runs (M27).
        120 | 411 => row!(P, [Fd, NestedDest, Scalar]),
        // preadv(int fd, struct iovec *iov, int iovcnt, off_t offset)
        540 => row!(P, [Fd, NestedDest, Scalar, Scalar]),
        // ---- sockets ------------------------------------------------------------------------
        // recvmsg(int s, struct msghdr *msg, int flags) / recvmsg_nocancel: msg_iov is nested.
        27 | 401 => row!(P, [Fd, NestedDest, Scalar]),
        // recvmsg_x(int s, struct msghdr_x *msgp, u_int cnt, int flags): the public SDK carries
        // only the number (`SYS_recvmsg_x 480`, sys/syscall.h); the prototype and `struct
        // msghdr_x` are xnu's private bsd/sys/socket.h (`PRIVATE` block) — the source named the
        // way getdirentries64's measured shape is.
        480 => row!(P, [Fd, NestedDest, Scalar, Scalar]),
        // sendmsg(int s, const struct msghdr *msg, int flags) / sendmsg_nocancel: with sendto, the
        // socket-side spelling of the same two shapes — flat buffer and iovec vector.
        28 | 402 => row!(P, [Fd, NestedSource, Scalar]),
        // sendmsg_x(int s, const struct msghdr_x *msgp, u_int cnt, int flags): prototype from
        // xnu's private bsd/sys/socket.h, as for recvmsg_x — the SDK has only `SYS_sendmsg_x 481`.
        481 => row!(P, [Fd, NestedSource, Scalar, Scalar]),
        // sendto(int s, const void *buf, size_t len, int flags, const struct sockaddr *to,
        //        socklen_t tolen). `to`: the kernel rejects tolen > SOCK_MAXADDRLEN (255,
        // sys/socket.h) — a cited bound, so Ptr. 413 is the _nocancel spelling: it was in
        // reads_guest_buffer and not in fd_operands — the _nocancel trap a fourth time.
        SYS_SENDTO | 413 => row!(P, [Fd, Source, Scalar, Scalar, Ptr, Scalar]),
        // recvfrom(int s, void *buf, size_t len, int flags, struct sockaddr *from,
        //          socklen_t *fromlen): destination x1, length x2 (M29). It also writes `from`
        // (x4) — unmodelled by decision: the kernel caps that write at the real address size
        // (`sockaddr_storage` is 128 bytes), NOT at `*fromlen`, so it is self-bounding and already
        // deep inside the flat window; `*fromlen` (x5) is 4 bytes in-out. Its x0 is a socket fd:
        // it was in neither the fd nor the dest table before M29, while its `sendto` counterpart
        // was already in fd_operands — the M10 class, and the same both-tables-at-once asymmetry
        // M27 found in pread_nocancel.
        SYS_RECVFROM | SYS_RECVFROM_NOCANCEL => row!(P, [Fd, Dest(Reg(2)), Scalar, Scalar, Ptr, Ptr]),
        // connect(int s, const struct sockaddr *name, socklen_t namelen): namelen > SOCK_MAXADDRLEN
        // (255) is rejected — the cited bound.
        SYS_CONNECT => row!(P, [Fd, Ptr, Scalar]),
        // socket(int domain, int type, int protocol) → a NEW descriptor: guest fds are not
        // files-only.
        SYS_SOCKET => row!(F, [Scalar, Scalar, Scalar]),
        // sendfile(int fd, int s, off_t offset, off_t *len, struct sf_hdtr *hdtr, int flags).
        // No guest in this repo issues 337 (M32 §2, by grep) — every kind here is header truth,
        // unexercised. The bulk data comes from a FILE, but `hdtr`'s header and trailer iovecs are
        // guest memory the kernel reads with caller-chosen lengths: the nested read M30 listed.
        // `*len` is in-out, 8 bytes: the kernel writes the transferred byte count back through it
        // — a destination, which is why M30's "no destination worth canarying" claim is FALSE here
        // and the reader listing costs real destination-side canary coverage on this call.
        337 => row!(P, [Fd, Fd, Scalar, Ptr, NestedSource, Scalar]),
        // ---- memory -------------------------------------------------------------------------
        // msync(void *addr, size_t len, int flags) / msync_nocancel: `len` is the caller's and the
        // kernel reads the range to flush it — listed by M30 on that INFERENCE. All guest memory
        // is anonymous (the SPTM rule), which probably makes the read moot — but "probably" is an
        // inference about kernel internals, and the cost of being wrong is a silent corruption
        // while the cost of listing it is nothing.
        65 | 405 => row!(P, [Source, Scalar, Scalar]),
        // mmap(void *addr, size_t len, int prot, int flags, int fd, off_t offset): the fd is x4,
        // consumed by guest_mmap_file, which translates for itself and never reaches
        // forward_and_diff — the exception that makes a single choke point insufficient.
        SYS_MMAP => row!(P, [Scalar, Scalar, Scalar, Scalar, Fd, Scalar]),
        // ---- mach ---------------------------------------------------------------------------
        // mach_msg2_trap(void *msg, u64 options, u64 bits_and_send_size, u64 remote_and_local,
        //   u64 voucher_and_id, u64 desc_count_and_rcv_name, u64 rcv_size_and_priority, u64 timeout)
        // — keyed as the two's-complement of -47, matching how the negative mach traps are
        // compared everywhere else. The kernel reads the message buffer at x0; retrace-core bounds
        // `send_size` to `machmsg::SEND_SIZE_MAX` (4 KiB) by assert, before `route()` even runs.
        //
        // The receive buffer is the SAME pointer and a live destination: `machmsg.rs`'s
        // `FORWARD_ALLOWLIST` sends five ids through `forward_and_diff`, and `3405 task_info` and
        // `412 host_get_special_port` are there *precisely because* the kernel writes a reply into
        // guest memory which is then captured as `writes` — a path the corpus exercises on every
        // jq and CPython run, not a hypothetical. So M30's "no destination worth canarying" claim
        // is FALSE here too, and the reader listing costs destination-side coverage on live
        // traffic. It stays `Source`, unwidened, and the reason is now a measurement:
        //
        // **M32 measured the reason this entry used to give, and disproved it.** That reason was:
        // "a band lands wherever some register points, and nothing relates that to the message's
        // own extent." For argument 0 the two ARE related — the band's start is `ipa + window_cap`,
        // `window_cap` is 65536 on every production constructor, and the 4096 ceiling above holds
        // for every mach_msg2 call — so a band on this argument provably cannot land inside the
        // region the kernel reads. The old sentence is left NAMED rather than silently swapped,
        // per CLAUDE.md's own rule: a superseded claim is left standing with a forward pointer
        // rather than quietly corrected, so a reader who met it before can tell it was overturned
        // rather than wonder whether they misremembered it. (An earlier draft of this comment said
        // the sentence is kept "because it is cited by the proof that replaced it" — it is not
        // cited anywhere; that was a wrong supporting fact bolted to a right decision, which is the
        // class M32 spent the milestone catching.)
        //
        // **The entry stays anyway, and the reason is now the measurement, not the hazard.** M32
        // walked 35 real mach_msg2 landmarks across hello_dyn, jq and CPython: 13 were `Route::
        // Forward` (the only route `forward_and_diff`, and so any canary fill, ever runs for), and
        // their maximum `avail` was 24,672 bytes against the 65,536 a band needs to exist at all —
        // zero bands. Removing this entry would therefore change nothing observable today, while
        // requiring a per-ARGUMENT direction notion the old whole-syscall predicate could not
        // express. The residual is depth, not shape: nothing bounds the stack depth at which a
        // governed id can fire, and all 13 measured calls are process-initialisation calls. See
        // `docs/superpowers/specs/2026-09-09-retrace-m32-dirtable-design.md` §9 and M32's section
        // of `docs/status-log.md`.
        0xffff_ffff_ffff_ffd1 => row!(P, [Source, Scalar, Scalar, Scalar, Scalar, Scalar, Scalar, Scalar]),
        // ---- descriptors --------------------------------------------------------------------
        // close(int fd) / close_nocancel
        SYS_CLOSE | SYS_CLOSE_NOCANCEL => row!(P, [Fd]),
        // dup(int fd) → a NEW descriptor
        SYS_DUP => row!(F, [Fd]),
        // dup2(int fd, int fd2): both are descriptors; the return is NOT bound (see Ret::Fd).
        SYS_DUP2 => row!(P, [Fd, Fd]),
        // fcntl(int fd, int cmd, ...) / fcntl_nocancel (406 measured beside 92 in the jq run — see
        // the read row). The third argument is cmd-dependent: an int for F_GETFL/F_SETFD/F_DUPFD, a
        // pointer for F_GETPATH (writes ≤ MAXPATHLEN 1024) and F_PREALLOCATE (a 32-byte fstore_t,
        // in-out) — every pointer case far inside the window.
        SYS_FCNTL | SYS_FCNTL_NOCANCEL => row!(P, [Fd, Scalar, Ptr]),
        // fstat(int fd, struct stat *buf) / fstat64: a fixed 144-byte struct (sys/stat.h).
        SYS_FSTAT | SYS_FSTAT64 => row!(P, [Fd, Ptr]),
        // fstatfs64(int fd, struct statfs *buf): a fixed 2168-byte struct (measured at M29 Task 7).
        // M25-cpython Task 3, header-derived like its M10 siblings (see its constant).
        SYS_FSTATFS64 => row!(P, [Fd, Ptr]),
        // lseek(int fd, off_t offset, int whence)
        SYS_LSEEK => row!(P, [Fd, Scalar, Scalar]),
        // ioctl(int fd, unsigned long request, void *arg): the kernel copies IOCPARM_LEN(request)
        // bytes in and/or out, at most IOCPARM_MASK = 0x1fff (sys/ioccom.h:74) — the cited bound
        // on the DIRECT parameter (it is `iocparm_len`'s own mask, so the bound holds by
        // definition), and `ioctl_request_codes_the_corpora_issue_decode_as_the_row_states` pins
        // the decoded length and direction for every request the census measured (2026-09-12,
        // four distinct codes):
        //   0x4004667a FIODTYPE          _IOR('f', 122, int)          len 4, OUT
        //   0x40087468 TIOCGWINSZ        _IOR('t', 104, winsize)      len 8, OUT
        //   0x40487413 TIOCGETA          _IOR('t',  19, termios)      len 72, OUT
        //   0x80086804 DTRACEHIOC_ADDDOF _IOW('h',   4, user_addr_t)  len 8, IN
        // The fourth is the nested case §4b asked about: its 8-byte parameter IS a guest pointer
        // to a `dof_ioctl_data_t`, which the kernel follows (bsd/dev/dtrace/dtrace.c
        // `dtrace_ioctl_helper`, `copyin(user_address + offsetof(dof_ioctl_data_t,
        // dofiod_count), …)`), and dyld issues it on nearly every dynamic guest to register DOF
        // sections. MEASURED (M33 t5, 10 guests: jq, CPython, /bin/ps, ls, date, sh, zsh, sort,
        // sleep, hostname): every one returns `ret=0xe err=true` — EFAULT — because that first
        // nested copyin reads a GUEST address in retrace's process, so the later `copyout` of
        // generation ids into the same struct is unreachable and no guest byte is written. It is
        // a forwarded nested pointer, unrefused, and owed to a successor (the M27 class): the
        // refuse-by-value assert §4b pre-authorised would make every dynamic guest unrecordable,
        // which is the spec's own §9 halt clause (Ruling 4). Ptr on the direct bound; the nested
        // residual is named here, not modelled.
        SYS_IOCTL => row!(P, [Fd, Scalar, Ptr]),
        // fgetattrlist(int fd, struct attrlist *alist, void *attrbuf, size_t bufsize, u_long opts):
        // alist is a fixed 24-byte struct (sys/attr.h, measured with sizeof); attrbuf is a
        // destination of bufsize bytes — M34's row to widen (spec §7); Ptr until then.
        SYS_FGETATTRLIST => row!(P, [Fd, Ptr, Ptr, Scalar, Scalar]),
        // getdirentries64(int fd, char *buf, u_int bufsize, off_t *basep): destination x1, length
        // x2 (M29). It also writes 8 bytes at `*basep` (x3) — unmodelled by decision: 8 bytes sits
        // far inside the flat 64 KiB window every pointer argument already receives, so it cannot
        // produce the truncation class `Dest` exists to prevent. Its fd position is MEASURED, not
        // header-derived: the call is not in the SDK (M25-cpython Task 3, Finding 3 — see its
        // constant).
        SYS_GETDIRENTRIES64 => row!(P, [Fd, Dest(Reg(2)), Scalar, Ptr]),
        // ---- paths --------------------------------------------------------------------------
        // open(const char *path, int flags, mode_t mode) / open_nocancel → a NEW descriptor
        SYS_OPEN | SYS_OPEN_NOCANCEL => row!(F, [Path, Scalar, Scalar]),
        // openat(int dirfd, const char *path, int flags, mode_t mode) → a NEW descriptor. The
        // dirfd is translated; AT_FDCWD passes through untouched.
        SYS_OPENAT => row!(F, [Fd, Path, Scalar, Scalar]),
        // fstatat64(int dirfd, const char *path, struct stat *buf, int flag): a dirfd like
        // openat's; buf is the fixed 144-byte struct (sys/stat.h).
        SYS_FSTATAT64 => row!(P, [Fd, Path, Ptr, Scalar]),
        // shm_open(const char *name, int oflag, mode_t mode) → a NEW descriptor: guest fds are not
        // files-only.
        SYS_SHM_OPEN => row!(F, [Path, Scalar, Scalar]),
        // ---- sysctl -------------------------------------------------------------------------
        // sysctl(int *name, u_int namelen, void *oldp, size_t *oldlenp, void *newp, size_t newlen)
        // name: namelen ints, and the kernel rejects namelen > CTL_MAXNAME (12, sys/sysctl.h).
        // oldp: the destination is x2 and its length is `*(size_t*)x3`, in guest memory rather
        // than a register — measured via /bin/ps, whose KERN_PROC_ALL buffer runs far past the
        // 64 KiB window (M26). oldlenp: 8 bytes in-out. newp: read for newlen bytes, and the
        // bound is the handler's (rule 5). Census 2026-09-12: 7 of 8 distinct (newp, newlen) rows
        // passed a non-null newp, and M33 t5 measured every one of the 7 (jq, CPython, /bin/ps,
        // ls, date, sh, zsh, sort, sleep, hostname): all are MIB `{0, 3}` — libc's `name2oid`
        // idiom, the name string in newp — with newlen 10..=32, and that handler rejects
        // `newlen >= MAXPATHLEN` with ENAMETOOLONG (bsd/kern/kern_newsysctl.c
        // `sysctl_sysctl_name2oid`, "XXX arbitrary, undocumented"). For any other MIB the
        // built-in handlers `sysctl_root` dispatches to read exactly the oid's own size
        // (`sysctl_io_number`: `SYSCTL_IN(req, pValue, valueSize)`; `sysctl_io_opaque` likewise;
        // `sysctl_io_string` rejects `newlen >= valueSize`) — the general bound. So Ptr, and the
        // KERN_PROC_ALL `Dest` keeps its canary; the day a corpus guest passes newp to a custom
        // handler with no such check is the day this flips to Source under rule 6 (spec §4c).
        SYS_SYSCTL => row!(P, [Ptr, Scalar, Dest(DerefU64(3)), Ptr, Ptr, Scalar]),
        // sysctlbyname — the RAW syscall's shape, not libc's 5-arg wrapper. `sysctlbyname(3)`'s C
        // signature is (name, oldp, oldlenp, newp, newlen), but the kernel entry point behind it
        // takes an extra `namelen` first, exactly like `SYS_SYSCTL`: (name, namelen, oldp, oldlenp,
        // newp, newlen). Measured directly against the live kernel (M29 fix round 1) with a raw
        // `syscall(274, ...)` bypassing libc's wrapper: the 6-arg form on `"kern.ostype"` returns 0
        // with `oldp` filled (`"Darwin"`, `*oldlenp` 7); the naive 5-arg reading of the libc
        // prototype (`oldp` at index 1) returns -1. So `oldp` is index 2 and `oldlenp` index 3 —
        // IDENTICAL to `SYS_SYSCTL`, not "one index lower" as this entry previously (and wrongly)
        // claimed. That wrong claim was never caught here: nothing in the M29 measurement corpus
        // ever dispatched syscall 274 (see the M29 report's Finding B), so a real `sysctlbyname`
        // call would have computed `want` from the first 8 bytes of the destination buffer's own
        // CONTENTS (reading `args[2]`, the actual `oldp`, as if it were `oldlenp`) and treated
        // `namelen` (`args[1]`) as the destination pointer — misdiagnosing a legal call as
        // `[M29 DEREFLEN-UNBACKED]` rather than measuring it. The two rows share one shape because
        // both use the same indices, not despite them differing by one. name: a string of namelen
        // bytes, and the kernel rejects `namelen >= MAXPATHLEN` with ENAMETOOLONG before its
        // copyin (bsd/kern/kern_newsysctl.c `sys_sysctlbyname`) — the cited bound, so Ptr. newp:
        // the same per-handler bound as SYS_SYSCTL's, reached through the same `sysctl_root`.
        // Census 2026-09-12: 274 is dispatched by no corpus guest (0 calls), so both are header
        // truth.
        SYS_SYSCTLBYNAME => row!(P, [Ptr, Scalar, Dest(DerefU64(3)), Ptr, Ptr, Scalar]),
        // getfsstat64(struct statfs *buf, int bufsize, int flags): destination x0, length x1 in
        // BYTES rather than a mount count — the reason this entry is easy to get wrong, since a
        // mount count would be small enough never to matter. It does NOT currently overrun on this
        // machine, and an earlier version of this comment claimed it did. MEASURED at M29 (Task 7)
        // against the live system: `sizeof(struct statfs)` is 2168 bytes and this machine reports
        // 16 mounts, so a full reply is 34,688 bytes and it takes 31 mounts to cross a 65536-byte
        // window. The row is here because the call is structurally able to cross that cap on a
        // machine with more mounts, not because it has been seen to.
        SYS_GETFSSTAT64 => row!(P, [Dest(Reg(1)), Scalar, Scalar]),
        // ---- M33 census rows ------------------------------------------------------------------
        // Every number below is in `tests/census.rs` and had no row before M33 Task 5. Prototypes
        // are the KERNEL's, from xnu-12377.1.9 `bsd/kern/syscalls.master` (the SDK's `unistd.h` and
        // friends describe libc's wrappers, which differ — `gettid` takes two out-pointers,
        // `gettimeofday` a third, `bsdthread_register` seven arguments), so each comment names the
        // xnu file its bound comes from. A row for a call serviced or emulated above the trace is
        // documentation: it never reaches `forwarded_shape`.
        //
        // ---- process / identity ---------------------------------------------------------------
        // exit(int rval): serviced above the trace (record's SYS_EXIT arm).
        SYS_EXIT => row!(P, [Scalar]),
        // getpid(void) / getuid(24) / geteuid(25) / getppid(39) / getegid(43) / getgid(47) /
        // getpgrp(81) / issetugid(327) / thread_selfid(void) → uint64_t / sync(void)
        SYS_GETPID | 24 | 25 | 39 | 43 | 47 | 81 | 327 | SYS_THREAD_SELFID | 36 => row!(P, []),
        // gettid(uid_t *uidp, gid_t *gidp): the SDK declares no `gettid`; the kernel's prototype
        // (syscalls.master) takes two out-pointers and writes 4 bytes through each (`suword`,
        // bsd/kern/kern_prot.c `gettid`) — not the `(void)` the calibration list assumed.
        286 => row!(P, [Ptr, Ptr]),
        // umask(int newmask)
        60 => row!(P, [Scalar]),
        // crossarch_trap(uint32_t name): bsd/kern/kern_crossarch.c `sys_crossarch_trap` — returns
        // EINVAL or ENOTSUP, touches no memory. Issued by every dynamic guest (libSystem init).
        38 => row!(P, [Scalar]),
        // getlogin(char *namebuf, u_int namelen): the kernel clamps namelen to MAXLOGNAME (255,
        // sys/param.h) before its copyout — bsd/kern/kern_prot.c `getlogin`.
        49 => row!(P, [Ptr, Scalar]),
        // gettimeofday(struct timeval *tp, struct timezone *tzp, uint64_t *mach_absolute_time):
        // the kernel prototype has the third, xnu-private out-pointer libsyscall passes; 16, 8
        // and 8 bytes (bsd/kern/kern_time.c `gettimeofday`).
        116 => row!(P, [Ptr, Ptr, Ptr]),
        // getrusage(int who, struct rusage *rusage): a fixed 144-byte struct (sizeof, measured
        // against the SDK; bsd/kern/kern_resource.c `getrusage` copies out `user64_rusage`).
        117 => row!(P, [Scalar, Ptr]),
        // getrlimit(u_int which, struct rlimit *rlp) / setrlimit: a fixed 16-byte struct, out for
        // 194 and in for 195 (bsd/kern/kern_resource.c, `copyin(uap->rlp, …, sizeof(struct
        // rlimit))`). getrlimit is serviced above the trace for RLIMIT_STACK (M8), forwarded
        // otherwise.
        SYS_GETRLIMIT | 195 => row!(P, [Scalar, Ptr]),
        // getentropy(void *buffer, size_t size): the kernel rejects size > 256 with EINVAL
        // (bsd/dev/random/randomdev.c `getentropy`, `char buffer[256]`) — the cited bound.
        500 => row!(P, [Ptr, Scalar]),
        // ---- signals (serviced above the trace, M11/M12/M16 — rows are documentation) -----------
        // kill(int pid, int signum, int posix): the kernel's third argument, which libc's stub
        // passes as 1. Serviced above the trace (M11).
        SYS_KILL => row!(P, [Scalar, Scalar, Scalar]),
        // sigaction(int signum, struct __sigaction *nsa, struct sigaction *osa): a 24-byte struct
        // in (handler, trampoline, mask, flags) and a 16-byte struct out (sys/signal.h, sizeof).
        // The handler and trampoline are code addresses the kernel records and never follows.
        SYS_SIGACTION => row!(P, [Scalar, Ptr, Ptr]),
        // sigprocmask(int how, sigset_t *mask, sigset_t *omask): 4 bytes each (sigset_t is a
        // uint32_t on Darwin).
        SYS_SIGPROCMASK => row!(P, [Scalar, Ptr, Ptr]),
        // sigpending(sigset_t *osv): 4 bytes out.
        SYS_SIGPENDING => row!(P, [Ptr]),
        // sigaltstack(const stack_t *nss, stack_t *oss): a fixed 24-byte struct each (sizeof).
        SYS_SIGALTSTACK => row!(P, [Ptr, Ptr]),
        // sigreturn(struct ucontext *uctx, int infostyle, user_addr_t token): the kernel copies in
        // the 56-byte ucontext and then FOLLOWS `uc_mcontext64` for the 816-byte mcontext
        // (bsd/dev/arm/unix_signal.c `sigreturn_copyin_ctx64`) — rule 1, a nested read, so
        // NestedSource even though both pieces are fixed-size. Serviced above the trace (M12) and
        // asserted off the forward path by `is_signal_syscall`, so the view is documentation
        // (EXPECTED_DIFFS). `token` is a value the kernel compares, not a pointer it follows.
        SYS_SIGRETURN => row!(P, [NestedSource, Scalar, Scalar]),
        // __pthread_kill(int thread_port, int sig): xnu-private, serviced above the trace (M16).
        SYS_PTHREAD_KILL => row!(P, [Scalar, Scalar]),
        // __pthread_sigmask(int how, sigset_t *set, sigset_t *oset): xnu-private, 4 bytes each;
        // serviced above the trace (M16).
        SYS_PTHREAD_SIGMASK => row!(P, [Scalar, Ptr, Ptr]),
        // __disable_threadsignal(int value): xnu-private, one scalar. Forwarded today — it is
        // not in `is_signal_syscall` — so the kernel applies it to RETRACE's thread rather than
        // the guest's; the exiting guest thread that issues it never observes the difference.
        // Noted here, not modelled.
        331 => row!(P, [Scalar]),
        // ---- threads / workqueue (emulated above the trace, M14/M18 — rows are documentation) ---
        // bsdthread_create(func, func_arg, stack, pthread, flags): xnu-private, shape per
        // SYS_BSDTHREAD_CREATE's doc. func/func_arg/stack are values handed to the new thread's
        // registers; `pthread` is written at a fixed offset (the kernel port, 4 bytes, at
        // pthread + tsd_offset + mach_thread_self_offset — libpthread kern/kern_support.c
        // `_bsdthread_create`; measured at +0xf8, M14). Emulated (M14); forwarding is asserted
        // against in retrace-core because it is whole-process fatal.
        SYS_BSDTHREAD_CREATE => row!(P, [Scalar, Scalar, Scalar, Ptr, Scalar]),
        // bsdthread_terminate(stackaddr, freesize, port, sema_or_ulock): xnu-private, shape per
        // SYS_BSDTHREAD_TERMINATE's doc. stackaddr is a VM range to deallocate, not data;
        // sema_or_ulock a port name or wait-queue key — no user memory is read or written
        // (libpthread `_bsdthread_terminate`). Emulated (M14).
        SYS_BSDTHREAD_TERMINATE => row!(P, [Scalar, Scalar, Scalar, Scalar]),
        // bsdthread_register(threadstart, wqthread, flags, pthread_init_data, pthread_init_data_
        // size, dispatchqueue_offset, tsd_offset): xnu-private; syscalls.master names SEVEN
        // arguments, not the six of the older `(…, pthsize, dummy, targetconc, dispatchqueue_off)`
        // reading. threadstart/wqthread are code addresses the kernel records and never follows;
        // pthread_init_data is a `struct _pthread_registration_data` copied in AND out for
        // MIN(sizeof(data), size) bytes (libpthread `_bsdthread_register`) — a fixed struct.
        // Emulated since M18 Stage 1.
        SYS_BSDTHREAD_REGISTER => row!(P, [Scalar, Scalar, Scalar, Ptr, Scalar, Scalar, Scalar]),
        // workq_open(void): emulated (M18 Stage 2a).
        SYS_WORKQ_OPEN => row!(P, []),
        // workq_kernreturn(int options, user_addr_t item, int affinity, int prio): `item` is
        // opcode-dependent — a count for REQTHREADS, a `struct workq_dispatch_config` copied in
        // for MIN(sizeof(cfg), affinity) bytes for SETUP_DISPATCH (bsd/pthread/pthread_workqueue.c
        // `workq_kernreturn`). Emulated (M18); the box refuses unmeasured opcodes by value.
        SYS_WORKQ_KERNRETURN => row!(P, [Scalar, Ptr, Scalar, Scalar]),
        // bsdthread_ctl(user_addr_t cmd, arg1, arg2, arg3): xnu-private, cmd-dependent
        // (bsd/pthread/pthread_workqueue.c `bsdthread_ctl`). arg1/arg2 are port names, priorities
        // or resource keys the kernel never dereferences; arg3 is, for QOS_OVERRIDE_DISPATCH
        // (`workq_thread_add_dispatch_override`), a ulock address read with a 4-byte
        // `copyin_atomic32` — the only memory access in the switch, so Ptr with that bound. (For
        // QOS_OVERRIDE_START arg3 is a `resource` key, never dereferenced.) Forwarded; the census
        // saw it from one guest (`panicky`).
        478 => row!(P, [Scalar, Scalar, Scalar, Ptr]),
        // __ulock_wait(uint32_t operation, void *addr, uint64_t value, uint32_t timeout):
        // xnu-private, shape per SYS_ULOCK_WAIT's doc. The kernel reads 4 or 8 bytes at addr
        // (`copyin_atomic32`/`_atomic64`, bsd/kern/sys_ulock.c) to compare against `value`.
        // Emulated (M14).
        SYS_ULOCK_WAIT => row!(P, [Scalar, Ptr, Scalar, Scalar]),
        // __ulock_wake(uint32_t operation, void *addr, uint64_t wake_value): xnu-private, shape
        // per SYS_ULOCK_WAKE's doc. addr is a wait-queue KEY (`ulock_wake`, bsd/kern/sys_ulock.c
        // — no copyin), so Scalar. Emulated (M14).
        SYS_ULOCK_WAKE => row!(P, [Scalar, Scalar, Scalar]),
        // ---- paths (the kernel stops at PATH_MAX) ---------------------------------------------
        // access(const char *path, int flags)
        33 => row!(P, [Path, Scalar]),
        // pathconf(const char *path, int name)
        191 => row!(P, [Path, Scalar]),
        // readlink(const char *path, char *buf, int count): buf is a destination of `count`
        // bytes, but what the kernel writes is the link's stored target, which `symlink` itself
        // capped at MAXPATHLEN (1024) on the way in (bsd/vfs/vfs_syscalls.c `symlinkat_internal`,
        // `copyinstr(path_data, path, MAXPATHLEN, …)`) — the cited bound, far inside the window.
        58 => row!(P, [Path, Ptr, Scalar]),
        // stat64(const char *path, struct stat *ub) / lstat64: a fixed 144-byte struct (sys/stat.h).
        338 | 340 => row!(P, [Path, Ptr]),
        // getattrlist(const char *path, struct attrlist *alist, void *attributeBuffer, size_t
        //             bufferSize, u_long options): alist is a fixed 24-byte struct (sys/attr.h,
        // sizeof); attributeBuffer is a destination of bufferSize bytes — M34's row to widen
        // (spec §7), Ptr until measured, exactly like its `fgetattrlist` sibling.
        220 => row!(P, [Path, Ptr, Ptr, Scalar, Scalar]),
        // fsgetpath(char *buf, size_t bufsize, fsid_t *fsid, uint64_t objid): the kernel rejects
        // bufsize > MAXLONGPATHLEN (8192) with EINVAL before writing (bsd/vfs/vfs_syscalls.c
        // `fsgetpath_extended`) — the cited bound; fsid is 8 bytes copied in (`sizeof(fsid_t)`),
        // and names a VOLUME, not a descriptor.
        427 => row!(P, [Ptr, Scalar, Ptr, Scalar]),
        // execve(char *fname, char **argp, char **envp): the kernel reads every argv/envp string
        // through the nested pointers — rule 1, NestedSource (EXPECTED_DIFFS; exercised by /bin/sh).
        // Forwarded today, and it fails only because those guest pointers EFAULT in retrace's
        // process: a forwarded exec that SUCCEEDED would replace retrace's own process image. The
        // fail-loud assert that precedent (`bsdthread_create`) demands is owed to a successor —
        // adding it re-parks cpython_e2e's launcher test, the operator's call, not this row's.
        59 => row!(P, [Path, NestedSource, NestedSource]),
        // ---- descriptors ----------------------------------------------------------------------
        // fchdir(int fd): a descriptor the legacy fd table never translated (EXPECTED_DIFFS;
        // exercised by /bin/ls). Forwarded, and the descriptor now names the guest's file — but
        // a forwarded fchdir changes retrace's OWN working directory, the same recorder-side
        // effect the `__disable_threadsignal` (331) row above names. Noted, not modelled.
        13 => row!(P, [Fd]),
        // pipe(void) → TWO new descriptors, in x0 and x1 (bsd/kern/sys_pipe.c `pipe`, `retval[0]`
        // and `retval[1]`). See Ret::FdPair for why the return is unmodelled.
        42 => row!(Ret::FdPair, []),
        // kqueue(void) → a NEW descriptor (bsd/kern/kern_event.c `kqueue`). Bound like open's
        // (EXPECTED_DIFFS; exercised by /bin/wait4path). No kevent spelling (363/369/374/375) is
        // in the census, so nothing yet consumes the bound slot.
        362 => row!(F, []),
        // ---- memory ---------------------------------------------------------------------------
        // munmap(void *addr, size_t len) / mprotect(addr, len, prot): emulated above the trace
        // (record's SYS_MUNMAP / SYS_MPROTECT arms). addr is a VM range, not data.
        SYS_MUNMAP => row!(P, [Scalar, Scalar]),
        SYS_MPROTECT => row!(P, [Scalar, Scalar, Scalar]),
        // madvise(void *addr, size_t len, int behav): addr is a VM range the kernel neither reads
        // nor writes as data. Forwarded — `host_span` rebases it onto the guest backing.
        75 => row!(P, [Scalar, Scalar, Scalar]),
        // shared_region_check_np(uint64_t *start_address): 8 bytes out — serviced above the
        // trace (forced to fail so dyld maps the cache itself).
        SYS_SHARED_REGION_CHECK_NP => row!(P, [Ptr]),
        // map_with_linking_np(const struct mwl_region regions[], uint32_t region_count,
        //                     const struct mwl_info_hdr *link_info, uint32_t link_info_size)
        // (mach/dyld_pager.h `__map_with_linking_np`). regions: read for region_count × 32 bytes,
        // region_count > MWL_MAX_REGION_COUNT (5) rejected (bsd/vm/vm_unix.c
        // `map_with_linking_np`) — Ptr on that bound; the descriptor INSIDE regions[i].mwlr_fd is
        // translated by `translate_mwl_regions`, which no operand index can name. link_info: read
        // for link_info_size bytes, and the only kernel cap is MWL_MAX_LINK_INFO_SIZE = 64 MiB
        // (osfmk/vm/vm_dyld_pager.h, "just a guess for now") — a caller-chosen length with no cap
        // below the window, so rule 4: Source (EXPECTED_DIFFS). The blob carries offsets, not
        // pointers, so it is flat, not nested. MEASURED M33 t5 (jq, CPython, /bin/ps, ls, date,
        // sh, zsh, sort, sleep, hostname): 11 calls, link_info_size 80..=2920 bytes — far inside
        // the window today, so the canary this listing withholds could not have landed inside
        // any measured blob; the kind is the contract, not the measurement, and the call has no
        // destination, so withholding the canary costs nothing (EXPECTED_DIFFS).
        SYS_MAP_WITH_LINKING_NP => row!(P, [Ptr, Scalar, Source, Scalar]),
        // ---- code signing / policy -------------------------------------------------------------
        // csops(pid_t pid, uint32_t ops, void *useraddr, size_t usersize) / csops_audittoken(…,
        // audit_token_t *uaudittoken): useraddr is op-dependent — a 4-byte status word for
        // CS_OPS_STATUS, a hash, or a blob copied out for usersize bytes (`csops_copy_token`,
        // bsd/kern/kern_proc.c `csops_internal`) — M34's destination to widen; Ptr until measured.
        // The audit token is a fixed 32-byte copyin.
        169 => row!(P, [Scalar, Scalar, Ptr, Scalar]),
        170 => row!(P, [Scalar, Scalar, Ptr, Scalar, Ptr]),
        // csrctl(uint32_t op, void *useraddr, size_t usersize): both ops reject usersize !=
        // sizeof(csr_config_t) (4) with EINVAL (bsd/kern/kern_csr.c `syscall_csr_check` /
        // `syscall_csr_get_active_config`) — the cited bound.
        483 => row!(P, [Scalar, Ptr, Scalar]),
        // __mac_syscall(char *policy, int call, void *arg): policy is `copyinstr`'d into a
        // MAC_MAX_POLICY_NAME (32) buffer (security/mac_base.c `__mac_syscall`) — NUL-terminated
        // and kernel-stopped, so Path. arg: xnu itself never copies it — it hands the raw pointer
        // to the policy's `mpo_policy_syscall(p, call, arg)`, and Sandbox.kext (closed) reads a
        // per-`call` struct of ITS choosing. No argument carries a length, so rule 4's
        // precondition (a caller-chosen length that can cross the window) is structurally
        // absent; the size is the callee's, fixed per call — Ptr, on that reasoning rather than
        // on a number nobody outside Apple can cite.
        381 => row!(P, [Path, Scalar, Ptr]),
        // MAC_SYSCALL_MAGIC (0x8000_0000): not a syscall number. dyld's inline
        // `__mac_syscall("Sandbox", …)` loads this magic into x16 (`movz x16, #0x8000, lsl #16`);
        // only a platform binary may issue it, so retrace-core synthesizes the reply and never
        // forwards it (its `MAC_SYSCALL_MAGIC` arm). The argument shape is __mac_syscall's.
        0x8000_0000 => row!(P, [Path, Scalar, Ptr]),
        // proc_info(int32_t callnum, int32_t pid, uint32_t flavor, uint64_t arg, void *buffer,
        //           int32_t buffersize): buffer is M34's destination to widen; Ptr until measured.
        336 => row!(P, [Scalar, Scalar, Scalar, Scalar, Ptr, Scalar]),
        // task_read_for_pid(mach_port_name_t target_tport, int pid, mach_port_name_t *t): 4 bytes
        // out (bsd/kern/kern_proc.c `task_read_for_pid`, `copyout(…, sizeof(mach_port_name_t))`).
        539 => row!(P, [Scalar, Scalar, Ptr]),
        // ---- spawn ----------------------------------------------------------------------------
        // posix_spawn(pid_t *pid, const char *path, const struct _posix_spawn_args_desc *adesc,
        //             char **argv, char **envp): pid is 4 bytes out. adesc is read as a fixed
        // struct and then the kernel follows attrp, file_actions, port_actions, persona_info and
        // more INSIDE it (bsd/kern/kern_exec.c `posix_spawn`, one `copyin` per member) — rule 1,
        // NestedSource, like argv/envp's strings. Forwarded today — the launcher-shim gap
        // cpython_e2e pins (EXPECTED_DIFFS) — and it fails only because those guest pointers
        // EFAULT in retrace's process; a forwarded spawn that succeeded would start a real child
        // of retrace. The fail-loud assert precedent demands is owed to a successor, since adding
        // it re-parks cpython_e2e's launcher test — the operator's decision.
        244 => row!(P, [Ptr, Path, NestedSource, NestedSource, NestedSource]),
        // ---- mach traps (numbers per xnu osfmk/mach/syscall_sw.h; see the constants) ------------
        // _kernelrpc_mach_vm_allocate_trap(target, mach_vm_offset_t *addr, size, flags): 8 bytes
        // in-out (osfmk/ipc/mach_kernelrpc.c). Serviced above the trace (M2: acts on guest IPA).
        MACH_VM_ALLOCATE_TRAP => row!(P, [Scalar, Ptr, Scalar, Scalar]),
        // _kernelrpc_mach_vm_deallocate_trap(target, address, size): serviced above the trace (M2).
        MACH_VM_DEALLOCATE_TRAP => row!(P, [Scalar, Scalar, Scalar]),
        // _kernelrpc_mach_vm_protect_trap(target, address, size, set_maximum, new_protection):
        // serviced above the trace (M2).
        MACH_VM_PROTECT_TRAP => row!(P, [Scalar, Scalar, Scalar, Scalar, Scalar]),
        // _kernelrpc_mach_vm_map_trap(target, mach_vm_offset_t *address, size, mask, flags, prot):
        // 8 bytes in-out. Serviced above the trace (M2).
        MACH_VM_MAP_TRAP => row!(P, [Scalar, Ptr, Scalar, Scalar, Scalar, Scalar]),
        // _kernelrpc_mach_port_deallocate_trap(target, name) /
        // _kernelrpc_mach_port_mod_refs_trap(target, name, right, delta): names and counts.
        MACH_PORT_DEALLOCATE_TRAP => row!(P, [Scalar, Scalar]),
        MACH_PORT_MOD_REFS_TRAP => row!(P, [Scalar, Scalar, Scalar, Scalar]),
        // _kernelrpc_mach_port_construct_trap(target, mach_port_options_t *options, context,
        //                                     mach_port_name_t *name): a fixed 24-byte options
        // struct in, 4 bytes out (osfmk/ipc/mach_kernelrpc.c, `mach_copyin(…, sizeof(options))`).
        MACH_PORT_CONSTRUCT_TRAP => row!(P, [Scalar, Ptr, Scalar, Ptr]),
        // mach_reply_port() / thread_self_trap() / task_self_trap() / host_self_trap() /
        // thread_get_special_reply_port(): the result is a port name in x0.
        MACH_REPLY_PORT_TRAP | MACH_THREAD_SELF_TRAP | MACH_TASK_SELF_TRAP | MACH_HOST_SELF_TRAP
        | MACH_THREAD_GET_SPECIAL_REPLY_PORT_TRAP => row!(P, []),
        // semaphore_wait_trap(name) / semaphore_signal_trap(name): a port name. Serviced above the
        // trace (M18 Stage 2b); forwarding either hangs the recorder (see their constants).
        MACH_SEMAPHORE_WAIT | MACH_SEMAPHORE_SIGNAL => row!(P, [Scalar]),
        // host_create_mach_voucher_trap(host, mach_voucher_attr_raw_recipe_array_t recipes,
        //   int recipes_size, mach_port_name_t *voucher): recipes is read for recipes_size, which
        // the kernel rejects above MACH_VOUCHER_ATTR_MAX_RAW_RECIPE_ARRAY_SIZE (5120,
        // mach/mach_voucher_types.h; osfmk/ipc/mach_kernelrpc.c `host_create_mach_voucher_trap`)
        // — the cited bound; voucher is 4 bytes out.
        MACH_HOST_CREATE_MACH_VOUCHER_TRAP => row!(P, [Scalar, Ptr, Scalar, Ptr]),
        // mach_timebase_info_trap(mach_timebase_info_t info): 8 bytes out (osfmk/kern/clock.c).
        // Forwarded — retrace-core has no arm for it; the numer/denom pair is a constant of the
        // machine and lands in the trace as `writes`. (The SYNTHETIC timebase is the CNTVCT read
        // `Box_::run()` emulates, a different thing.)
        MACH_TIMEBASE_INFO_TRAP => row!(P, [Ptr]),
        _ => None,
    }
}

/// The shape of a syscall about to be FORWARDED — loud on an unenumerated one.
///
/// `translate_fds` calls this first, and `translate_fds` is the first statement of
/// `forward_and_diff`, so no syscall reaches the host kernel through the generic forward path
/// without a row: the M10 class ("a forgotten entry forwards a raw guest fd, silently") is
/// structurally closed. Every other view consulted inside `forward_and_diff` is downstream of
/// this check. Record-only by construction — replay never forwards.
pub fn forwarded_shape(num: u64) -> &'static Shape {
    arg_kinds(num).unwrap_or_else(|| panic!(
        "M33: syscall {num} ({}) has no arg_kinds row in crates/retrace-arch/src/lib.rs — it \
         cannot be forwarded unclassified (an untranslated guest fd would act on retrace's own \
         descriptor of that number). Classify each argument from the SDK prototype under the \
         rules in ArgKind's docs and add the row; if a guest in the corpora dispatches it, add the \
         number to tests/census.rs too.", num as i64))
}

/// Which operand indices of `num` hold a GUEST file descriptor. View over `arg_kinds`; empty for
/// an unenumerated syscall (the loud check is `forwarded_shape`, upstream of every caller in the
/// forward path).
pub fn fd_operands(num: u64) -> impl Iterator<Item = usize> {
    arg_kinds(num).into_iter().flat_map(Shape::fd_operands)
}
/// Does `num`'s RETURN value need binding to a fresh guest fd slot? View over `arg_kinds`.
pub fn allocates_fd(num: u64) -> bool { arg_kinds(num).is_some_and(Shape::allocates_fd) }
/// The destination buffer `num` fills, as `(argument index, where its length lives)`. View.
pub fn dest_buffer(num: u64) -> Option<(usize, DestLen)> { arg_kinds(num)?.dest_buffer() }
/// Refused-by-value family: a destination behind a nested guest pointer. View.
pub fn writes_via_nested_pointer(num: u64) -> bool {
    arg_kinds(num).is_some_and(Shape::writes_via_nested_pointer)
}
/// Does the host kernel READ guest memory through this call, in an amount no window bounds? View.
pub fn reads_guest_buffer(num: u64) -> bool { arg_kinds(num).is_some_and(Shape::reads_guest_buffer) }

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
pub const CPU_SUBTYPE_ARM64_ALL: u32 = 0;
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
/// `bsdthread_register(threadstart, wqthread, flags, pthread_init_data, pthread_init_data_size,
/// dispatchqueue_offset, tsd_offset)` — seven arguments per xnu `syscalls.master`, with `flags`
/// at index 2 (not the older six-argument `(…, pthsize, dummy, targetconc, dispatchqueue_off)`
/// reading; see its `arg_kinds` row). Already fires on EVERY dynamic guest since M7, unremarked;
/// `threadstart` is the address a new thread must be entered at.
pub const SYS_BSDTHREAD_REGISTER: u64 = 366;
/// `workq_open()` — brings up the process's kernel workqueue. **Never forwarded** (M18 Stage 2a):
/// the host would bring up a real workqueue for RETRACE's own process, and the `REQTHREADS` that
/// follows would then start a real worker thread inside the recorder, which jumps to address 0.
/// `Box_::guest_workq_open` emulates it. It first fired once M18 Stage 1 stopped forwarding
/// `bsdthread_register` — before that it had never fired at all, and the doc here said so.
/// Measured order is `kernreturn(0x400)` -> `open` -> `kernreturn(0x20)`, so a `workq_kernreturn`
/// precedes the `workq_open` (M18 Task 6, `stage2-measurements.md` §2 — Stage 1's measurement, which
/// is an UNTRACKED working-tree artifact under `.superpowers/sdd/2026-08-20-retrace-m18-workq/` and
/// so is absent from a fresh clone; Stage 2b's, cited below, was relocated into `docs/` for exactly
/// that reason. The same order is restated in `docs/status-log.md`'s M18 Stage 1 section, which is
/// tracked).
pub const SYS_WORKQ_OPEN: u64 = 367;
/// `workq_kernreturn(options, item, affinity, prio)` — the workqueue's whole control surface: a
/// worker parks, returns, and is dispatched through it. **Never forwarded** for the reason
/// `SYS_WORKQ_OPEN` gives; `Box_::guest_workq_kernreturn` dispatches on `args[0]` and refuses by
/// value every operation word no run has measured. Two have: `0x400` (dispatch setup) and `0x20`
/// (request threads), each once per run, measured M18 Task 6 and re-measured with a permissive
/// REQTHREADS stub in Task 4 without a third appearing
/// (`docs/superpowers/specs/2026-08-21-retrace-m18-stage2b-measurements.md` §5). That is a floor,
/// not a ceiling: the park/return opcodes a *running* worker issues cannot be enumerated until
/// Stage 2b makes one run.
pub const SYS_WORKQ_KERNRETURN: u64 = 368;
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

// ---- Mach traps ---------------------------------------------------------------------------
// Mach traps arrive as `svc #0x80` with a NEGATIVE selector in x16, so these are stored as their
// two's-complement `u64` — the form `Stop::Syscall { num, .. }` actually carries, and the form
// `retrace-core`'s own `MACH_MSG2`/`MACH_TASK_SELF` already use.

/// The mach semaphore **wait** trap, `-36`. `dispatch_semaphore_wait`'s slow path reaches it via
/// `_dispatch_sema4_wait` -> `semaphore_wait` -> `b _semaphore_wait_trap`.
///
/// M18 Stage 2b Task 1 §3a **CONFIRMED** the predecessor document's prediction: it is not merely
/// attributed any more, it is read off libsystem_kernel's own stub on this machine
/// (`_semaphore_wait_trap: mov x16, #-0x24; svc #0x80`), with `_mach_msg2_trap`'s `#-0x2f` in the
/// same table cross-checking the numbering against the existing `MACH_MSG2 = -47`. Stage 2a
/// independently observed the trap itself, twice, at `pc=0x1804adbb0` carrying the port name its
/// preceding `semaphore_create` minted.
///
/// **Never forwarded.** Forwarding blocks retrace's OWN process forever on a semaphore only the
/// guest's worker could signal — measured, not theorised: both Stage 2a measurement runs hung
/// there and produced zero bytes of guest stdout. Its return value must be `0`; `0xe` makes
/// libdispatch re-issue the trap forever and everything else is fatal (§3).
pub const MACH_SEMAPHORE_WAIT: u64 = (-36i64) as u64;

/// The mach semaphore **signal** trap, `-33` — the wake half of the pair, reached from
/// `_dispatch_sema4_signal`. Task 1 §3a CONFIRMED this prediction the same way, from
/// `_semaphore_signal_trap: mov x16, #-0x21; svc #0x80`.
///
/// Two things about it are deliberately kept distinct. It has been **verified as the instruction
/// libdispatch reaches**, but it has still **never been observed in a retrace trace** — no run has
/// got far enough to execute it. And `dispatch_semaphore_signal`'s FAST path issues no trap at all
/// (`ldaddl` on `sem+0x30`, §3c), so a worker signalling a semaphore nobody is waiting on produces
/// no landmark: the park/wake seam must not assume a signal trap always appears. Return `0`.
pub const MACH_SEMAPHORE_SIGNAL: u64 = (-33i64) as u64;

/// Mach traps the corpora dispatch (M33 census, `tests/census.rs`), keyed as the two's-complement
/// `u64` the trap carries. Numbers per xnu `osfmk/mach/syscall_sw.h` (xnu-12377.1.9) — the SDK
/// does not ship that header — cross-checked against retrace-core's private `MACH_VM_ALLOCATE`
/// (-10) / `_DEALLOCATE` (-12) / `_PROTECT` (-14) / `_MAP` (-15) / `MACH_TASK_SELF` (-28). They
/// exist so `arg_kinds` can match them by name: a cast is not a pattern (`(-10i64) as u64 => …`
/// does not compile).
pub const MACH_VM_ALLOCATE_TRAP: u64 = (-10i64) as u64;
pub const MACH_VM_DEALLOCATE_TRAP: u64 = (-12i64) as u64;
pub const MACH_VM_PROTECT_TRAP: u64 = (-14i64) as u64;
pub const MACH_VM_MAP_TRAP: u64 = (-15i64) as u64;
pub const MACH_PORT_DEALLOCATE_TRAP: u64 = (-18i64) as u64;
pub const MACH_PORT_MOD_REFS_TRAP: u64 = (-19i64) as u64;
pub const MACH_PORT_CONSTRUCT_TRAP: u64 = (-24i64) as u64;
pub const MACH_REPLY_PORT_TRAP: u64 = (-26i64) as u64;
pub const MACH_THREAD_SELF_TRAP: u64 = (-27i64) as u64;
pub const MACH_TASK_SELF_TRAP: u64 = (-28i64) as u64;
pub const MACH_HOST_SELF_TRAP: u64 = (-29i64) as u64;
pub const MACH_THREAD_GET_SPECIAL_REPLY_PORT_TRAP: u64 = (-50i64) as u64;
pub const MACH_HOST_CREATE_MACH_VOUCHER_TRAP: u64 = (-70i64) as u64;
pub const MACH_TIMEBASE_INFO_TRAP: u64 = (-89i64) as u64;

/// Is `num` any of the seven mach semaphore traps? Task 1 §3a read the whole contiguous family off
/// libsystem_kernel's own stubs on this machine, so the bound is measured rather than assumed:
///
/// | selector | stub | | selector | stub |
/// |---|---|---|---|---|
/// | `-33` | `_semaphore_signal_trap` | | `-37` | `_semaphore_wait_signal_trap` |
/// | `-34` | `_semaphore_signal_all_trap` | | `-38` | `_semaphore_timedwait_trap` |
/// | `-35` | `_semaphore_signal_thread_trap` | | `-39` | `_semaphore_timedwait_signal_trap` |
/// | `-36` | `_semaphore_wait_trap` | | | |
///
/// The neighbours pin both ends: `-32` is `_mach_msg_overwrite_trap` and `-40` is not a semaphore
/// stub, so the family is exactly `-39..=-33`.
///
/// **The guard covers all seven, not just the two libdispatch's measured path uses** (fix round 1,
/// finding 5). Only `-36` has ever been observed in a retrace trace and only `-33` is known to be
/// reachable from `_dispatch_sema4_signal`, but every one of the other five would reach
/// `forward_and_diff` and block retrace's own process on a semaphore only the guest could signal —
/// the identical hazard, and a hang is expensive to diagnose precisely because it produces no
/// output to diagnose it with. Guarding by family costs nothing and makes the refusal name the
/// trap that got there.
pub fn is_mach_semaphore_trap(num: u64) -> bool {
    (-39..=-33).contains(&(num as i64))
}

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
                    SYS_CONNECT, SYS_SENDTO, SYS_FGETATTRLIST, SYS_OPENAT, SYS_FSTATAT64,
                    SYS_GETDIRENTRIES64, SYS_FSTATFS64] {
            assert_eq!(fd_operands(num).collect::<Vec<_>>(), [0], "syscall {num} holds its fd in x0");
        }
        assert_eq!(fd_operands(SYS_MMAP).collect::<Vec<_>>(), [4], "mmap's fd is x4, consumed by guest_mmap_file");
        assert_eq!(fd_operands(SYS_DUP2).collect::<Vec<_>>(), [0, 1]);
        // Path-only, fd-free, and fd-RETURNING calls must not have an operand translated.
        for num in [SYS_OPEN, SYS_OPEN_NOCANCEL, SYS_SOCKET, SYS_SHM_OPEN,
                    SYS_EXIT, SYS_MUNMAP, SYS_SYSCTL] {
            assert_eq!(fd_operands(num).count(), 0, "syscall {num} has no fd operand");
        }
        // map_with_linking_np carries its fd INSIDE a guest struct, so it is deliberately absent
        // here — an arg index cannot name it. The box translates it separately.
        assert_eq!(fd_operands(SYS_MAP_WITH_LINKING_NP).count(), 0);
        // fsgetpath(char*, size_t, fsid_t*, uint64_t) (SDK sys/fsgetpath.h:45) takes an fsid_t*
        // identifying a *volume*, not a file descriptor — 427 is deliberately absent from the
        // table (M25-cpython Task 3, Step 1 census).
        assert_eq!(fd_operands(427).count(), 0, "fsgetpath (427) takes no descriptor");
    }

    // M27: 414 was missing from THREE places at once — fd_operands, the forwarded-count clamp, and
    // the diff window — and the missing clamp is the serious one: an unclamped forward lets the
    // host kernel write past the guest buffer's backing. `fd_operands`' own doc comment already
    // states the rule this broke: "A plain-only table fails *silently*."
    #[test]
    fn pread_nocancel_is_treated_exactly_like_pread() {
        assert_eq!(SYS_PREAD_NOCANCEL, 414);
        assert_eq!(fd_operands(SYS_PREAD_NOCANCEL).collect::<Vec<_>>(), fd_operands(SYS_PREAD).collect::<Vec<_>>());
        assert_eq!(dest_buffer(SYS_PREAD_NOCANCEL), dest_buffer(SYS_PREAD));
    }

    // The read family's length is a register. sysctl's is behind a guest pointer — the shape M26's
    // yes/no predicate could not express, and the reason this is a table.
    #[test]
    fn dest_buffer_knows_where_each_length_lives() {
        assert_eq!(dest_buffer(SYS_READ),     Some((1, DestLen::Reg(2))));
        assert_eq!(dest_buffer(SYS_PREAD),    Some((1, DestLen::Reg(2))));
        assert_eq!(dest_buffer(SYS_READ_NOCANCEL), Some((1, DestLen::Reg(2))));
        assert_eq!(dest_buffer(SYS_SYSCTL),   Some((2, DestLen::DerefU64(3))));
    }

    // Absence must mean "provably writes no buffer we can size", never "not gotten to yet".
    // fsgetpath takes an fsid_t* naming a VOLUME, not a descriptor and not a sized buffer; M25
    // pinned that and it must not silently reopen.
    #[test]
    fn dest_buffer_omits_what_it_should() {
        // 427 as a bare literal, matching how `fd_operands`' existing assertion pins it —
        // there is no SYS_FSGETPATH constant in this crate and this test must not invent one.
        assert_eq!(dest_buffer(427), None, "fsgetpath takes an fsid_t*, not a sized buffer");
        assert_eq!(dest_buffer(SYS_WRITE), None, "write reads the buffer, it does not fill it");
    }

    #[test]
    fn dest_buffer_knows_the_m29_additions() {
        // getdirentries64(fd, buf, bufsize, off_t *position) — destination x1, length x2.
        assert_eq!(dest_buffer(SYS_GETDIRENTRIES64), Some((1, DestLen::Reg(2))));
        // getfsstat64(struct statfs64 *buf, int bufsize, int flags) — destination x0, and the
        // length in x1 is BYTES, not a mount count.
        assert_eq!(dest_buffer(SYS_GETFSSTAT64), Some((0, DestLen::Reg(1))));
        // recvfrom(s, buf, len, flags, sockaddr *from, socklen_t *fromlen) — both spellings.
        assert_eq!(dest_buffer(SYS_RECVFROM), Some((1, DestLen::Reg(2))));
        assert_eq!(dest_buffer(SYS_RECVFROM_NOCANCEL), Some((1, DestLen::Reg(2))));
        // sysctlbyname: the RAW syscall's shape (name, namelen, oldp, oldlenp, newp, newlen) —
        // IDENTICAL indices to `SYS_SYSCTL`, measured against the live kernel (M29 fix round 1;
        // see the `dest_buffer` match arm's comment). It was missing from this table AND from the
        // README's list of what was missing.
        assert_eq!(dest_buffer(SYS_SYSCTLBYNAME), Some((2, DestLen::DerefU64(3))));
    }

    #[test]
    fn recvfrom_translates_its_socket_fd() {
        // sendto (133) has been in fd_operands since the fd table landed; recvfrom was not, so a
        // guest receiving on a socket handed the host kernel an untranslated guest fd — the M10
        // class, and the same both-tables-at-once asymmetry M27 found in pread_nocancel.
        assert_eq!(fd_operands(SYS_RECVFROM).collect::<Vec<_>>(), [0]);
        assert_eq!(fd_operands(SYS_RECVFROM_NOCANCEL).collect::<Vec<_>>(), [0]);
        // getfsstat64 and sysctlbyname take no fd, and must NOT have gained one.
        assert_eq!(fd_operands(SYS_GETFSSTAT64).count(), 0);
        assert_eq!(fd_operands(SYS_SYSCTLBYNAME).count(), 0);
    }

    // M27: these put their destination behind a pointer INSIDE a guest struct (iovec.iov_base,
    // msghdr.msg_iov). forward_and_diff translates only top-level register arguments, so a guest
    // IPA would reach the host kernel AS A HOST ADDRESS. That is not a fidelity gap like the
    // truncation class — it is a potential wild write into retrace's own process.
    //
    // The reading that they would merely EFAULT (guest IPAs being unlikely to be mapped in
    // retrace's process) is an INFERENCE, and the downside of it being wrong is severe. So they are
    // refused by value rather than tested or translated, the way guest_workq_kernreturn refuses an
    // unenumerated opcode. Translating them properly needs the translate_mwl_regions treatment and
    // its own measurement.
    #[test]
    fn the_nested_pointer_family_is_pinned_by_number() {
        for num in [120u64, 411, 27, 401, 540, 480] {
            assert!(writes_via_nested_pointer(num), "syscall {num} must be refused");
        }
        for num in [SYS_READ, SYS_PREAD, SYS_SYSCTL, SYS_WRITE] {
            assert!(!writes_via_nested_pointer(num), "syscall {num} has top-level operands");
        }
    }

    // M30 fix round 1. Every number here is from `sys/syscall.h` on this SDK, checked rather than
    // recalled — the whole family is numeric, so a typo would silently unprotect one spelling while
    // its neighbour stayed safe, which is precisely the `_nocancel` trap M9/M10/M27 each hit.
    #[test]
    fn the_guest_buffer_readers_are_pinned_by_number() {
        for num in [SYS_WRITE, SYS_WRITE_NOCANCEL, 154, 415, 121, 412, 541,
                    SYS_SENDTO, 413, 28, 402, 481, 337, 65, 405] {
            assert!(reads_guest_buffer(num), "syscall {num} hands the kernel guest bytes to read");
        }
        assert!(reads_guest_buffer((-47i64) as u64), "mach_msg2_trap reads its message buffer");

        // The destination-side calls must NOT be listed: they are where the canary earns its keep,
        // and silently disabling the fill for them would leave the milestone shipping a detector
        // that never runs on the very syscalls it was built for.
        for num in [SYS_READ, SYS_READ_NOCANCEL, SYS_PREAD, SYS_SYSCTL, SYS_FSTAT,
                    SYS_GETDIRENTRIES64, SYS_RECVFROM, SYS_GETFSSTAT64, SYS_OPEN, SYS_EXIT] {
            assert!(!reads_guest_buffer(num), "syscall {num} is a destination, not a source");
        }
        // `SYS_FSTAT` in particular: `the_canary_catches_zeros_written_over_zeros` and M28's
        // positive control both drive trap 189, so listing it would turn both green for the wrong
        // reason rather than red.
        assert!(!reads_guest_buffer(SYS_FSTAT));
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
        assert_eq!(fd_operands(SYS_READ).collect::<Vec<_>>(), fd_operands(SYS_READ_NOCANCEL).collect::<Vec<_>>());
        assert_eq!(fd_operands(SYS_WRITE).collect::<Vec<_>>(), fd_operands(SYS_WRITE_NOCANCEL).collect::<Vec<_>>());
        assert_eq!(fd_operands(SYS_CLOSE).collect::<Vec<_>>(), fd_operands(SYS_CLOSE_NOCANCEL).collect::<Vec<_>>());
        assert_eq!(fd_operands(SYS_FCNTL).collect::<Vec<_>>(), fd_operands(SYS_FCNTL_NOCANCEL).collect::<Vec<_>>());
        assert_eq!(allocates_fd(SYS_OPEN), allocates_fd(SYS_OPEN_NOCANCEL));
    }

    // M33: the table behind the five views.
    #[test]
    fn arg_kinds_reproduces_the_read_family_shape() {
        use ArgKind::*;
        let s = arg_kinds(SYS_READ).expect("read has a row");
        assert_eq!(s.args, &[Fd, Dest(DestLen::Reg(2)), Scalar]);
        assert_eq!(s.ret, Ret::Plain);
        assert_eq!(arg_kinds(SYS_READ), arg_kinds(SYS_READ_NOCANCEL), "the _nocancel spelling shares the row");
        assert_eq!(arg_kinds(SYS_OPEN).unwrap().ret, Ret::Fd);
    }

    #[test]
    #[should_panic(expected = "has no arg_kinds row")]
    fn an_unenumerated_syscall_panics_by_name() {
        // 8 is the kernel's `nosys` slot (old creat): no syscall lives there, so no row ever will.
        let _ = forwarded_shape(8);
    }

    // `dest_buffer` returns ONE destination and the clamp/window consult it — a row with two
    // `Dest` arguments would silently pick the first. The schema forbids it.
    #[test]
    fn no_row_has_more_than_one_dest_argument_or_more_than_eight_arguments() {
        // The same domain the equivalence sweep walks: BSD numbers, mach traps, and the
        // MAC_SYSCALL_MAGIC band (0x8000_0000 has a row since M33 t5).
        let domain = (0..=1023u64)
            .chain((1..=128i64).map(|n| (-n) as u64))
            .chain(0x8000_0000u64..=0x8000_000f);
        for num in domain {
            if let Some(s) = arg_kinds(num) {
                assert!(s.args.len() <= 8, "syscall {} has {} arguments", num as i64, s.args.len());
                let dests = s.args.iter().filter(|k| matches!(k, ArgKind::Dest(_))).count();
                assert!(dests <= 1, "syscall {} has {dests} Dest arguments", num as i64);
            }
        }
    }

    // M33 t5: the decoded lengths and directions ioctl's row states for the four request codes
    // the census saw (2026-09-12, `ioctl_requests.txt`) are checked facts, not prose — decoded
    // per sys/ioccom.h. The `<= IOCPARM_MASK` bound itself is not asserted here because
    // `iocparm_len` masks with it and the assertion would be tautological; the bound is the
    // decoder's definition. DTRACEHIOC_ADDDOF is the nested one (an 8-byte IN parameter that IS
    // a pointer), and its decode is pinned separately so the row's story stays tied to a number.
    #[test]
    fn ioctl_request_codes_the_corpora_issue_decode_as_the_row_states() {
        const FIODTYPE: u64 = 0x4004_667a;
        const TIOCGWINSZ: u64 = 0x4008_7468;
        const TIOCGETA: u64 = 0x4048_7413;
        const DTRACEHIOC_ADDDOF: u64 = 0x8008_6804;
        // (len, IN, OUT) — the three facts the row's table states per request.
        let decode = |req: u64| (iocparm_len(req), req & IOC_IN != 0, req & IOC_OUT != 0);
        assert_eq!(decode(FIODTYPE), (4, false, true));
        assert_eq!(decode(TIOCGWINSZ), (8, false, true));
        assert_eq!(decode(TIOCGETA), (72, false, true));
        // `_IOW('h', 4, user_addr_t)`: one 8-byte parameter copied IN, group 'h', number 4.
        assert_eq!(decode(DTRACEHIOC_ADDDOF), (8, true, false), "the parameter is one user_addr_t");
        assert_eq!(((DTRACEHIOC_ADDDOF >> 8) & 0xff, DTRACEHIOC_ADDDOF & 0xff), (b'h' as u64, 4));
    }

    // M33 t5: the mach-trap constants `arg_kinds` matches by name, pinned to the selectors xnu's
    // osfmk/mach/syscall_sw.h assigns (read from the fetched header, not from memory) and to the
    // two the retrace-core dispatch already carried privately (-28 task_self, -10 vm_allocate).
    #[test]
    fn mach_trap_constants_match_syscall_sw_h() {
        let expect: [(u64, i64); 14] = [
            (MACH_VM_ALLOCATE_TRAP, -10), (MACH_VM_DEALLOCATE_TRAP, -12), (MACH_VM_PROTECT_TRAP, -14),
            (MACH_VM_MAP_TRAP, -15), (MACH_PORT_DEALLOCATE_TRAP, -18), (MACH_PORT_MOD_REFS_TRAP, -19),
            (MACH_PORT_CONSTRUCT_TRAP, -24), (MACH_REPLY_PORT_TRAP, -26), (MACH_THREAD_SELF_TRAP, -27),
            (MACH_TASK_SELF_TRAP, -28), (MACH_HOST_SELF_TRAP, -29),
            (MACH_THREAD_GET_SPECIAL_REPLY_PORT_TRAP, -50), (MACH_HOST_CREATE_MACH_VOUCHER_TRAP, -70),
            (MACH_TIMEBASE_INFO_TRAP, -89),
        ];
        for (k, n) in expect {
            assert_eq!(k as i64, n);
            assert!(arg_kinds(k).is_some(), "mach trap {n} has no row");
        }
    }

    // M33 t5/t6: pipe's two-descriptor return is expressed (`Ret::FdPair`) but deliberately not
    // bound — `allocates_fd` stays false, matching the legacy table, because binding one of two
    // would alias. Its own test, named for what it checks (Task 5 review, minor 6).
    #[test]
    fn pipe_return_is_a_pair_and_is_not_bound() {
        assert_eq!(arg_kinds(42).unwrap().ret, Ret::FdPair);
        assert!(!allocates_fd(42), "binding one of pipe's two descriptors would alias");
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
    fn m25_syscall_numbers() {
        assert_eq!((SYS_GETDIRENTRIES64, SYS_FSTATFS64), (344, 346));
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
        //
        // SYS_ULOCK_WAKE is here because M14's plan MISLABELED 516 as `__ulock_wait2` (it is
        // `__ulock_wake`; `__ulock_wait2` is 544), and a wrong syscall number sitting unexercised is
        // exactly what this cross-check exists to catch — the same shape as M13's `signal_of_esr`
        // row. All six are the SDK's own values, `MacOSX.sdk/usr/include/sys/syscall.h` lines
        // 555/556 for the pair.
        assert_eq!(
            (SYS_BSDTHREAD_CREATE, SYS_BSDTHREAD_TERMINATE, SYS_BSDTHREAD_REGISTER, SYS_THREAD_SELFID,
             SYS_ULOCK_WAIT, SYS_ULOCK_WAKE),
            (360, 361, 366, 372, 515, 516)
        );

        // M18: the workqueue pair. Both are SDK values (`MacOSX.sdk/usr/include/sys/syscall.h`).
        // M14 measured BOTH as firing zero times from a pthread guest; M18's probe measured them
        // as STILL firing zero times from a libdispatch guest, because libdispatch dies before it
        // reaches them. They are pinned here before they have ever fired, so the number is right
        // the first time one does.
        assert_eq!((SYS_WORKQ_OPEN, SYS_WORKQ_KERNRETURN), (367, 368));
    }
}
