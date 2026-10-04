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
/// A guest pipe's buffer: `EVFILT_WRITE`'s `data` on an empty pipe (t0 M3, `write-ready on an
/// empty pipe`; `PIPE_SIZE` in the SDK, F9). The kernel grows a buffer under a large write; the
/// model does not, and refuses a watched pipe that passes this (K1).
pub const PIPE_CAPACITY: u64 = 16384;
/// `PIPE_BUF` (SDK `sys/syslimits.h`): a write end is ready while this much room is left (F9).
pub const PIPE_BUF: u64 = 512;
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
    /// The guest VA of its event list.
    pub events: u64,
    pub nevents: usize,
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
        let lost = || format!(
            "M48: pipe {fd}: the byte count is unknown: a read returned more bytes than the model \
             counted, so a write reached the pipe by a path note_fd_effects does not see");
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
                if count > PIPE_CAPACITY {
                    return Err(format!(
                        "M48: pipe {fd}: {count} unread bytes exceed the measured {PIPE_CAPACITY}-byte \
                         buffer: the kernel grows a pipe's buffer, which the model does not (t0 M3)"));
                }
                let room = PIPE_CAPACITY - count;
                Ok((room >= PIPE_BUF).then_some((0, room as i64)))
            }
            // K3: libuv's uv__stream_try_select probe, never ready.
            (FdKind::Other, _) => Ok(None),
            (w, f) => unreachable!("M48: apply admits no filter {f} on {w:?}"),
        }
    }
}

/// Every guest kqueue, by its guest fd, and the guest's pipes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuestKqueues {
    kqs: BTreeMap<u64, Kqueue>,
    pipes: Pipes,
    /// The activation counter, one for every kqueue, so the order is total.
    seq: u64,
}

impl GuestKqueues {
    pub fn is_kqueue(&self, fd: u64) -> bool { self.kqs.contains_key(&fd) }

    pub fn kind_of(&self, fd: u64) -> FdKind {
        if self.is_kqueue(fd) { FdKind::Kqueue } else { self.pipes.kind_of(fd).unwrap_or(FdKind::Other) }
    }

    /// A forwarded `kqueue()` returned guest fd `fd`: an empty table for it (M48 §3c).
    pub fn create(&mut self, fd: u64) -> Result<(), String> {
        let k = self.kind_of(fd);
        if k != FdKind::Other {
            return Err(format!("M48: kevent on fd {fd}: kqueue() returned fd {fd}, which the model \
                                still holds as {k:?}: a close it never saw"));
        }
        self.kqs.insert(fd, Kqueue::default());
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
        self.pipes.pipes.insert(id, Pipe { count: Some(0), readers: 0, writers: 0 });
        self.pipes.bind(r, id, false);
        self.pipes.bind(w, id, true);
        Ok(())
    }

    /// `fd` closed. A kqueue's table goes with it; every knote on `fd` leaves every kqueue
    /// (`kern_event.c:knote_fdclose`); and a pipe end loses a descriptor. The close of a kqueue a
    /// thread is blocked on is refused, because the kernel's answer to that thread is unmeasured.
    pub fn close(&mut self, fd: u64) -> Result<(), String> {
        if let Some(w) = self.kqs.get(&fd).and_then(|k| k.waiter) {
            return Err(format!("M48: kevent on kq {fd}: closed while thread {} is blocked on it; the \
                                kernel's answer to the waiter is unmeasured", w.tid));
        }
        self.kqs.remove(&fd);
        for k in self.kqs.values_mut() {
            k.knotes.retain(|&(ident, filter), _| !(ident == fd && matches!(filter, EVFILT_READ | EVFILT_WRITE)));
        }
        if self.pipes.unbind(fd) { self.refresh()?; }
        Ok(())
    }

    /// `dup`, `dup2` or `F_DUPFD` made `new` a copy of `old`. Closing `new` first is `dup2`'s
    /// implicit close. A kqueue is refused (M48 §3c: the kernel's kqueue is per open file, and
    /// sharing one is unmeasured). A pipe end gains a descriptor.
    pub fn dup(&mut self, old: u64, new: u64) -> Result<(), String> {
        if old == new { return Ok(()); }
        if self.is_kqueue(old) {
            return Err(format!("M48: kevent on fd {old}: a dup of a guest kqueue to fd {new}: the \
                                kernel's kqueue is per open file and sharing one is unmeasured (M48 §3c)"));
        }
        self.close(new)?;
        if let Some(&(id, write)) = self.pipes.ends.get(&old) { self.pipes.bind(new, id, write); }
        Ok(())
    }

    /// A read from `fd` returned `n` bytes. Only a pipe's read end counts.
    pub fn note_read(&mut self, fd: u64, n: u64) -> Result<(), String> {
        let Some(&(id, false)) = self.pipes.ends.get(&fd) else { return Ok(()) };
        let p = self.pipes.pipes.get_mut(&id).expect("a bound end's pipe exists");
        p.count = p.count.and_then(|c| c.checked_sub(n));
        self.refresh()
    }

    /// A write to `fd` returned `n` bytes. Only a pipe's write end counts.
    pub fn note_write(&mut self, fd: u64, n: u64) -> Result<(), String> {
        let Some(&(id, true)) = self.pipes.ends.get(&fd) else { return Ok(()) };
        let p = self.pipes.pipes.get_mut(&id).expect("a bound end's pipe exists");
        p.count = p.count.map(|c| c + n);
        self.refresh()
    }

    pub fn pipe_count(&self, fd: u64) -> Option<u64> {
        self.pipes.ends.get(&fd).and_then(|(id, _)| self.pipes.pipes[id].count)
    }

    /// Re-evaluate every pipe knote: one that became ready joins the activation order at its tail,
    /// and one that stopped being ready leaves it (level-triggered).
    fn refresh(&mut self) -> Result<(), String> {
        let Self { kqs, pipes, seq } = self;
        for k in kqs.values_mut() {
            for (&(ident, filter), n) in k.knotes.iter_mut() {
                let Some(watch) = n.watch else { continue };
                match pipes.ready(ident, watch, filter)? {
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
        let mut k = self.kqs.get(&kq).cloned()
            .ok_or_else(|| format!("M48: kevent on fd {kq}, which is not a guest kqueue"))?;
        let mut seq = self.seq;
        for (i, c) in changes.iter().enumerate() {
            self.apply_one(&mut k, &mut seq, i, c)?;
        }
        self.kqs.insert(kq, k);
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
            return Err(format!("{at}: flags {:#x} are not modelled (EV_RECEIPT, EV_DISPATCH, \
                                EV_UDATA_SPECIFIC and the rest are unmeasured)", flags & !MODELLED));
        }
        if flags & (EV_ADD | EV_DELETE) == EV_ADD | EV_DELETE || flags & (EV_ENABLE | EV_DISABLE) == EV_ENABLE | EV_DISABLE {
            return Err(format!("{at}: contradictory flags are not modelled"));
        }
        if !matches!(c.filter, EVFILT_USER | EVFILT_READ | EVFILT_WRITE) {
            return Err(format!("{at}: filter {} is not modelled: only EVFILT_USER and the pipe filters \
                                are (M48 §3c)", c.filter));
        }
        let key = (c.ident, c.filter);
        if flags & EV_DELETE != 0 {
            return match k.knotes.remove(&key) {
                Some(_) => Ok(()),
                None => Err(format!("{at}: EV_DELETE of a knote that is not registered: the kernel \
                                     answers an ENOENT EV_ERROR event, which is not modelled")),
            };
        }
        let fd_filter = c.filter != EVFILT_USER;
        if fd_filter && (c.fflags != 0 || c.data != 0) {
            return Err(format!("{at}: a pipe filter with fflags or data (NOTE_LOWAT and the rest) is not modelled"));
        }
        let Some(n) = k.knotes.get_mut(&key) else {
            if flags & EV_ADD == 0 {
                return Err(format!("{at}: the knote is not registered and the change has no EV_ADD: \
                                    the kernel answers an ENOENT EV_ERROR event, which is not modelled"));
            }
            let mut n = Knote { flags, sfflags: 0, sdata: c.data, udata: c.udata,
                                enabled: flags & EV_DISABLE == 0, active: None, watch: None };
            if fd_filter {
                if flags & (EV_CLEAR | EV_ONESHOT) != 0 {
                    return Err(format!("{at}: EV_CLEAR or EV_ONESHOT on a pipe filter is not modelled"));
                }
                let watch = self.kind_of(c.ident);
                match (watch, c.filter) {
                    (FdKind::PipeRead(_), EVFILT_READ) | (FdKind::PipeWrite(_), EVFILT_READ | EVFILT_WRITE) => {}
                    (FdKind::Other, EVFILT_READ) if flags == EV_ADD | EV_ENABLE => {}
                    (FdKind::Other, f) => return Err(format!(
                        "M48: kevent on fd {}: change {i}'s filter {f} with flags {flags:#x} asks about a \
                         descriptor that is not a guest pipe end: readiness of a file, socket, tty or \
                         inherited descriptor is not modelled (M48 §7); only uv__stream_try_select's \
                         EVFILT_READ, EV_ADD|EV_ENABLE is answered, never ready (K3)", c.ident)),
                    (w, f) => return Err(format!("{at}: filter {f} on {w:?} is not modelled")),
                }
                n.active = match self.pipes.ready(c.ident, watch, c.filter)? {
                    Some(_) => { *seq += 1; Some(*seq) }
                    None => None,
                };
                n.watch = Some(watch);
            } else {
                if c.fflags & !NOTE_FFLAGSMASK != 0 {
                    return Err(format!("{at}: an EV_ADD whose fflags carry NOTE_TRIGGER or a NOTE_FF* \
                                        operation: what the kernel then delivers as fflags is unmeasured"));
                }
                n.sfflags = c.fflags;
            }
            k.knotes.insert(key, n);
            return Ok(());
        };
        // A touch (K9).
        let kind = flags & (EV_CLEAR | EV_ONESHOT);
        if kind != 0 && kind != n.flags & (EV_CLEAR | EV_ONESHOT) {
            return Err(format!("{at}: a touch that changes EV_CLEAR or EV_ONESHOT on a registered knote \
                                is not modelled"));
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
        self.kqs.get(&kq).is_some_and(|k| k.knotes.values().any(|n| n.enabled && n.active.is_some()))
    }

    /// Deliver up to `max` active, enabled knotes of `kq` in activation order
    /// (`kern_event.c:kqueue_process`, F8): `EV_ONESHOT` drops a knote, `EV_CLEAR` deactivates it,
    /// and any other is re-activated at the tail. A pipe knote's `data` and `EV_EOF` are read now.
    pub fn take_events(&mut self, kq: u64, max: usize) -> Result<Vec<Kevent>, String> {
        let Self { kqs, pipes, seq } = self;
        let k = kqs.get_mut(&kq).expect("take_events on a guest kqueue");
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
                    let (eof, data) = pipes.ready(key.0, w, key.1)?
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

    pub fn waiter(&self, kq: u64) -> Option<Waiter> { self.kqs.get(&kq).and_then(|k| k.waiter) }

    /// Record `w` as `kq`'s waiter. `Box_::guest_kevent` refuses a second one first (K2).
    pub fn set_waiter(&mut self, kq: u64, w: Waiter) {
        let k = self.kqs.get_mut(&kq).expect("set_waiter on a guest kqueue");
        assert!(k.waiter.is_none(), "M48: a second waiter on kq {kq}; guest_kevent refuses it first (K2)");
        k.waiter = Some(w);
    }

    pub fn take_waiter(&mut self, kq: u64) -> Option<Waiter> {
        self.kqs.get_mut(&kq).and_then(|k| k.waiter.take())
    }

    /// Every kqueue whose waiter now has an event, in kqueue fd order.
    pub fn ready_waiters(&self) -> Vec<u64> {
        self.kqs.keys().copied().filter(|&kq| self.waiter(kq).is_some() && self.has_events(kq)).collect()
    }

    pub fn knote(&self, kq: u64, ident: u64, filter: i16) -> Option<&Knote> {
        self.kqs.get(&kq).and_then(|k| k.knotes.get(&(ident, filter)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        g.set_waiter(4, Waiter { tid: 1, events: 0x1000, nevents: 1 });
        let e = g.close(4).unwrap_err();
        assert!(e.starts_with("M48: kevent on kq 4: closed while thread 1 is blocked on it"), "{e}");
        assert!(g.is_kqueue(4), "the refused close changed nothing");
        g.close(KQ).unwrap();
        assert!(!g.is_kqueue(KQ) && g.knote(KQ, ID, EVFILT_USER).is_none(), "a kqueue's close drops its table");
    }

    /// K4: the kernel answers these with an ENOENT `EV_ERROR` event, which the model never makes.
    /// A refused call applies nothing, even its admitted earlier changes.
    #[test]
    fn a_change_to_an_unregistered_knote_is_refused_and_applies_nothing() {
        let mut g = kq();
        let e = g.apply(KQ, &[user(0, NOTE_TRIGGER)]).unwrap_err();
        assert!(e.starts_with("M48: kevent change 0: ") && e.contains("not registered and the change has no EV_ADD"), "{e}");
        let e = g.apply(KQ, &[user(EV_DELETE, 0)]).unwrap_err();
        assert!(e.contains("EV_DELETE of a knote that is not registered"), "{e}");
        let e = g.apply(KQ, &[user(EV_ADD, 0), ch(9, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0)]).unwrap_err();
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
        let cap = PIPE_CAPACITY as i64;
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
        g.note_write(6, PIPE_CAPACITY - (PIPE_BUF - 1)).unwrap();
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
    fn an_unmodelled_filter_flag_descriptor_or_kqueue_dup_is_refused_by_value() {
        let mut g = kq();
        for (c, why) in [
            (ch(1, EVFILT_TIMER, EV_ADD, 0, 0, 0), "filter -7 is not modelled"),
            (ch(1, EVFILT_USER, EV_ADD | EV_RECEIPT, 0, 0, 0), "flags 0x40 are not modelled"),
            (ch(1, EVFILT_USER, EV_ADD, NOTE_TRIGGER, 0, 0), "an EV_ADD whose fflags carry NOTE_TRIGGER"),
            (ch(1, EVFILT_READ, EV_ADD, 0, 0, 0), "not a guest pipe end"),
            (ch(KQ, EVFILT_READ, EV_ADD, 0, 0, 0), "filter -1 on Kqueue is not modelled"),
        ] {
            let e = g.apply(KQ, &[c]).unwrap_err();
            assert!(e.starts_with("M48: kevent ") && e.contains(why), "{why}: {e}");
        }
        g.apply(KQ, &[ch(1, EVFILT_READ, EV_ADD | EV_ENABLE, 0, 0, 0)]).unwrap();
        assert!(!g.has_events(KQ), "§11b item 7: registered, never ready");
        let e = g.dup(KQ, 9).unwrap_err();
        assert!(e.starts_with("M48: kevent on fd 3: a dup of a guest kqueue"), "{e}");
    }
}
