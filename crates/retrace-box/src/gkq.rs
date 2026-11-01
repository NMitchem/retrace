//! M48 §3c (K1): the guest's own kqueues, and the byte counts of the guest's pipes that their read
//! and write filters answer from (R4). Pure data with no `Box_` access, the `kq.rs` pattern:
//! `Box_` owns one (`Box_::gkq`) and carries it through every rebuild path in `BoxState`, because
//! a mid-run capture cannot re-derive it.
//!
//! Natively, a change the kernel rejects becomes an `EV_ERROR` event. The model never makes one: it
//! refuses every such change by value, naming its index and fields (R5, K4). So no change produces
//! an output, and a call scans exactly when it has an event list (F7).

use std::collections::BTreeMap;

use retrace_arch::{Kevent, EVFILT_READ, EVFILT_USER, EVFILT_WRITE, EV_ADD, EV_CLEAR, EV_DELETE,
                   EV_ENABLE, EV_EOF, EV_ONESHOT, EV_SYSFLAGS, NOTE_FFAND, NOTE_FFCOPY,
                   NOTE_FFCTRLMASK, NOTE_FFLAGSMASK, NOTE_FFOR, NOTE_TRIGGER};

/// `EV_DISABLE` (SDK `sys/event.h`). Task 2's constants stop short of it.
pub const EV_DISABLE: u16 = 0x0008;
/// `sizeof(struct kevent)` (SDK `sys/event.h`).
pub const KEVENT_BYTES: usize = 32;
/// A new guest pipe's buffer: `EVFILT_WRITE`'s `data` on an empty pipe (t0 M3).
pub const PIPE_SIZE: u64 = 16384;
/// What the buffer grows to when a write takes the held bytes past `PIPE_SIZE` (Ruling T4-pipesize,
/// `pipesizeprobe.c`: even one byte past it grows it, and it never shrinks).
pub const PIPE_SIZE_GROWN: u64 = 65536;
/// `PIPE_BUF` (SDK `sys/syslimits.h`): a write end is ready while this much room is left (F9).
pub const PIPE_BUF: u64 = 512;
/// A Unix stream socket pair's buffer, each direction (M55, `p0/sockprobe.txt`): `SO_SNDBUF` and `SO_RCVBUF`
/// are 8192 and `EVFILT_WRITE`'s `data` on an empty pair.
pub const SOCK_CAPACITY: u64 = 8192;
/// `SO_SNDLOWAT` (M55): a write end is ready while at least this much room is free.
pub const SOCK_LOWAT: u64 = 2048;
/// The bits of a knote's creating flags a delivery hands back (t0 M3, `read on the write end,
/// reader closed`, whose knote was added `EV_ADD|EV_ENABLE`). Every measured delivery returns the
/// add's flags verbatim (`detect`: `0x21`).
pub const ADD_FLAGS_RETURNED: u16 = !EV_SYSFLAGS;

/// What a guest descriptor is to the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FdKind {
    /// A kqueue the guest's `kqueue()` returned.
    Kqueue,
    /// The read end of guest pipe `id`.
    PipeRead(u64),
    /// The write end of guest pipe `id`.
    PipeWrite(u64),
    /// End `id` of a guest Unix stream socket pair (M55). An end is an open file shared by its dups.
    Sock(u64),
    /// Anything else: a file, a socket, a tty, or a descriptor the guest inherited (its 0, 1, 2).
    Other,
}

/// One knote, keyed in its kqueue by `(ident, filter)`, the kernel's key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Knote {
    /// The creating `EV_ADD`'s flags, less `EV_SYSFLAGS`. A touch never changes them (K9).
    pub flags: u16,
    /// `kn_sfflags`: what an `EVFILT_USER` delivery returns as `fflags` (F8).
    pub sfflags: u32,
    /// `kn_sdata`: what an `EVFILT_USER` delivery returns as `data` (F8).
    pub sdata: i64,
    pub udata: u64,
    pub enabled: bool,
    /// `Some(n)` while active: `n` is its place in the activation order, which is delivery order.
    pub active: Option<u64>,
    /// For `EVFILT_READ`/`EVFILT_WRITE`: what the descriptor was at `EV_ADD`.
    pub watch: Option<FdKind>,
}

/// The thread blocked in `kevent` on a kqueue, and where its events go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Waiter {
    pub tid: usize,
    /// The descriptor it blocked through. A kqueue may have several (M53 t3), and its close is the
    /// one refused while the thread waits.
    pub fd: u64,
    /// The guest VA of its event list.
    pub events: u64,
    pub nevents: usize,
}

/// What a thread blocked in `poll` or `select` waits on, parsed at the call (M53 §3e). The reply
/// goes to the same addresses when the wait ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PollWait {
    /// `poll`: the `pollfd` array at `va`, as the guest passed it (8 bytes an entry).
    Poll { va: u64, raw: Vec<u8> },
    /// `select` and `pselect`: the guest VAs of the three sets (0 for a NULL set), `nfds`, and the
    /// read and write sets as passed (`ceil(nfds / 32) * 4` bytes each).
    Select { nfds: usize, vas: [u64; 3], rset: Vec<u8>, wset: Vec<u8> },
}

impl PollWait {
    /// Does this wait watch descriptor `fd`?
    fn watches(&self, fd: u64) -> bool {
        match self {
            PollWait::Poll { raw, .. } => raw.chunks_exact(8)
                .any(|e| i32::from_le_bytes(e[..4].try_into().unwrap()) as i64 == fd as i64),
            PollWait::Select { nfds, rset, wset, .. } => (fd as usize) < *nfds
                && [rset, wset].iter().any(|s| s.get(fd as usize / 8).is_some_and(|b| b >> (fd % 8) & 1 != 0)),
        }
    }
}

/// What a pipe end is to a poller: whether it has anything to read, room to write, and whether the
/// other end is gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PipeState {
    /// M55: a socket end, which is both readable and writable and answers `poll` and `select` by its own rules.
    pub socket: bool,
    pub write_end: bool,
    pub readable: bool,
    pub writable: bool,
    pub other_closed: bool,
}

/// One guest kqueue.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kqueue {
    knotes: BTreeMap<(u64, i16), Knote>,
    /// At most one (K2).
    waiter: Option<Waiter>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pipe {
    /// Unread bytes. `None` once a read returned more than the count, so the model lost it.
    count: Option<u64>,
    /// The buffer's size: `PIPE_SIZE`, then `PIPE_SIZE_GROWN` for good once a write passes the first.
    size: u64,
    /// Open guest descriptors on each end. A `dup` adds one, and the end closes at zero.
    readers: u32,
    writers: u32,
}

/// The guest's pipes, by descriptor (R4). The counts move only by the calls' own returns:
/// forwarded on record, recorded on replay (`Box_::note_fd_effects`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pipes {
    /// Guest fd -> (pipe id, is the write end).
    ends: BTreeMap<u64, (u64, bool)>,
    pipes: BTreeMap<u64, Pipe>,
    next: u64,
}

impl Pipes {
    fn kind_of(&self, fd: u64) -> Option<FdKind> {
        self.ends.get(&fd).map(|&(id, w)| if w { FdKind::PipeWrite(id) } else { FdKind::PipeRead(id) })
    }

    fn bind(&mut self, fd: u64, id: u64, write: bool) {
        self.ends.insert(fd, (id, write));
        let p = self.pipes.get_mut(&id).expect("a bound end's pipe exists");
        if write { p.writers += 1 } else { p.readers += 1 }
    }

    /// Forget `fd`. Returns whether it was a pipe end. A pipe with no descriptor left goes.
    fn unbind(&mut self, fd: u64) -> bool {
        let Some((id, write)) = self.ends.remove(&fd) else { return false };
        let p = self.pipes.get_mut(&id).expect("a bound end's pipe exists");
        if write { p.writers -= 1 } else { p.readers -= 1 }
        if p.readers == 0 && p.writers == 0 { self.pipes.remove(&id); }
        true
    }

    /// Is `filter` on descriptor `fd`, which `watch` describes, ready, and with what `EV_EOF` bit
    /// and `data`? `sys_pipe.c:filt_piperead_common` and `:filt_pipewrite_common` (F9).
    pub fn ready(&self, fd: u64, watch: FdKind, filter: i16) -> Result<Option<(u16, i64)>, String> {
        let lost = || retrace_arch::wall(retrace_arch::WallCategory::Threads, format!(
            "M48: pipe {fd}: the byte count is unknown: a read returned more bytes than the model \
             counted, so a write reached the pipe by a path note_fd_effects does not see"));
        match (watch, filter) {
            (FdKind::PipeRead(id), EVFILT_READ) => {
                let p = self.pipes[&id];
                let eof = p.writers == 0;
                let count = p.count.ok_or_else(lost)?;
                Ok((count >= 1 || eof).then_some((if eof { EV_EOF } else { 0 }, count as i64)))
            }
            // Each end has its own buffer and nothing writes into the write end's, so its read
            // filter fires only at EOF (F9).
            (FdKind::PipeWrite(id), EVFILT_READ) => Ok((self.pipes[&id].readers == 0).then_some((EV_EOF, 0))),
            (FdKind::PipeWrite(id), EVFILT_WRITE) => {
                let p = self.pipes[&id];
                if p.readers == 0 { return Ok(Some((EV_EOF, 0))); }
                let count = p.count.ok_or_else(lost)?;
                let room = p.size.checked_sub(count).ok_or_else(lost)?;
                Ok((room >= PIPE_BUF).then_some((0, room as i64)))
            }
            // K3: libuv's uv__stream_try_select probe, never ready.
            (FdKind::Other, _) => Ok(None),
            (w, f) => unreachable!("M48: apply admits no filter {f} on {w:?}"),
        }
    }
}

/// What a socket end is to the model (M55 3c).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SockState {
    /// Bytes the peer wrote that this end has not read. `None` once a read returned more than the count.
    pub unread: Option<u64>,
    /// The same for the peer's end: what this end's writes have left it to read.
    pub peer_unread: Option<u64>,
    pub peer_open: bool,
    pub nonblock: bool,
    pub nosigpipe: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SockEnd {
    peer: u64,
    unread: Option<u64>,
    /// Open guest descriptors on this end. A `dup` adds one, and the end is closed at zero.
    open: u32,
    nonblock: bool,
    nosigpipe: bool,
}

/// The guest's socket pairs (M55 3c). Like `Pipes`, the counts move only by the calls' own returns:
/// forwarded on record, recorded on replay (`Box_::note_fd_effects`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Socks {
    /// Guest fd -> end id.
    fds: BTreeMap<u64, u64>,
    ends: BTreeMap<u64, SockEnd>,
    next: u64,
}

impl Socks {
    fn kind_of(&self, fd: u64) -> Option<FdKind> { self.fds.get(&fd).map(|&e| FdKind::Sock(e)) }

    fn bind(&mut self, fd: u64, end: u64) {
        self.fds.insert(fd, end);
        self.ends.get_mut(&end).expect("a bound descriptor's end exists").open += 1;
    }

    /// Forget `fd`. Returns whether it was a socket end. A pair with no descriptor left on either end goes.
    fn unbind(&mut self, fd: u64) -> bool {
        let Some(end) = self.fds.remove(&fd) else { return false };
        let e = self.ends.get_mut(&end).expect("a bound descriptor's end exists");
        e.open -= 1;
        let peer = e.peer;
        if e.open == 0 && self.ends[&peer].open == 0 { self.ends.remove(&end); self.ends.remove(&peer); }
        true
    }

    fn state(&self, end: u64) -> SockState {
        let e = self.ends[&end];
        let p = self.ends[&e.peer];
        SockState { unread: e.unread, peer_unread: p.unread, peer_open: p.open > 0, nonblock: e.nonblock, nosigpipe: e.nosigpipe }
    }

    /// Is `filter` on descriptor `fd`, an end of the pair, ready, and with what `EV_EOF` bit and `data`
    /// (spec 2b, measured): read from one unread byte or a closed peer, `data` the unread count; write
    /// while `SOCK_CAPACITY - peer_unread >= SOCK_LOWAT` with `data` that free space, or a closed peer
    /// (`EV_EOF`, `data` 8192).
    pub fn ready(&self, fd: u64, end: u64, filter: i16) -> Result<Option<(u16, i64)>, String> {
        let st = self.state(end);
        match filter {
            EVFILT_READ => {
                let unread = st.unread.ok_or_else(|| sock_lost(fd))?;
                Ok((unread >= 1 || !st.peer_open).then_some((if st.peer_open { 0 } else { EV_EOF }, unread as i64)))
            }
            EVFILT_WRITE => {
                if !st.peer_open { return Ok(Some((EV_EOF, SOCK_CAPACITY as i64))); }
                let free = SOCK_CAPACITY.checked_sub(st.peer_unread.ok_or_else(|| sock_lost(fd))?).ok_or_else(|| sock_lost(fd))?;
                Ok((free >= SOCK_LOWAT).then_some((0, free as i64)))
            }
            f => unreachable!("M55: apply admits no filter {f} on a socket"),
        }
    }
}

/// The refusal for a socket whose byte count the model lost.
fn sock_lost(fd: u64) -> String {
    retrace_arch::wall(retrace_arch::WallCategory::Threads, format!(
        "M55: socket {fd}: the byte count is unknown: a read returned more bytes than the model \
         counted, so a write reached the socket by a path note_fd_effects does not see"))
}

/// Is `filter` on descriptor `fd`, which `watch` describes, ready? One answer over the pipe and socket
/// models; a descriptor that is neither is never ready (K3, libuv's probe).
fn ready_of(pipes: &Pipes, socks: &Socks, fd: u64, watch: FdKind, filter: i16) -> Result<Option<(u16, i64)>, String> {
    match watch {
        FdKind::Sock(end) => socks.ready(fd, end, filter),
        w => pipes.ready(fd, w, filter),
    }
}

/// Every guest kqueue, by id, the descriptors on each, and the guest's pipes. The kernel's kqueue is
/// per open file, so a `dup` of its descriptor shares the one table (M53 t3, t0 §M4).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuestKqueues {
    kqs: BTreeMap<u64, Kqueue>,
    /// Guest fd -> the id of the kqueue it names.
    fd_kq: BTreeMap<u64, u64>,
    next_id: u64,
    pipes: Pipes,
    /// The guest's socket pairs (M55).
    socks: Socks,
    /// Threads blocked in `poll` or `select`, by thread (M53 §3e).
    poll_waiters: BTreeMap<usize, PollWait>,
    /// The activation counter, one for every kqueue, so the order is total.
    seq: u64,
}

impl GuestKqueues {
    pub fn is_kqueue(&self, fd: u64) -> bool { self.fd_kq.contains_key(&fd) }

    fn kq_of(&self, fd: u64) -> Option<&Kqueue> { self.fd_kq.get(&fd).and_then(|id| self.kqs.get(id)) }

    fn kq_of_mut(&mut self, fd: u64) -> Option<&mut Kqueue> {
        let id = *self.fd_kq.get(&fd)?;
        self.kqs.get_mut(&id)
    }

    pub fn kind_of(&self, fd: u64) -> FdKind {
        if self.is_kqueue(fd) { FdKind::Kqueue } else {
            self.pipes.kind_of(fd).or_else(|| self.socks.kind_of(fd)).unwrap_or(FdKind::Other)
        }
    }

    /// A forwarded `kqueue()` returned guest fd `fd`: an empty table for it (M48 §3c).
    pub fn create(&mut self, fd: u64) -> Result<(), String> {
        let k = self.kind_of(fd);
        if k != FdKind::Other {
            return Err(format!("M48: kevent on fd {fd}: kqueue() returned fd {fd}, which the model \
                                still holds as {k:?}: a close it never saw"));
        }
        let id = self.next_id;
        self.next_id += 1;
        self.kqs.insert(id, Kqueue::default());
        self.fd_kq.insert(fd, id);
        Ok(())
    }

    /// A forwarded `pipe()` returned the guest pair `(r, w)` (M38's `Ret::FdPair`), empty.
    pub fn pipe(&mut self, r: u64, w: u64) -> Result<(), String> {
        for fd in [r, w] {
            let k = self.kind_of(fd);
            if k != FdKind::Other {
                return Err(format!("M48: pipe {fd}: pipe() returned fd {fd}, which the model still \
                                    holds as {k:?}: a close it never saw"));
            }
        }
        let id = self.pipes.next;
        self.pipes.next += 1;
        self.pipes.pipes.insert(id, Pipe { count: Some(0), size: PIPE_SIZE, readers: 0, writers: 0 });
        self.pipes.bind(r, id, false);
        self.pipes.bind(w, id, true);
        Ok(())
    }

    /// A forwarded `socketpair(AF_UNIX, SOCK_STREAM, 0)` wrote the guest pair `(g0, g1)` (M55 `Ret::FdPairOut`):
    /// two connected ends, nothing unread, blocking, and `SO_NOSIGPIPE` off.
    pub fn socket_pair(&mut self, g0: u64, g1: u64) -> Result<(), String> {
        for fd in [g0, g1] {
            let k = self.kind_of(fd);
            if k != FdKind::Other {
                return Err(format!("M55: socketpair: socketpair() returned fd {fd}, which the model still \
                                    holds as {k:?}: a close it never saw"));
            }
        }
        let (a, b) = (self.socks.next, self.socks.next + 1);
        self.socks.next += 2;
        for (id, peer) in [(a, b), (b, a)] {
            self.socks.ends.insert(id, SockEnd { peer, unread: Some(0), open: 0, nonblock: false, nosigpipe: false });
        }
        self.socks.bind(g0, a);
        self.socks.bind(g1, b);
        Ok(())
    }

    /// What socket end `fd` is, or `None` for any other descriptor (M55).
    pub fn sock_state(&self, fd: u64) -> Option<SockState> {
        self.socks.fds.get(&fd).map(|&e| self.socks.state(e))
    }

    /// `O_NONBLOCK` is a bit of the open file, shared by every dup of the end (M55 3g; Task 4's callers).
    pub fn set_nonblock(&mut self, fd: u64, on: bool) {
        if let Some(&e) = self.socks.fds.get(&fd) { self.socks.ends.get_mut(&e).expect("a bound end exists").nonblock = on; }
    }

    /// `SO_NOSIGPIPE` of the end (M55 3f; Task 4's callers).
    pub fn set_nosigpipe(&mut self, fd: u64, on: bool) {
        if let Some(&e) = self.socks.fds.get(&fd) { self.socks.ends.get_mut(&e).expect("a bound end exists").nosigpipe = on; }
    }

    /// M54 t3 (spec 2b, measured): exec closes every kqueue descriptor, and the threads that waited on
    /// anything are gone with their image. Every kqueue (its knotes and its waiter) and every poll waiter is
    /// dropped, and the descriptors that named a kqueue come back so the caller closes their slots. The
    /// pipes and their counts stay: both ends survive exec (measured), less the descriptors the caller then
    /// closes. Ids and the activation counter only ever grow, so they stay too.
    pub fn reset_for_exec(&mut self) -> Vec<u64> {
        let fds: Vec<u64> = self.fd_kq.keys().copied().collect();
        self.kqs.clear();
        self.fd_kq.clear();
        self.poll_waiters.clear();
        fds
    }

    /// `fd` closed. A kqueue's table goes with its last descriptor; every knote on `fd` leaves every
    /// kqueue (`kern_event.c:knote_fdclose`); and a pipe end loses a descriptor. The close of the
    /// very descriptor a thread blocked in `kevent` through is refused: natively that thread wakes at
    /// once with EBADF, which is unmodelled (Ruling T0-M4). The close of another descriptor of the
    /// same kqueue is a plain close, and the waiter is not woken (t0 §M4).
    pub fn close(&mut self, fd: u64) -> Result<(), String> {
        // Ruling T4-closepolled (a): what natively happens to a poller whose descriptor is closed under
        // it is unmeasured.
        if let Some((&tid, _)) = self.poll_waiters.iter().find(|(_, w)| w.watches(fd)) {
            return Err(retrace_arch::wall(retrace_arch::WallCategory::Threads, format!(
                "M53: close of descriptor {fd} while thread {tid} waits in poll or select on it: what the \
                 waiter then sees is not modelled (Ruling T4-closepolled)")));
        }
        if let Some(w) = self.kq_of(fd).and_then(|k| k.waiter).filter(|w| w.fd == fd) {
            return Err(retrace_arch::wall(retrace_arch::WallCategory::Threads, format!(
                "M53: close of kqueue descriptor {fd} while thread {} waits in kevent on it: natively the \
                 waiter wakes at once with EBADF, which is not modelled (Ruling T0-M4)", w.tid)));
        }
        if let Some(id) = self.fd_kq.remove(&fd) {
            if !self.fd_kq.values().any(|&v| v == id) { self.kqs.remove(&id); }
        }
        for k in self.kqs.values_mut() {
            k.knotes.retain(|&(ident, filter), _| !(ident == fd && matches!(filter, EVFILT_READ | EVFILT_WRITE)));
        }
        if self.pipes.unbind(fd) | self.socks.unbind(fd) { self.refresh()?; }
        Ok(())
    }

    /// `dup`, `dup2` or `F_DUPFD` made `new` a copy of `old`. Closing `new` first is `dup2`'s
    /// implicit close. A kqueue's new descriptor names the same kqueue (M53 t3). A pipe end gains a
    /// descriptor.
    pub fn dup(&mut self, old: u64, new: u64) -> Result<(), String> {
        if old == new { return Ok(()); }
        let id = self.fd_kq.get(&old).copied();
        self.close(new)?;
        if let Some(id) = id { self.fd_kq.insert(new, id); }
        if let Some(&(id, write)) = self.pipes.ends.get(&old) { self.pipes.bind(new, id, write); }
        if let Some(&end) = self.socks.fds.get(&old) { self.socks.bind(new, end); }
        Ok(())
    }

    /// A read from `fd` returned `n` bytes. A pipe's read end counts, and a socket end's own unread bytes.
    pub fn note_read(&mut self, fd: u64, n: u64) -> Result<(), String> {
        if let Some(&end) = self.socks.fds.get(&fd) {
            let e = self.socks.ends.get_mut(&end).expect("a bound descriptor's end exists");
            e.unread = e.unread.and_then(|c| c.checked_sub(n));
            return self.refresh();
        }
        let Some(&(id, false)) = self.pipes.ends.get(&fd) else { return Ok(()) };
        let p = self.pipes.pipes.get_mut(&id).expect("a bound end's pipe exists");
        p.count = p.count.and_then(|c| c.checked_sub(n));
        self.refresh()
    }

    /// A write to `fd` returned `n` bytes. A pipe's write end counts, and a socket end's write adds to its
    /// peer's unread bytes.
    pub fn note_write(&mut self, fd: u64, n: u64) -> Result<(), String> {
        if let Some(&end) = self.socks.fds.get(&fd) {
            let peer = self.socks.ends[&end].peer;
            let p = self.socks.ends.get_mut(&peer).expect("a connected end's peer exists");
            p.unread = p.unread.map(|c| c + n);
            return self.refresh();
        }
        let Some(&(id, true)) = self.pipes.ends.get(&fd) else { return Ok(()) };
        let p = self.pipes.pipes.get_mut(&id).expect("a bound end's pipe exists");
        p.count = p.count.map(|c| c + n);
        if p.count.is_some_and(|c| c > PIPE_SIZE) { p.size = PIPE_SIZE_GROWN; }
        self.refresh()
    }

    pub fn pipe_count(&self, fd: u64) -> Option<u64> {
        self.pipes.ends.get(&fd).and_then(|(id, _)| self.pipes.pipes[id].count)
    }

    /// Re-evaluate every pipe knote: one that became ready joins the activation order at its tail,
    /// and one that stopped being ready leaves it (level-triggered).
    fn refresh(&mut self) -> Result<(), String> {
        let Self { kqs, pipes, socks, seq, .. } = self;
        for k in kqs.values_mut() {
            for (&(ident, filter), n) in k.knotes.iter_mut() {
                let Some(watch) = n.watch else { continue };
                match ready_of(pipes, socks, ident, watch, filter)? {
                    Some(_) if n.active.is_none() => { *seq += 1; n.active = Some(*seq); }
                    Some(_) => {}
                    None => n.active = None,
                }
            }
        }
        Ok(())
    }

    /// Apply `changes` to kqueue `kq`, in list order (`kern_event.c:kevent_register`, F8). All or
    /// nothing: they go to a copy that replaces the table only if every change is admitted.
    pub fn apply(&mut self, kq: u64, changes: &[Kevent]) -> Result<(), String> {
        let id = *self.fd_kq.get(&kq)
            .ok_or_else(|| format!("M48: kevent on fd {kq}, which is not a guest kqueue"))?;
        let mut k = self.kqs[&id].clone();
        let mut seq = self.seq;
        for (i, c) in changes.iter().enumerate() {
            self.apply_one(&mut k, &mut seq, i, c)?;
        }
        self.kqs.insert(id, k);
        self.seq = seq;
        Ok(())
    }

    fn apply_one(&self, k: &mut Kqueue, seq: &mut u64, i: usize, c: &Kevent) -> Result<(), String> {
        // F8: input flags lose EV_SYSFLAGS at copyin.
        let flags = c.flags & !EV_SYSFLAGS;
        let at = format!("M48: kevent change {i}: (ident {:#x}, filter {}, flags {flags:#x}, fflags {:#x}, data {:#x})",
                         c.ident, c.filter, c.fflags, c.data);
        const MODELLED: u16 = EV_ADD | EV_DELETE | EV_ENABLE | EV_DISABLE | EV_CLEAR | EV_ONESHOT;
        if flags & !MODELLED != 0 {
            return Err(retrace_arch::wall(retrace_arch::WallCategory::Threads, format!("{at}: flags {:#x} are not modelled (EV_RECEIPT, EV_DISPATCH, \
                                EV_UDATA_SPECIFIC and the rest are unmeasured)", flags & !MODELLED)));
        }
        if flags & (EV_ADD | EV_DELETE) == EV_ADD | EV_DELETE || flags & (EV_ENABLE | EV_DISABLE) == EV_ENABLE | EV_DISABLE {
            return Err(retrace_arch::wall(retrace_arch::WallCategory::Threads, format!("{at}: contradictory flags are not modelled")));
        }
        if !matches!(c.filter, EVFILT_USER | EVFILT_READ | EVFILT_WRITE) {
            return Err(retrace_arch::wall(retrace_arch::WallCategory::Threads, format!("{at}: filter {} is not modelled: only EVFILT_USER and the pipe filters \
                                are (M48 §3c)", c.filter)));
        }
        let key = (c.ident, c.filter);
        if flags & EV_DELETE != 0 {
            return match k.knotes.remove(&key) {
                Some(_) => Ok(()),
                None => Err(retrace_arch::wall(retrace_arch::WallCategory::Threads, format!("{at}: EV_DELETE of a knote that is not registered: the kernel \
                                     answers an ENOENT EV_ERROR event, which is not modelled"))),
            };
        }
        let fd_filter = c.filter != EVFILT_USER;
        if fd_filter && (c.fflags != 0 || c.data != 0) {
            return Err(retrace_arch::wall(retrace_arch::WallCategory::Threads, format!("{at}: a pipe filter with fflags or data (NOTE_LOWAT and the rest) is not modelled")));
        }
        let Some(n) = k.knotes.get_mut(&key) else {
            if flags & EV_ADD == 0 {
                return Err(retrace_arch::wall(retrace_arch::WallCategory::Threads, format!("{at}: the knote is not registered and the change has no EV_ADD: \
                                    the kernel answers an ENOENT EV_ERROR event, which is not modelled")));
            }
            let mut n = Knote { flags, sfflags: 0, sdata: c.data, udata: c.udata,
                                enabled: flags & EV_DISABLE == 0, active: None, watch: None };
            if fd_filter {
                if flags & (EV_CLEAR | EV_ONESHOT) != 0 {
                    return Err(retrace_arch::wall(retrace_arch::WallCategory::Threads, format!("{at}: EV_CLEAR or EV_ONESHOT on a pipe filter is not modelled")));
                }
                let watch = self.kind_of(c.ident);
                match (watch, c.filter) {
                    (FdKind::PipeRead(_), EVFILT_READ) | (FdKind::PipeWrite(_), EVFILT_READ | EVFILT_WRITE) => {}
                    (FdKind::Sock(_), EVFILT_READ | EVFILT_WRITE) => {}
                    (FdKind::Other, EVFILT_READ) if flags == EV_ADD | EV_ENABLE => {}
                    (FdKind::Other, f) => return Err(retrace_arch::wall(retrace_arch::WallCategory::Threads, format!(
                        "M48: kevent on fd {}: change {i}'s filter {f} with flags {flags:#x} asks about a \
                         descriptor that is not a guest pipe end: readiness of a file, socket, tty or \
                         inherited descriptor is not modelled (M48 §7); only uv__stream_try_select's \
                         EVFILT_READ, EV_ADD|EV_ENABLE is answered, never ready (K3)", c.ident))),
                    (w, f) => return Err(retrace_arch::wall(retrace_arch::WallCategory::Threads, format!("{at}: filter {f} on {w:?} is not modelled"))),
                }
                n.active = match ready_of(&self.pipes, &self.socks, c.ident, watch, c.filter)? {
                    Some(_) => { *seq += 1; Some(*seq) }
                    None => None,
                };
                n.watch = Some(watch);
            } else {
                if c.fflags & !(NOTE_FFLAGSMASK | NOTE_TRIGGER) != 0 {
                    return Err(retrace_arch::wall(retrace_arch::WallCategory::Threads, format!("{at}: an EV_ADD whose fflags carry a NOTE_FF* \
                                        operation: what the kernel then delivers as fflags is unmeasured")));
                }
                if c.fflags & NOTE_TRIGGER != 0 && flags & EV_DISABLE != 0 {
                    return Err(retrace_arch::wall(retrace_arch::WallCategory::Threads, format!("{at}: an EV_ADD carrying both EV_DISABLE and NOTE_TRIGGER: whether the \
                                        trigger is held for a later EV_ENABLE is unmeasured")));
                }
                n.sfflags = c.fflags & NOTE_FFLAGSMASK;
                // `kern_event.c:filt_userattach` (inferred from source): an add carrying NOTE_TRIGGER
                // activates at once. mio's waker registers and wakes in this one change.
                if c.fflags & NOTE_TRIGGER != 0 && n.enabled {
                    *seq += 1;
                    n.active = Some(*seq);
                }
            }
            k.knotes.insert(key, n);
            return Ok(());
        };
        // A touch (K9).
        let kind = flags & (EV_CLEAR | EV_ONESHOT);
        if kind != 0 && kind != n.flags & (EV_CLEAR | EV_ONESHOT) {
            return Err(retrace_arch::wall(retrace_arch::WallCategory::Threads, format!("{at}: a touch that changes EV_CLEAR or EV_ONESHOT on a registered knote \
                                is not modelled")));
        }
        n.udata = c.udata;
        if flags & EV_ENABLE != 0 { n.enabled = true; }
        if flags & EV_DISABLE != 0 { n.enabled = false; }
        if !fd_filter {
            // kern_event.c:filt_usertouch (F8).
            let ff = c.fflags & NOTE_FFLAGSMASK;
            match c.fflags & NOTE_FFCTRLMASK {
                NOTE_FFAND => n.sfflags &= ff,
                NOTE_FFOR => n.sfflags |= ff,
                NOTE_FFCOPY => n.sfflags = ff,
                _ => {} // NOTE_FFNOP
            }
            n.sdata = c.data;
            if c.fflags & NOTE_TRIGGER != 0 && n.active.is_none() {
                *seq += 1;
                n.active = Some(*seq);
            }
        }
        Ok(())
    }

    /// Does `kq` hold an event a scan would deliver?
    pub fn has_events(&self, kq: u64) -> bool {
        self.kq_of(kq).is_some_and(|k| k.knotes.values().any(|n| n.enabled && n.active.is_some()))
    }

    /// Deliver up to `max` active, enabled knotes of `kq` in activation order
    /// (`kern_event.c:kqueue_process`, F8): `EV_ONESHOT` drops a knote, `EV_CLEAR` deactivates it,
    /// and any other is re-activated at the tail. A pipe knote's `data` and `EV_EOF` are read now.
    pub fn take_events(&mut self, kq: u64, max: usize) -> Result<Vec<Kevent>, String> {
        let Self { kqs, fd_kq, pipes, socks, seq, .. } = self;
        let id = fd_kq.get(&kq).expect("take_events on a guest kqueue");
        let k = kqs.get_mut(id).expect("a mapped kqueue id is live");
        let mut due: Vec<((u64, i16), u64)> = k.knotes.iter()
            .filter_map(|(&key, n)| n.active.filter(|_| n.enabled).map(|s| (key, s)))
            .collect();
        due.sort_by_key(|&(_, s)| s);
        due.truncate(max);
        let mut out = Vec::with_capacity(due.len());
        for (key, _) in due {
            let n = k.knotes.get_mut(&key).expect("collected from this table just above");
            let (eof, data, fflags) = match n.watch {
                None => (0, n.sdata, n.sfflags),
                Some(w) => {
                    let (eof, data) = ready_of(pipes, socks, key.0, w, key.1)?
                        .expect("an active pipe knote is ready: refresh keeps the two in step");
                    (eof, data, 0)
                }
            };
            out.push(Kevent { ident: key.0, filter: key.1, flags: (n.flags & ADD_FLAGS_RETURNED) | eof,
                              fflags, data, udata: n.udata });
            if n.flags & EV_ONESHOT != 0 {
                k.knotes.remove(&key);
            } else if n.flags & EV_CLEAR != 0 {
                n.active = None;
            } else {
                *seq += 1;
                n.active = Some(*seq);
            }
        }
        Ok(out)
    }

    /// What `fd` is to a poller, or the refusal's tail for a descriptor that is not a guest pipe end
    /// (M53 §3e; the caller names the call and the descriptor). `Err` also reports a byte count the
    /// model lost, as `Pipes::ready` does.
    pub fn pipe_state(&self, fd: u64) -> Result<PipeState, String> {
        let lost = || format!("pipe {fd}: the byte count is unknown: a read returned more bytes than the model \
                               counted, so a write reached the pipe by a path note_fd_effects does not see");
        match self.kind_of(fd) {
            FdKind::PipeRead(id) => {
                let p = self.pipes.pipes[&id];
                let count = p.count.ok_or_else(lost)?;
                let closed = p.writers == 0;
                Ok(PipeState { socket: false, write_end: false, readable: count >= 1 || closed, writable: false, other_closed: closed })
            }
            FdKind::PipeWrite(id) => {
                let p = self.pipes.pipes[&id];
                let count = p.count.ok_or_else(lost)?;
                let room = p.size.checked_sub(count).ok_or_else(lost)?;
                Ok(PipeState { socket: false, write_end: true, readable: false,
                               writable: p.readers > 0 && room >= PIPE_BUF,
                               other_closed: p.readers == 0 })
            }
            // M55: `readable` is `unread >= 1` alone and `writable` the peer-open free-space rule; a closed
            // peer is `other_closed`, which `poll_ready` and `select` each turn into their own answer.
            FdKind::Sock(end) => {
                let st = self.socks.state(end);
                let unread = st.unread.ok_or_else(|| sock_lost(fd))?;
                let free = SOCK_CAPACITY.checked_sub(st.peer_unread.ok_or_else(|| sock_lost(fd))?).ok_or_else(|| sock_lost(fd))?;
                Ok(PipeState { socket: true, write_end: false, readable: unread >= 1,
                               writable: st.peer_open && free >= SOCK_LOWAT, other_closed: !st.peer_open })
            }
            FdKind::Kqueue => Err("a kqueue, not a guest pipe".to_string()),
            FdKind::Other => Err("not a guest pipe end".to_string()),
        }
    }

    /// `poll`'s `revents` for descriptor `fd` asked about `events` (t0 M7, Ruling T0-M7: computed from
    /// the requested bits and the pipe's state, never a fixed pair). A requested read-ish bit reports
    /// itself when the pipe is readable or its other end is closed; a requested write-ish bit reports
    /// `POLLOUT` (`POLLWRBAND` reports `0x100`) when the pipe is writable and nothing otherwise;
    /// `POLLHUP` is added when the other end is closed and some bit but `POLLERR` was requested.
    /// `POLLERR` is never reported on a pipe. `Err` is `pipe_state`'s refusal.
    pub fn poll_ready(&self, fd: u64, events: u16) -> Result<u16, String> {
        use retrace_arch::{POLLERR, POLLHUP, POLLIN, POLLOUT, POLLPRI, POLLRDBAND, POLLRDNORM, POLLWRBAND, POLLWRNORM};
        let st = self.pipe_state(fd)?;
        let mut r = 0;
        for bit in [POLLIN, POLLPRI, POLLRDNORM, POLLRDBAND] {
            // T0-poll: an open socket end never answers POLLPRI or POLLRDBAND, a closed peer answers every
            // requested read-ish bit.
            let by_data = st.readable && !(st.socket && matches!(bit, POLLPRI | POLLRDBAND));
            if events & bit != 0 && (by_data || st.other_closed) { r |= bit; }
        }
        if st.writable {
            if events & (POLLOUT | POLLWRNORM) != 0 { r |= POLLOUT; }
            if events & POLLWRBAND != 0 { r |= POLLWRBAND; }
        }
        if st.other_closed && events & !POLLERR != 0 { r |= POLLHUP; }
        Ok(r)
    }

    pub fn set_poll_waiter(&mut self, tid: usize, w: PollWait) {
        let old = self.poll_waiters.insert(tid, w);
        assert!(old.is_none(), "M53: thread {tid} blocked in poll twice");
    }

    pub fn take_poll_waiter(&mut self, tid: usize) -> Option<PollWait> { self.poll_waiters.remove(&tid) }

    /// The threads blocked in `poll` or `select`, with what they wait on, in thread order.
    pub fn poll_waiters(&self) -> Vec<(usize, PollWait)> {
        self.poll_waiters.iter().map(|(&t, w)| (t, w.clone())).collect()
    }

    pub fn waiter(&self, kq: u64) -> Option<Waiter> { self.kq_of(kq).and_then(|k| k.waiter) }

    /// Record `w` as `kq`'s waiter. `Box_::guest_kevent` refuses a second one first (K2).
    pub fn set_waiter(&mut self, kq: u64, w: Waiter) {
        let k = self.kq_of_mut(kq).expect("set_waiter on a guest kqueue");
        assert!(k.waiter.is_none(), "M48: a second waiter on kq {kq}; guest_kevent refuses it first (K2)");
        k.waiter = Some(w);
    }

    pub fn take_waiter(&mut self, kq: u64) -> Option<Waiter> {
        self.kq_of_mut(kq).and_then(|k| k.waiter.take())
    }

    /// The descriptor each ready waiter blocked through, in kqueue creation order.
    pub fn ready_waiters(&self) -> Vec<u64> {
        self.kqs.values()
            .filter_map(|k| k.waiter.filter(|_| k.knotes.values().any(|n| n.enabled && n.active.is_some())))
            .map(|w| w.fd).collect()
    }

    pub fn knote(&self, kq: u64, ident: u64, filter: i16) -> Option<&Knote> {
        self.kq_of(kq).and_then(|k| k.knotes.get(&(ident, filter)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use retrace_arch::Unmark;
    use retrace_arch::{EVFILT_TIMER, EV_RECEIPT};

    const KQ: u64 = 3;
    const ID: u64 = 0x1e7e_7711;

    fn ch(ident: u64, filter: i16, flags: u16, fflags: u32, data: i64, udata: u64) -> Kevent {
        Kevent { ident, filter, flags, fflags, data, udata }
    }
    fn user(flags: u16, fflags: u32) -> Kevent { ch(ID, EVFILT_USER, flags, fflags, 0, 0) }
    fn kq() -> GuestKqueues {
        let mut g = GuestKqueues::default();
        g.create(KQ).unwrap();
        g
    }

    /// M55 spec 2b: read is ready from one unread byte (`data` the count); write is ready while at least
    /// 2048 bytes are free (`data` the free space): 1024 free is not ready, 3072 is.
    #[test]
    fn a_socket_end_is_readable_from_one_byte_and_writable_while_2048_are_free() {
        let mut g = kq();
        g.socket_pair(10, 11).unwrap();
        let end = g.socks.fds[&10];
        assert_eq!(g.socks.ready(10, end, EVFILT_READ).unwrap(), None);
        assert_eq!(g.socks.ready(10, end, EVFILT_WRITE).unwrap(), Some((0, 8192)));
        g.note_write(11, 1).unwrap();
        assert_eq!(g.socks.ready(10, end, EVFILT_READ).unwrap(), Some((0, 1)));
        g.note_read(10, 1).unwrap();
        g.note_write(10, 7168).unwrap(); // 1024 free in 10's direction
        assert_eq!(g.socks.ready(10, end, EVFILT_WRITE).unwrap(), None);
        g.note_read(11, 2048).unwrap(); // 3072 free
        assert_eq!(g.socks.ready(10, end, EVFILT_WRITE).unwrap(), Some((0, 3072)));
        g.note_read(11, 8000).unwrap(); // more than counted: lost
        let e = g.socks.ready(11, g.socks.fds[&11], EVFILT_READ).unwrap_err().unmark();
        assert!(e.starts_with("M55: socket 11: the byte count is unknown"), "{e}");
    }

    /// M55 spec 2b: the peer's last close fires both filters with `EV_EOF`: read's `data` is the unread
    /// count, write's is 8192; and a registered knote is re-queued by the close.
    #[test]
    fn a_socket_peer_close_sets_eof_on_both_filters() {
        let mut g = kq();
        g.socket_pair(10, 11).unwrap();
        g.apply(KQ, &[ch(10, EVFILT_READ, EV_ADD, 0, 0, 0)]).unwrap();
        g.apply(KQ, &[ch(10, EVFILT_WRITE, EV_ADD, 0, 0, 0)]).unwrap();
        g.note_write(11, 2).unwrap();
        g.close(11).unwrap();
        let end = g.socks.fds[&10];
        assert_eq!(g.socks.ready(10, end, EVFILT_READ).unwrap(), Some((EV_EOF, 2)));
        assert_eq!(g.socks.ready(10, end, EVFILT_WRITE).unwrap(), Some((EV_EOF, 8192)));
        let ev = g.take_events(KQ, 4).unwrap();
        assert_eq!(ev.len(), 2);
        assert!(ev.iter().all(|e| e.flags & EV_EOF != 0), "{ev:?}");
        assert!(!g.sock_state(10).unwrap().peer_open);
    }

    /// A dup shares the open file: closing one of the peer's descriptors changes nothing; the last close does.
    #[test]
    fn a_dup_of_a_socket_end_keeps_the_peer_open_until_the_last_close() {
        let mut g = kq();
        g.socket_pair(10, 11).unwrap();
        g.dup(11, 12).unwrap();
        assert_eq!(g.kind_of(12), g.kind_of(11));
        g.close(11).unwrap();
        assert!(g.sock_state(10).unwrap().peer_open, "a dup of the peer remains");
        g.close(12).unwrap();
        assert!(!g.sock_state(10).unwrap().peer_open, "the last close ends it");
        g.close(10).unwrap();
        assert!(g.sock_state(10).is_none() && g.socks.ends.is_empty(), "a pair with no descriptor goes");
    }

    /// `kern_event.c:knote_fdclose`: a close drops the descriptor's READ and WRITE knotes in every
    /// kqueue, and a kqueue's own close drops its table. An `EVFILT_USER` knote names no descriptor
    /// and stays. Closing a kqueue a thread is blocked on is refused, because the kernel's answer
    /// to that thread is unmeasured.
    #[test]
    fn a_close_drops_the_descriptors_knotes_and_a_waited_kqueue_is_refused() {
        let mut g = kq();
        g.create(4).unwrap();
        g.apply(KQ, &[ch(9, EVFILT_READ, EV_ADD | EV_ENABLE, 0, 0, 0), user(EV_ADD, 0)]).unwrap();
        g.apply(4, &[ch(9, EVFILT_READ, EV_ADD | EV_ENABLE, 0, 0, 0)]).unwrap();
        g.close(9).unwrap();
        assert!(g.knote(KQ, 9, EVFILT_READ).is_none() && g.knote(4, 9, EVFILT_READ).is_none(),
            "every kqueue loses fd 9's knote");
        assert!(g.knote(KQ, ID, EVFILT_USER).is_some(), "an EVFILT_USER knote names no descriptor and stays");
        g.set_waiter(4, Waiter { tid: 1, fd: 4, events: 0x1000, nevents: 1 });
        let e = g.close(4).unwrap_err().unmark();
        assert!(e.starts_with("M53: close of kqueue descriptor 4 while thread 1 waits in kevent on it"), "{e}");
        assert!(g.is_kqueue(4), "the refused close changed nothing");
        g.close(KQ).unwrap();
        assert!(!g.is_kqueue(KQ) && g.knote(KQ, ID, EVFILT_USER).is_none(), "a kqueue's close drops its table");
    }

    /// K4: the kernel answers these with an ENOENT `EV_ERROR` event, which the model never makes.
    /// A refused call applies nothing, even its admitted earlier changes.
    #[test]
    fn a_change_to_an_unregistered_knote_is_refused_and_applies_nothing() {
        let mut g = kq();
        let e = g.apply(KQ, &[user(0, NOTE_TRIGGER)]).unwrap_err().unmark();
        assert!(e.starts_with("M48: kevent change 0: ") && e.contains("not registered and the change has no EV_ADD"), "{e}");
        let e = g.apply(KQ, &[user(EV_DELETE, 0)]).unwrap_err().unmark();
        assert!(e.contains("EV_DELETE of a knote that is not registered"), "{e}");
        let e = g.apply(KQ, &[user(EV_ADD, 0), ch(9, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0)]).unwrap_err().unmark();
        assert!(e.starts_with("M48: kevent change 1: "), "{e}");
        assert!(g.knote(KQ, ID, EVFILT_USER).is_none(), "the refused call's first change was not applied");
    }

    /// F8: a trigger activates; two before a scan are one event; `EV_CLEAR` deactivates at delivery.
    #[test]
    fn two_triggers_before_a_scan_are_one_event_and_ev_clear_resets_it() {
        let mut g = kq();
        g.apply(KQ, &[user(EV_ADD | EV_CLEAR, 0)]).unwrap();
        assert!(!g.has_events(KQ), "an add without a trigger is not active");
        g.apply(KQ, &[user(0, NOTE_TRIGGER)]).unwrap();
        g.apply(KQ, &[user(0, NOTE_TRIGGER)]).unwrap();
        assert_eq!(g.take_events(KQ, 16).unwrap().len(), 1, "two triggers before a scan are one event");
        assert!(!g.has_events(KQ));
        g.apply(KQ, &[user(0, NOTE_TRIGGER)]).unwrap();
        assert!(g.has_events(KQ), "a trigger after the reset raises a new event");
    }

    /// F8: a knote with neither `EV_CLEAR` nor `EV_ONESHOT` is re-activated after its delivery.
    #[test]
    fn a_knote_with_neither_clear_nor_oneshot_stays_active_after_delivery() {
        let mut g = kq();
        g.apply(KQ, &[user(EV_ADD, 0), user(0, NOTE_TRIGGER)]).unwrap();
        assert_eq!(g.take_events(KQ, 1).unwrap(), vec![ch(ID, EVFILT_USER, EV_ADD, 0, 0, 0)]);
        assert_eq!(g.take_events(KQ, 1).unwrap().len(), 1, "level: delivered again on the next scan");
    }

    /// F8: `EV_ONESHOT` drops the knote at its delivery.
    #[test]
    fn ev_oneshot_drops_the_knote_at_delivery() {
        let mut g = kq();
        g.apply(KQ, &[user(EV_ADD | EV_ONESHOT, 0), user(0, NOTE_TRIGGER)]).unwrap();
        assert_eq!(g.take_events(KQ, 1).unwrap(), vec![ch(ID, EVFILT_USER, EV_ADD | EV_ONESHOT, 0, 0, 0)]);
        assert!(g.knote(KQ, ID, EVFILT_USER).is_none(), "dropped");
        assert!(g.take_events(KQ, 1).unwrap().is_empty());
    }

    /// F8 (`filt_usertouch`) and K9: the `NOTE_FF*` operations act on `kn_sfflags`, a touch sets
    /// `kn_sdata` and replaces `udata`, and a delivery returns the creating flags.
    #[test]
    fn the_note_ff_operations_apply_to_the_saved_fflags_and_a_touch_sets_data_and_udata() {
        let mut g = kq();
        g.apply(KQ, &[ch(ID, EVFILT_USER, EV_ADD | EV_CLEAR, 0b1100, 0, 0x1234)]).unwrap();
        g.apply(KQ, &[ch(ID, EVFILT_USER, 0, NOTE_FFOR | 0b0011, 0, 0x1234)]).unwrap();
        assert_eq!(g.knote(KQ, ID, EVFILT_USER).unwrap().sfflags, 0b1111, "NOTE_FFOR");
        g.apply(KQ, &[ch(ID, EVFILT_USER, 0, NOTE_FFAND | 0b0110, 0, 0x1234)]).unwrap();
        assert_eq!(g.knote(KQ, ID, EVFILT_USER).unwrap().sfflags, 0b0110, "NOTE_FFAND");
        g.apply(KQ, &[ch(ID, EVFILT_USER, 0, NOTE_TRIGGER | NOTE_FFCOPY | 5, 9, 0x5678)]).unwrap();
        assert_eq!(g.take_events(KQ, 1).unwrap(), vec![ch(ID, EVFILT_USER, EV_ADD | EV_CLEAR, 5, 9, 0x5678)],
            "fflags = kn_sfflags after NOTE_FFCOPY, data = kn_sdata, udata = the trigger's");
    }

    /// `EV_DISABLE` holds an active knote back, `EV_ENABLE` releases it, `EV_DELETE` removes it.
    #[test]
    fn a_disabled_knote_is_held_until_enabled_and_ev_delete_removes_it() {
        let mut g = kq();
        g.apply(KQ, &[user(EV_ADD | EV_CLEAR | EV_DISABLE, 0), user(0, NOTE_TRIGGER)]).unwrap();
        assert!(!g.has_events(KQ), "active but disabled");
        g.apply(KQ, &[user(EV_ENABLE, 0)]).unwrap();
        assert!(g.has_events(KQ), "enabled, and still active");
        g.apply(KQ, &[user(EV_DELETE, 0)]).unwrap();
        assert!(g.knote(KQ, ID, EVFILT_USER).is_none() && !g.has_events(KQ));
    }

    /// Delivery is in activation order (`kern_event.c:knote_enqueue` appends to the active queue's
    /// tail), not in the table's ident order, and a knote past `max` stays active for the next scan.
    #[test]
    fn events_are_delivered_in_activation_order_up_to_max() {
        let mut g = kq();
        g.apply(KQ, &[ch(1, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0), ch(2, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0)]).unwrap();
        g.apply(KQ, &[ch(2, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0)]).unwrap();
        g.apply(KQ, &[ch(1, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0)]).unwrap();
        assert_eq!(g.take_events(KQ, 1).unwrap()[0].ident, 2, "ident 2 was activated first");
        let rest = g.take_events(KQ, 16).unwrap();
        assert_eq!(rest.iter().map(|e| e.ident).collect::<Vec<_>>(), vec![1], "the knote past max was kept");
    }

    /// F9 and R4: a read end is ready from one byte with `data` the count; a write end while at
    /// least `PIPE_BUF` of the buffer is free, with `data` the room. Both are level-triggered, so
    /// a delivered one re-queues at the tail of the order.
    #[test]
    fn a_pipe_read_end_is_ready_from_one_byte_and_the_write_end_while_room_is_left() {
        let mut g = kq();
        g.pipe(5, 6).unwrap();
        g.apply(KQ, &[ch(5, EVFILT_READ, EV_ADD, 0, 0, 0xabc), ch(6, EVFILT_WRITE, EV_ADD, 0, 0, 0)]).unwrap();
        let cap = PIPE_SIZE as i64;
        assert_eq!(g.take_events(KQ, 16).unwrap(), vec![ch(6, EVFILT_WRITE, EV_ADD, 0, cap, 0)],
            "an empty pipe: writable with the whole buffer, not readable");
        g.note_write(6, 5).unwrap();
        assert_eq!(g.take_events(KQ, 16).unwrap(),
            vec![ch(6, EVFILT_WRITE, EV_ADD, 0, cap - 5, 0), ch(5, EVFILT_READ, EV_ADD, 0, 5, 0xabc)],
            "the write knote re-queued at its delivery; the read knote joined behind it at the write");
        g.note_read(5, 5).unwrap();
        assert_eq!(g.pipe_count(5), Some(0));
        let filters = |g: &mut GuestKqueues| g.take_events(KQ, 16).unwrap().iter().map(|e| e.filter).collect::<Vec<_>>();
        assert_eq!(filters(&mut g), vec![EVFILT_WRITE], "drained: not readable");
        g.note_write(6, PIPE_SIZE - (PIPE_BUF - 1)).unwrap();
        assert_eq!(filters(&mut g), vec![EVFILT_READ], "PIPE_BUF - 1 bytes of room: not writable");
    }

    /// F9: the last writer's close is `EV_EOF` to the reader (a `dup`'d descriptor keeps the end
    /// open), and the reader's close is `EV_EOF` to a read filter on the write end (t0 M3).
    #[test]
    fn a_closed_writer_reports_eof_to_the_reader_and_a_closed_reader_to_the_writer() {
        let mut g = kq();
        g.pipe(5, 6).unwrap();
        g.dup(6, 7).unwrap();
        g.apply(KQ, &[ch(5, EVFILT_READ, EV_ADD, 0, 0, 0)]).unwrap();
        g.close(6).unwrap();
        assert!(!g.has_events(KQ), "fd 7 still holds the write end open");
        g.close(7).unwrap();
        assert_eq!(g.take_events(KQ, 1).unwrap(), vec![ch(5, EVFILT_READ, EV_ADD | EV_EOF, 0, 0, 0)],
            "the last writer gone: EV_EOF, data the count");
        let mut g = kq();
        g.pipe(5, 6).unwrap();
        g.apply(KQ, &[ch(6, EVFILT_READ, EV_ADD, 0, 0, 0)]).unwrap();
        assert!(!g.has_events(KQ), "a write end's read filter waits for EOF (t0 M3: `read on a pipe write end, 1 ns`)");
        g.close(5).unwrap();
        let e = g.take_events(KQ, 1).unwrap();
        assert_eq!((e[0].ident, e[0].flags & EV_EOF, e[0].data), (6, EV_EOF, 0), "t0 M3: `read on the write end, reader closed`");
    }

    /// R5 and K3: every unmodelled filter, flag, descriptor or kqueue `dup` is refused by value,
    /// naming it. libuv's `uv__stream_try_select` probe on a descriptor that is not a guest pipe is
    /// the one external shape admitted, and it is never ready.
    #[test]
    fn an_unmodelled_filter_flag_or_descriptor_is_refused_by_value() {
        let mut g = kq();
        for (c, why) in [
            (ch(1, EVFILT_TIMER, EV_ADD, 0, 0, 0), "filter -7 is not modelled"),
            (ch(1, EVFILT_USER, EV_ADD | EV_RECEIPT, 0, 0, 0), "flags 0x40 are not modelled"),
            (ch(1, EVFILT_USER, EV_ADD, NOTE_FFCOPY, 0, 0), "an EV_ADD whose fflags carry a NOTE_FF* operation"),
            (ch(1, EVFILT_USER, EV_ADD | EV_DISABLE, NOTE_TRIGGER, 0, 0), "both EV_DISABLE and NOTE_TRIGGER"),
            (ch(1, EVFILT_READ, EV_ADD, 0, 0, 0), "not a guest pipe end"),
            (ch(KQ, EVFILT_READ, EV_ADD, 0, 0, 0), "filter -1 on Kqueue is not modelled"),
        ] {
            let e = g.apply(KQ, &[c]).unwrap_err().unmark();
            assert!(e.starts_with("M48: kevent ") && e.contains(why), "{why}: {e}");
        }
        g.apply(KQ, &[ch(1, EVFILT_READ, EV_ADD | EV_ENABLE, 0, 0, 0)]).unwrap();
        assert!(!g.has_events(KQ), "§11b item 7: registered, never ready");
    }

    /// M53 t3 (t0 §M4): a dup of a kqueue's descriptor names the same kqueue, so a knote added
    /// through one is delivered through the other, and a kqueue lives until its last descriptor closes.
    #[test]
    fn a_dup_shares_the_knotes_and_the_last_close_drops_the_kqueue() {
        let mut g = kq();
        g.apply(KQ, &[user(EV_ADD | EV_CLEAR, 0)]).unwrap();
        g.dup(KQ, 9).unwrap();
        assert!(g.is_kqueue(9) && g.knote(9, ID, EVFILT_USER).is_some(), "the copy sees the knote");
        g.apply(9, &[user(0, NOTE_TRIGGER)]).unwrap();
        assert!(g.has_events(KQ), "a trigger through the copy is an event on the original");
        g.close(KQ).unwrap();
        assert!(!g.is_kqueue(KQ) && g.knote(9, ID, EVFILT_USER).is_some(), "the kqueue outlives one close");
        assert_eq!(g.take_events(9, 4).unwrap().len(), 1);
        g.close(9).unwrap();
        assert!(!g.is_kqueue(9) && g.kqs.is_empty(), "the last close drops the kqueue");
    }

    /// A close of another descriptor leaves a waiter and the knotes alone (t0 §M4); the close of the
    /// descriptor the waiter blocked through is the refusal, even with a copy alive.
    #[test]
    fn closing_a_descriptor_the_waiter_is_not_on_is_plain_and_the_waited_one_is_refused() {
        let mut g = kq();
        g.dup(KQ, 9).unwrap();
        g.apply(KQ, &[user(EV_ADD, 0)]).unwrap();
        g.set_waiter(9, Waiter { tid: 2, fd: 9, events: 0x2000, nevents: 4 });
        g.close(KQ).unwrap();
        assert_eq!(g.waiter(9).map(|w| w.tid), Some(2), "the waiter stays");
        assert!(g.knote(9, ID, EVFILT_USER).is_some(), "the knotes stay");
        g.dup(9, 11).unwrap();
        let e = g.close(9).unwrap_err().unmark();
        assert!(e.starts_with("M53: close of kqueue descriptor 9 while thread 2 waits in kevent on it"), "{e}");
        assert!(g.is_kqueue(9) && g.is_kqueue(11));
        g.close(11).unwrap();
        assert_eq!(g.waiter(9).map(|w| w.tid), Some(2));
    }

    /// `filt_userattach`: an add carrying NOTE_TRIGGER is active at once and delivers its fflags.
    #[test]
    fn an_add_carrying_note_trigger_activates_at_once() {
        let mut g = kq();
        g.apply(KQ, &[user(EV_ADD | EV_ONESHOT, NOTE_TRIGGER | 5)]).unwrap();
        assert!(g.has_events(KQ));
        let e = g.take_events(KQ, 4).unwrap();
        assert_eq!((e.len(), e[0].fflags), (1, 5));
        assert!(g.knote(KQ, ID, EVFILT_USER).is_none(), "EV_ONESHOT drops it at delivery");
    }

    /// The probe's `EV`: IN|OUT|PRI|ERR|HUP|NVAL|RDNORM|WRNORM.
    const EVERY_BIT: u16 = 0x1 | 0x4 | 0x2 | 0x8 | 0x10 | 0x20 | 0x40;

    /// t0 M7: `revents` follows the requested bits and the pipe's state, never a fixed pair.
    #[test]
    fn poll_ready_answers_each_pipe_state_by_the_requested_bits() {
        use retrace_arch::{POLLERR, POLLHUP, POLLIN, POLLOUT, POLLPRI, POLLRDBAND, POLLRDNORM, POLLWRBAND, POLLWRNORM};
        let mut g = GuestKqueues::default();
        g.pipe(5, 6).unwrap();
        for bit in [POLLIN, POLLPRI, POLLOUT, POLLRDNORM, POLLRDBAND, POLLWRNORM, POLLWRBAND, POLLERR, POLLHUP] {
            assert_eq!(g.poll_ready(5, bit).unwrap(), 0, "an empty read end, bit {bit:#x}");
        }
        g.note_write(6, 3).unwrap();
        assert_eq!(g.poll_ready(5, POLLIN).unwrap(), 0x1);
        assert_eq!(g.poll_ready(5, POLLIN | POLLOUT).unwrap(), 0x1, "POLLOUT is not reported for a read end");
        assert_eq!(g.poll_ready(5, 0).unwrap(), 0, "nothing requested, nothing reported");
        assert_eq!(g.poll_ready(5, EVERY_BIT).unwrap(), 0x43);
        assert_eq!(g.poll_ready(5, POLLRDBAND).unwrap(), 0x80);
        // The write end: room, then exactly 512 bytes of it, then 511.
        assert_eq!(g.poll_ready(6, POLLOUT).unwrap(), 0x4);
        assert_eq!(g.poll_ready(6, POLLWRBAND).unwrap(), 0x100);
        assert_eq!(g.poll_ready(6, POLLIN).unwrap(), 0);
        g.note_write(6, PIPE_SIZE_GROWN - 512 - 3).unwrap();
        assert_eq!(g.poll_ready(6, POLLOUT).unwrap(), 0x4, "512 bytes free");
        g.note_write(6, 1).unwrap();
        assert_eq!(g.poll_ready(6, POLLOUT).unwrap(), 0, "511 bytes free");
        g.note_write(6, 511).unwrap();
        assert_eq!(g.poll_ready(6, POLLOUT).unwrap(), 0, "full");
        // The writer closes: the read end reports HUP beside the bit it was asked about.
        g.close(6).unwrap();
        assert_eq!(g.poll_ready(5, POLLIN).unwrap(), 0x11);
        assert_eq!(g.poll_ready(5, POLLPRI).unwrap(), 0x12);
        assert_eq!(g.poll_ready(5, POLLRDNORM).unwrap(), 0x50);
        assert_eq!(g.poll_ready(5, POLLOUT).unwrap(), 0x10);
        assert_eq!(g.poll_ready(5, POLLHUP).unwrap(), 0x10);
        assert_eq!(g.poll_ready(5, POLLERR).unwrap(), 0, "POLLERR is never reported on a pipe");
        assert_eq!(g.poll_ready(5, 0).unwrap(), 0);
    }

    /// t0 M7: a write end whose reader closed answers POLLHUP alone for a write request, and POLLIN
    /// with it for a read request; `select` treats the same end as writable (`other_closed`).
    #[test]
    fn a_write_end_with_its_reader_closed_polls_as_hup() {
        use retrace_arch::{POLLIN, POLLOUT};
        let mut g = GuestKqueues::default();
        g.pipe(5, 6).unwrap();
        g.close(5).unwrap();
        assert_eq!(g.poll_ready(6, POLLOUT).unwrap(), 0x10);
        assert_eq!(g.poll_ready(6, POLLIN).unwrap(), 0x11);
        assert_eq!(g.poll_ready(6, 0).unwrap(), 0);
        let st = g.pipe_state(6).unwrap();
        assert!(st.write_end && st.other_closed && !st.writable);
    }

    /// A descriptor that is not a guest pipe end is the caller's refusal, named by kind.
    #[test]
    fn poll_ready_refuses_a_descriptor_that_is_not_a_guest_pipe_end() {
        let mut g = kq();
        assert_eq!(g.poll_ready(9, 1).unwrap_err(), "not a guest pipe end");
        assert_eq!(g.poll_ready(KQ, 1).unwrap_err(), "a kqueue, not a guest pipe");
        g.pipe(5, 6).unwrap();
        g.note_read(5, 1).unwrap();
        assert!(g.poll_ready(5, 1).unwrap_err().contains("byte count is unknown"), "a lost count is not guessed");
    }

    /// Ruling T4-pipesize: the buffer starts at 16,384 and grows to 65,536 once a write passes it,
    /// never shrinking; room is size - held, ready at PIPE_BUF (`pipesizeprobe.c`).
    #[test]
    fn a_pipes_buffer_grows_once_and_the_write_filter_follows_it() {
        let data = |g: &mut GuestKqueues| -> Option<i64> {
            g.take_events(KQ, 16).unwrap().iter().find(|e| e.filter == EVFILT_WRITE).map(|e| e.data)
        };
        let fresh = || { let mut g = kq(); g.pipe(5, 6).unwrap(); g.apply(KQ, &[ch(6, EVFILT_WRITE, EV_ADD, 0, 0, 0)]).unwrap(); g };
        let mut g = fresh();
        assert_eq!(data(&mut g), Some(16384));
        g.note_write(6, 15872).unwrap();
        assert_eq!(data(&mut g), Some(512), "15,872 held: ready");
        g.note_write(6, 1).unwrap();
        assert_eq!(data(&mut g), None, "15,873 held: not ready");
        g.note_write(6, 511).unwrap();
        assert_eq!(data(&mut g), None, "16,384 held: not ready");
        g.note_write(6, 1).unwrap();
        assert_eq!(data(&mut g), Some(49151), "one byte more grows it");
        g.note_write(6, 65536 - 16385).unwrap();
        assert_eq!(data(&mut g), None, "full at 65,536");
        g.note_read(5, 65536).unwrap();
        assert_eq!(data(&mut g), Some(65536), "drained after growth: it stays grown");
        g.note_write(6, 16384).unwrap();
        assert_eq!(data(&mut g), Some(49152));
        let mut g = fresh();
        g.note_write(6, 20000).unwrap();
        assert_eq!(data(&mut g), Some(45536), "one 20,000-byte write");
        let mut g = fresh();
        g.note_write(6, 16384).unwrap();
        assert_eq!(data(&mut g), None, "4 x 4096: not ready");
        g.note_write(6, 4096).unwrap();
        assert_eq!(data(&mut g), Some(45056), "a fifth succeeds");
    }
}
