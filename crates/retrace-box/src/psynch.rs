//! M48 §3e (P1): psynch condition variables, ported from libpthread-539.100.4's `kern/kern_synch.c`
//! and `kern/synch_internal.h` (plan F1). On this host every guest condvar is psynch and every
//! guest mutex firstfit psynch (plan F2), so `pthread_cond_wait` blocks in `psynch_cvwait` (305) and
//! a signal that finds a waiter issues `psynch_cvsignal` (304) or `psynch_cvbroad` (303).
//!
//! Pure data with no `Box_` access, keyed by the guest cv ADDRESS, the correlation the ulock pair
//! uses. `Box_` owns one (`Box_::psynch`), carries it through every rebuild path in `BoxState`, and
//! does what needs the box: the deadline, blocking the caller, and writing each woken thread's word
//! (`Box_::guest_psynch`, `Box_::cv_timed_out`).
//!
//! **Ported, not invented.** Each function names the kernel function it ports. Left out:
//! - `kw_cvkernelseq`, `kw_lowseq` and `kw_highseq`, which the cv paths write and never read;
//! - `kw_prepost` and `kw_intr`, which belong to the mutex and rwlock paths;
//! - cancellation (`__pthread_testcancel`, the queue scans' cancelled-thread skips): retrace models
//!   none, so no waiter is ever cancelled.
//!
//! A woken waiter's continuation (`psynch_cvcontinue`) runs at its wake (T5-b), and a cv's queue is
//! freed when it empties (T5-c). Every shape no walk measured is refused by value, before anything
//! changes, with an `Err` that starts `M48: psynch ` (R5, T5-d). The box panics with it on record,
//! and replay reports it as a `Divergence`.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use retrace_arch::{ECVCLEARED, ECVPREPOST, ETIMEDOUT, PTHRW_COUNT_MASK, PTHRW_COUNT_SHIFT, PTHRW_INC,
                   PTHRW_MAX_READERS, PTH_RWL_MTX_WAIT, PTH_RWS_CV_CBIT, PTH_RWS_CV_MBIT, PTH_RWS_CV_PBIT};

/// `PTHREAD_PSHARED_FLAGS_MASK` and `PTHREAD_PROCESS_SHARED` (kern_synch.c:187-189). A shared cv is
/// keyed by its VM object (`ksyn_findobj`), not its address.
const PSHARED_MASK: u32 = 0x30;
const PSHARED: u32 = 0x10;
/// `_psynch_cvwait` masks `nsec` before testing it (kern_synch.c:1273).
const NSEC_MASK: u32 = 0x3fff_ffff;

/// `is_seqlower` (synch_internal.h:97). Sequence words count in units of `PTHRW_INC` and wrap at
/// 2^32, so order is decided in a half window, and the low byte, which holds flag bits, is ignored.
pub fn is_seqlower(x: u32, y: u32) -> bool {
    let (x, y) = (x & PTHRW_COUNT_MASK, y & PTHRW_COUNT_MASK);
    if x < y { y - x < PTHRW_MAX_READERS / 2 } else { x - y > PTHRW_MAX_READERS / 2 }
}

/// `is_seqlower_eq` (synch_internal.h:109).
pub fn is_seqlower_eq(x: u32, y: u32) -> bool {
    x & PTHRW_COUNT_MASK == y & PTHRW_COUNT_MASK || is_seqlower(x, y)
}

/// `is_seqhigher` (synch_internal.h:119).
pub fn is_seqhigher(x: u32, y: u32) -> bool {
    let (x, y) = (x & PTHRW_COUNT_MASK, y & PTHRW_COUNT_MASK);
    if x > y { x - y < PTHRW_MAX_READERS / 2 } else { y - x > PTHRW_MAX_READERS / 2 }
}

/// `is_seqhigher_eq` (synch_internal.h:131).
pub fn is_seqhigher_eq(x: u32, y: u32) -> bool {
    x & PTHRW_COUNT_MASK == y & PTHRW_COUNT_MASK || is_seqhigher(x, y)
}

/// `diff_genseq` (synch_internal.h:141): how far `x` is ahead of `y`, across the wrap.
pub fn diff_genseq(x: u32, y: u32) -> u32 {
    let (x, y) = (x & PTHRW_COUNT_MASK, y & PTHRW_COUNT_MASK);
    match x.cmp(&y) {
        Ordering::Equal => 0,
        Ordering::Greater => x - y,
        Ordering::Less => (PTHRW_MAX_READERS - y) + x + PTHRW_INC,
    }
}

/// `_psynch_cvwait`'s timeout (kern_synch.c:1270-1280) in guest-clock ticks, or `None` to wait
/// without one, which is `pthread_cond_wait`'s `{0, 0}`. The kernel's `nsec` is 32 bits and loses its
/// top two before the zero test. At 24 MHz (plan F6) a nanosecond count is `ns * 3 / 125` ticks,
/// truncated, so node's `{0, 1}` is 0 ticks: a deadline already reached, which blocks and wakes in
/// the same schedule (Global Constraints). The box adds the clock at the call (T5-g).
pub fn timeout_ticks(sec: u64, nsec: u64) -> Result<Option<u64>, String> {
    let nsec = (nsec as u32) & NSEC_MASK;
    if sec == 0 && nsec == 0 {
        return Ok(None);
    }
    let ns = ((sec as i64) >= 0).then_some(sec)
        .and_then(|s| s.checked_mul(1_000_000_000))
        .and_then(|n| n.checked_add(nsec as u64))
        .ok_or_else(|| format!("M48: psynch psynch_cvwait timeout sec {} nsec {nsec:#x}: negative or past \
                                2^64 ns, which no guest was measured to pass", sec as i64))?;
    Ok(Some((ns as u128 * 3 / 125) as u64))
}

/// The SDK's name for psynch syscall `num` (`sys/syscall.h`), for refusals and divergences.
pub fn call_name(num: u64) -> &'static str {
    match num {
        297 => "psynch_rw_longrdlock",
        298 => "psynch_rw_yieldwrlock",
        299 => "psynch_rw_downgrade",
        300 => "psynch_rw_upgrade",
        301 => "psynch_mutexwait",
        302 => "psynch_mutexdrop",
        303 => "psynch_cvbroad",
        304 => "psynch_cvsignal",
        305 => "psynch_cvwait",
        306 => "psynch_rw_rdlock",
        307 => "psynch_rw_wrlock",
        308 => "psynch_rw_unlock",
        309 => "psynch_rw_unlock2",
        312 => "psynch_cvclrprepost",
        _ => "psynch",
    }
}

/// `kwe_state` (synch_internal.h:28).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KweState {
    /// `KWE_THREAD_INWAIT`: a blocked `cvwait`.
    InWait,
    /// `KWE_THREAD_PREPOST`: a signal that found no waiter at or below its sequence.
    Prepost,
    /// `KWE_THREAD_BROADCAST`: a broadcast's claim on waiters not yet in the kernel.
    Broadcast,
}

/// One `ksyn_waitq_element` (kern_internal.h:141), for the fields a cv reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kwe {
    pub state: KweState,
    /// `kwe_lockseq`, count bits only, as every cv path stores it.
    pub lockseq: u32,
    /// `kwe_count`: the signals a prepost still owes.
    pub count: u32,
    /// `kwe_thread` as a thread-table index; `None` for a prepost or a broadcast entry.
    pub thread: Option<usize>,
}

/// One cv's `ksyn_wait_queue` (kern_synch.c:125), for the fields its paths read. A cv uses only
/// `kw_ksynqueues[KSYN_QUEUE_WRITE]`, so there is one queue, kept in TAILQ order, which `SEQFIT`
/// keeps in sequence order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kwq {
    pub lword: u32,
    pub uword: u32,
    pub sword: u32,
    /// `KSYN_KWF_ZEROEDOUT`: L, U and S were cleared at an L == S transition, so the next call's
    /// words replace them outright (`UPDATE_CVKWQ`).
    pub zeroed_out: bool,
    pub queue: Vec<Kwe>,
    /// `kw_fakecount`: the prepost and broadcast entries in `queue`.
    pub fakecount: u32,
}

/// A woken waiter and the word its `cvwait` returns, with carry clear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wake {
    pub tid: usize,
    pub word: u32,
}

/// What a call answers its caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ret {
    /// The call returns this word at once, with carry clear.
    Word(u32),
    /// The caller blocks (`cvwait` only); its word comes at its wake.
    Block,
}

/// One call's answer, and the waiters it woke, in the order the kernel woke them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub ret: Ret,
    pub woken: Vec<Wake>,
}

/// A deadline wake: the timed-out waiter's errno word, delivered with carry set, and any waiter the
/// balancing L == S released with it. The word is `u64`, as `ETIMEDOUT`, `ECVCLEARED` and
/// `ECVPREPOST` are (Task 2): an errno the stub's caller reads whole (plan F11).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimedOut {
    pub word: u64,
    pub woken: Vec<Wake>,
}

/// `psynch_cvcontinue`'s success branch (kern_synch.c:1349-1358): a signalled waiter returns 0,
/// unless the kernel woke it while freeing the queue (`PTH_RWS_CV_MBIT`), when it returns one
/// increment with the C bit.
fn woken_word(psynchretval: u32) -> u32 {
    if psynchretval & PTH_RWS_CV_MBIT != 0 { PTHRW_INC | PTH_RWS_CV_CBIT } else { 0 }
}

fn refuse_shared(call: &str, cv: u64, flags: u32) -> Result<(), String> {
    if flags & PSHARED_MASK == PSHARED {
        return Err(format!("M48: psynch {call} on cv {cv:#x} with flags {flags:#x}: a process-shared cv is keyed \
                            by its VM object (ksyn_findobj), not its address; every measured flags word is 0xa0 \
                            (plan P4)"));
    }
    Ok(())
}

impl Kwq {
    /// `ksyn_wqfind`'s first use of an address (kern_synch.c:1799-1815): L, U and S from the call.
    fn new(lword: u32, uword: u32, sword: u32) -> Kwq {
        Kwq { lword, uword, sword, ..Kwq::default() }
    }

    /// `UPDATE_CVKWQ` (kern_synch.c:290). Its `kw_cvkernelseq` write is dropped: no cv path reads it.
    fn update(&mut self, mgen: u32, ugen: u32, rw_wc: u32) {
        let sinit = rw_wc & PTH_RWS_CV_CBIT != 0;
        if self.zeroed_out {
            (self.lword, self.uword, self.sword, self.zeroed_out) = (mgen, ugen, rw_wc, false);
        } else {
            if is_seqhigher(mgen, self.lword) { self.lword = mgen; }
            if is_seqhigher(ugen, self.uword) { self.uword = ugen; }
            if sinit && is_seqhigher(rw_wc, self.sword) { self.sword = rw_wc; }
        }
    }

    /// The L == S clearing `ksyn_cvupdate_fixup` and the timeout branch share.
    fn clear(&mut self) {
        (self.lword, self.uword, self.sword, self.zeroed_out) = (0, 0, 0, true);
    }

    /// `ksyn_queue_insert` with `SEQFIT` (kern_synch.c:2358), the only fit a cv uses. A second entry
    /// at the first or last sequence is the kernel's `EBUSY`, and a gap it cannot place its `ESRCH`.
    fn insert(&mut self, kwe: Kwe) -> Result<(), String> {
        let seq = kwe.lockseq;
        let at = match (self.queue.first(), self.queue.last()) {
            (Some(f), Some(l)) if seq == f.lockseq || seq == l.lockseq => {
                return Err(format!("an entry at sequence {seq:#x} is already queued (ksyn_queue_insert's EBUSY)"));
            }
            (Some(f), Some(l)) => {
                if is_seqlower(l.lockseq, seq) {
                    self.queue.len()
                } else if is_seqlower(seq, f.lockseq) {
                    0
                } else {
                    self.queue.iter().position(|q| is_seqhigher(q.lockseq, seq)).ok_or_else(|| format!(
                        "no entry above sequence {seq:#x} to insert before (ksyn_queue_insert's ESRCH)"))?
                }
            }
            _ => 0,
        };
        self.queue.insert(at, kwe);
        Ok(())
    }

    /// `ksyn_prepost` (kern_synch.c:907): a fake entry, counted in `fakecount`. The kernel ignores a
    /// failed insert; here it is refused, since the entry would be counted but never queued.
    fn prepost(&mut self, state: KweState, lockseq: u32) -> Result<(), String> {
        self.insert(Kwe { state, lockseq, count: 1, thread: None })?;
        self.fakecount += 1;
        Ok(())
    }

    /// `ksyn_queue_find_cvpreposeq` (kern_synch.c:2482): the first entry at or above `lockseq`,
    /// unless it is a waiter at another sequence.
    fn find_cvpreposeq(&self, lockseq: u32) -> Option<usize> {
        let i = self.queue.iter().position(|k| is_seqhigher_eq(k.lockseq, lockseq))?;
        let k = self.queue[i];
        (k.state != KweState::InWait || k.lockseq == lockseq).then_some(i)
    }

    /// `ksyn_queue_find_signalseq` (kern_synch.c:2505): a prepost or broadcast at or above
    /// `uptoseq`, else the waiter at or above `signalseq`, else the first waiter at or below
    /// `uptoseq`.
    fn find_signalseq(&self, uptoseq: u32, signalseq: u32) -> Option<usize> {
        let mut result = None;
        for (i, q) in self.queue.iter().enumerate() {
            match q.state {
                KweState::Prepost if is_seqhigher(q.lockseq, uptoseq) => return result,
                KweState::Prepost | KweState::Broadcast => {
                    if !is_seqlower(q.lockseq, uptoseq) {
                        return Some(i);
                    }
                }
                KweState::InWait => {
                    if is_seqhigher(q.lockseq, uptoseq) {
                        return result;
                    }
                    if is_seqhigher_eq(q.lockseq, signalseq) {
                        return Some(i);
                    }
                    result = result.or(Some(i));
                }
            }
        }
        result
    }

    /// `_ksyn_cvsignal_any` (kern_synch.c:920).
    fn signal_any(&mut self, uptoseq: u32, signalseq: u32, updatebits: &mut u32, broadcast: &mut bool,
                  woken: &mut Vec<Wake>) -> Result<(), String> {
        let Some(i) = self.find_signalseq(uptoseq, signalseq) else {
            return self.prepost(KweState::Prepost, uptoseq);
        };
        match self.queue[i].state {
            // A waiter below the signal's own sequence: matching it could leave the waiter the signal
            // was meant for with nobody to wake it, so the kernel converts to a broadcast
            // (kern_synch.c:950-960).
            KweState::InWait if is_seqlower(self.queue[i].lockseq, signalseq) => *broadcast = true,
            KweState::InWait => {
                let kwe = self.queue.remove(i);
                woken.push(Wake { tid: kwe.thread.expect("an InWait entry names its thread"), word: woken_word(PTH_RWL_MTX_WAIT) });
                *updatebits += PTHRW_INC;
            }
            KweState::Prepost => self.queue[i].count += 1,
            KweState::Broadcast => {}
        }
        Ok(())
    }

    /// `ksyn_handle_cvbroad` (kern_synch.c:2714): wake every waiter up to `upto` and drop every fake
    /// entry there; then, unless L == S, queue a broadcast entry for waiters not yet in the kernel.
    /// S is read before the caller adds this call's count, as in the kernel.
    fn broadcast(&mut self, upto: u32, updatebits: &mut u32, woken: &mut Vec<Wake>) -> Result<(), String> {
        let mut bits = 0;
        // Each entry it takes leaves the queue (`ksyn_signal` removes a waiter), so the scan always
        // reads the head.
        while let Some(&kwe) = self.queue.first() {
            if is_seqhigher(kwe.lockseq, upto) {
                break;
            }
            self.queue.remove(0);
            match kwe.state {
                KweState::InWait => {
                    woken.push(Wake { tid: kwe.thread.expect("an InWait entry names its thread"), word: woken_word(PTH_RWL_MTX_WAIT) });
                    bits += PTHRW_INC;
                }
                KweState::Prepost | KweState::Broadcast => self.fakecount -= 1,
            }
        }
        if diff_genseq(self.lword, self.sword) != 0 {
            self.prepost(KweState::Broadcast, upto)?;
        }
        *updatebits |= bits;
        Ok(())
    }

    /// `ksyn_queue_free_items` (kern_synch.c:2556): from the head, up to `upto` unless `all`. A
    /// waiter is woken as freed (`PTHRW_INC | PTH_RWS_CV_MBIT | PTH_RWL_MTX_WAIT`); a fake entry is
    /// dropped.
    fn free_items(&mut self, upto: u32, all: bool, woken: &mut Vec<Wake>) {
        while let Some(&kwe) = self.queue.first() {
            if !all && is_seqhigher(kwe.lockseq, upto) {
                break;
            }
            self.queue.remove(0);
            match kwe.state {
                KweState::InWait => woken.push(Wake {
                    tid: kwe.thread.expect("an InWait entry names its thread"),
                    word: woken_word(PTHRW_INC | PTH_RWS_CV_MBIT | PTH_RWL_MTX_WAIT),
                }),
                KweState::Prepost | KweState::Broadcast => self.fakecount -= 1,
            }
        }
    }

    /// `ksyn_cvupdate_fixup` (kern_synch.c:2787): at L == S, free the queue up to L, clear the words
    /// and answer the C bit; with only fake entries left, answer the P bit.
    fn fixup(&mut self, updatebits: &mut u32, woken: &mut Vec<Wake>) {
        if self.lword & PTHRW_COUNT_MASK == self.sword & PTHRW_COUNT_MASK {
            if !self.queue.is_empty() {
                let l = self.lword;
                self.free_items(l, false, woken);
            }
            self.clear();
            *updatebits |= PTH_RWS_CV_CBIT;
        } else if !self.queue.is_empty() && self.fakecount as usize == self.queue.len() {
            *updatebits |= PTH_RWS_CV_PBIT;
        }
    }
}

/// Every cv with a nonempty queue, by guest address.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Psynch {
    kwqs: BTreeMap<u64, Kwq>,
}

impl Psynch {
    pub fn is_empty(&self) -> bool { self.kwqs.is_empty() }

    pub fn kwq(&self, cv: u64) -> Option<&Kwq> { self.kwqs.get(&cv) }

    /// `ksyn_wqfind`'s lookup, or a fresh queue from the call's words. A copy: every operation
    /// works on it and stores it back only on success, so a refusal changes nothing.
    fn find(&self, cv: u64, lword: u32, uword: u32, sword: u32) -> Kwq {
        self.kwqs.get(&cv).cloned().unwrap_or_else(|| Kwq::new(lword, uword, sword))
    }

    /// `ksyn_wqrelease` with `qfreenow` (kern_synch.c:1835): an empty queue is freed (T5-c).
    fn store(&mut self, cv: u64, kwq: Kwq) {
        if kwq.queue.is_empty() {
            self.kwqs.remove(&cv);
        } else {
            self.kwqs.insert(cv, kwq);
        }
    }

    /// `_psynch_cvwait` (kern_synch.c:1171) for thread `tid`. `args` is the syscall's: `cv, cvlsgen,
    /// cvugen, mutex, mugen, flags, sec, nsec`; the timeout is the box's (`timeout_ticks`).
    pub fn cvwait(&mut self, args: [u64; 8], tid: usize) -> Result<Outcome, String> {
        let (cv, cvugen, mutex, flags) = (args[0], args[2] as u32, args[3], args[5] as u32);
        let (csgen, cgen) = ((args[1] >> 32) as u32, args[1] as u32);
        refuse_shared("psynch_cvwait", cv, flags)?;
        if mutex != 0 {
            return Err(format!("M48: psynch psynch_cvwait on cv {cv:#x} with mutex {mutex:#x} (mugen {:#x}): the \
                                cv would drop a firstfit mutex that has a kernel waiter (plan F3), which needs the \
                                unmodelled psynch_mutexwait/psynch_mutexdrop pair; every measured cvwait passes \
                                mutex 0 (plan P4)", args[4]));
        }
        let lockseq = cgen & PTHRW_COUNT_MASK;
        if is_seqhigher_eq(csgen, lockseq) {
            return Err(format!("M48: psynch psynch_cvwait on cv {cv:#x}: S {csgen:#x} is not below L {cgen:#x}, \
                                which _psynch_cvwait answers EINVAL; no guest was measured to pass it"));
        }
        let mut kwq = self.find(cv, cgen, cvugen, csgen);
        kwq.update(cgen, cvugen, csgen);
        let mut woken = Vec::new();
        let ret = match kwq.find_cvpreposeq(lockseq) {
            None => {
                kwq.insert(Kwe { state: KweState::InWait, lockseq, count: 1, thread: Some(tid) })
                    .map_err(|e| format!("M48: psynch psynch_cvwait on cv {cv:#x} by thread {tid}: {e}"))?;
                Ret::Block
            }
            Some(i) => {
                let mut updatebits = 0;
                let kwe = kwq.queue[i];
                match kwe.state {
                    KweState::InWait => return Err(format!(
                        "M48: psynch psynch_cvwait on cv {cv:#x} by thread {tid}: thread {} already waits at \
                         sequence {lockseq:#x}, which _psynch_cvwait answers EBUSY",
                        kwe.thread.expect("an InWait entry names its thread"))),
                    // A prepost at our own sequence: consume one of its references.
                    KweState::Prepost if kwe.lockseq == lockseq => {
                        kwq.queue[i].count -= 1;
                        if kwq.queue[i].count == 0 {
                            kwq.queue.remove(i);
                            kwq.fakecount -= 1;
                        }
                    }
                    // A prepost above our sequence can leave its own waiter unmatched, so the kernel
                    // converts it to a broadcast (kern_synch.c:1233-1244).
                    KweState::Prepost => kwq.broadcast(kwe.lockseq, &mut updatebits, &mut woken)
                        .map_err(|e| format!("M48: psynch psynch_cvwait on cv {cv:#x} by thread {tid}: {e}"))?,
                    KweState::Broadcast => {}
                }
                updatebits |= PTHRW_INC;
                kwq.sword = kwq.sword.wrapping_add(PTHRW_INC);
                kwq.fixup(&mut updatebits, &mut woken);
                Ret::Word(updatebits)
            }
        };
        self.store(cv, kwq);
        Ok(Outcome { ret, woken })
    }

    /// `_psynch_cvsignal` (kern_synch.c:1156). `args`: `cv, cvlsgen, cvugen, thread_port, mutex,
    /// mugen, tid, flags`.
    pub fn cvsignal(&mut self, args: [u64; 8]) -> Result<Outcome, String> {
        let (cv, port, flags) = (args[0], args[3] as u32, args[7] as u32);
        refuse_shared("psynch_cvsignal", cv, flags)?;
        if port != 0 {
            return Err(format!("M48: psynch psynch_cvsignal on cv {cv:#x} targets thread port {port:#x} \
                                (pthread_cond_signal_thread_np): the targeted form was never measured (plan P4)"));
        }
        self.signal("psynch_cvsignal", cv, args[1] as u32, args[2] as u32, (args[1] >> 32) as u32, false)
    }

    /// `_psynch_cvbroad` (kern_synch.c:1134). `args`: `cv, cvlsgen, cvudgen, flags, mutex, mugen,
    /// tid`, where `cvudgen` is the old U over the count of waiters being released.
    pub fn cvbroad(&mut self, args: [u64; 8], nthreads: usize) -> Result<Outcome, String> {
        let (cv, flags) = (args[0], args[3] as u32);
        refuse_shared("psynch_cvbroad", cv, flags)?;
        let count = (args[2] as u32) >> PTHRW_COUNT_SHIFT;
        // The kernel's bound is `get_task_threadmax()`, a host value retrace does not model. A
        // guest's count is its unreleased waiters, fewer than its threads, so the guest's own thread
        // count refuses only a count no guest can produce (T5-d).
        if count as usize > nthreads {
            return Err(format!("M48: psynch psynch_cvbroad on cv {cv:#x} releases {count} waiters, more than the \
                                guest's {nthreads} threads (the kernel answers EBUSY above task_threadmax)"));
        }
        self.signal("psynch_cvbroad", cv, args[1] as u32, (args[2] >> 32) as u32, (args[1] >> 32) as u32, true)
    }

    /// `__psynch_cvsignal` (kern_synch.c:1054) with no thread port.
    fn signal(&mut self, call: &str, cv: u64, cgen: u32, cugen: u32, csgen: u32, mut broadcast: bool)
              -> Result<Outcome, String> {
        let uptoseq = cgen & PTHRW_COUNT_MASK;
        let fromseq = (cugen & PTHRW_COUNT_MASK).wrapping_add(PTHRW_INC);
        if is_seqhigher(fromseq, uptoseq) || is_seqhigher(csgen, uptoseq) {
            return Err(format!("M48: psynch {call} on cv {cv:#x}: L {cgen:#x}, U {cugen:#x}, S {csgen:#x} are out \
                                of order, which __psynch_cvsignal answers EINVAL; no guest was measured to pass them"));
        }
        let at = |e: String| format!("M48: psynch {call} on cv {cv:#x}: {e}");
        let mut kwq = self.find(cv, cgen, cugen, csgen);
        kwq.update(cgen, cugen, csgen);
        let (mut updatebits, mut woken) = (0, Vec::new());
        // "No need to signal if the CV is already balanced" (kern_synch.c:1092).
        if !broadcast && diff_genseq(kwq.lword, kwq.sword) != 0 {
            kwq.signal_any(uptoseq, fromseq, &mut updatebits, &mut broadcast, &mut woken).map_err(&at)?;
        }
        if broadcast {
            kwq.broadcast(uptoseq, &mut updatebits, &mut woken).map_err(&at)?;
        }
        kwq.sword = kwq.sword.wrapping_add(updatebits & PTHRW_COUNT_MASK);
        kwq.fixup(&mut updatebits, &mut woken);
        self.store(cv, kwq);
        Ok(Outcome { ret: Ret::Word(updatebits), woken })
    }

    /// `psynch_cvcontinue`'s timeout branch (kern_synch.c:1309-1347) for `tid`'s deadline. The
    /// waiter leaves the queue unsignalled, so it counts itself in S. When that balances L and S the
    /// cv is cleared and the errno carries `ECVCLEARED`; when only fake entries remain it carries
    /// `ECVPREPOST`. libpthread reads both bits (plan F11).
    ///
    /// Panics if `tid` does not wait on `cv`: `wake_due_threads` reaches here only for a thread
    /// blocked on `cv` with a deadline, so that is a box defect, not a guest shape.
    pub fn time_out(&mut self, cv: u64, tid: usize) -> TimedOut {
        let mut kwq = self.kwqs.get(&cv).cloned()
            .unwrap_or_else(|| panic!("M48: psynch deadline of thread {tid} on cv {cv:#x}, which has no queue"));
        let i = kwq.queue.iter().position(|k| k.state == KweState::InWait && k.thread == Some(tid))
            .unwrap_or_else(|| panic!("M48: psynch deadline of thread {tid} on cv {cv:#x}, where it does not wait: {kwq:?}"));
        kwq.queue.remove(i);
        let (mut word, mut woken) = (ETIMEDOUT, Vec::new());
        kwq.sword = kwq.sword.wrapping_add(PTHRW_INC);
        if kwq.lword & PTHRW_COUNT_MASK == kwq.sword & PTHRW_COUNT_MASK {
            word |= ECVCLEARED;
            if !kwq.queue.is_empty() {
                let l = kwq.lword;
                kwq.free_items(l, true, &mut woken);
            }
            kwq.clear();
        } else if !kwq.queue.is_empty() && kwq.fakecount as usize == kwq.queue.len() {
            word |= ECVPREPOST;
        }
        self.store(cv, kwq);
        TimedOut { word, woken }
    }

    /// Test-only (Review Focus 5, T5-f): plant a waiter on `cv` at `lockseq`, the state an earlier
    /// silent divergence would leave, so a test can make replay refuse a call the recording accepted.
    #[doc(hidden)]
    pub fn dbg_plant_waiter(&mut self, cv: u64, lockseq: u32, tid: usize) {
        let lockseq = lockseq & PTHRW_COUNT_MASK;
        let kwq = self.kwqs.entry(cv).or_insert_with(|| Kwq::new(lockseq, 0, 0));
        kwq.insert(Kwe { state: KweState::InWait, lockseq, count: 1, thread: Some(tid) })
            .expect("a planted waiter must fit the queue");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CV: u64 = 0x1_0000_8000;

    /// `cvwait`'s arguments as libpthread builds them: `cvlsgen` is S (with the bits it saved)
    /// over L after this waiter's increment, then U, mutex 0, mugen 0, node's flags 0xa0 (plan
    /// P4), and no timeout, which is the box's (`timeout_ticks`).
    fn wait(l: u32, s: u32, u: u32) -> [u64; 8] {
        [CV, ((s as u64) << 32) | l as u64, u as u64, 0, 0, 0xa0, 0, 0]
    }

    /// `cvsignal`'s: `cvlsgen` as above, U before the signal's own increment, no thread port, and
    /// the flags in x7.
    fn signal(l: u32, s: u32, u: u32) -> [u64; 8] {
        [CV, ((s as u64) << 32) | l as u64, u as u64, 0, 0, 0, 0, 0xa0]
    }

    /// `cvbroad`'s: `cvudgen` is the old U over the count of waiters being released.
    fn broad(l: u32, s: u32, u: u32, diffgen: u32) -> [u64; 8] {
        [CV, ((s as u64) << 32) | l as u64, ((u as u64) << 32) | diffgen as u64, 0xa0, 0, 0, 0, 0]
    }

    fn woke(tid: usize) -> Wake { Wake { tid, word: 0 } }

    /// Review Focus 3. synch_internal.h compares the count bits in a half window, so a word just
    /// past the 2^32 wrap is HIGHER than one just before it. A model comparing raw integers gets
    /// every row that crosses the wrap backwards.
    #[test]
    fn the_sequence_window_wraps_as_synch_internal_h_computes() {
        assert!(is_seqhigher(0x100, 0xffff_ff00) && is_seqlower(0xffff_ff00, 0x100), "one step across the wrap");
        assert!(!is_seqhigher(0xffff_ff00, 0x100) && !is_seqlower(0x100, 0xffff_ff00));
        assert!(!is_seqhigher_eq(0xffff_ff01, 0), "S just below the wrap is not at or above L just past it");
        assert_eq!(diff_genseq(0, 0xffff_ff00), PTHRW_INC, "L one increment past S, across the wrap");
        assert_eq!(diff_genseq(0x100, 0xffff_ff00), 0x200);
        assert_eq!(diff_genseq(0xffff_ff00, 0x100), 0xffff_fe00, "the other way is nearly the whole space");
        // The low byte is flag bits, not count.
        assert!(is_seqlower_eq(0x1ff, 0x100) && !is_seqhigher(0x1ff, 0x100));
        assert_eq!(diff_genseq(0x3ff, 0x101), 0x200);
        assert_eq!(diff_genseq(0x101, 0x1ff), 0, "equal counts with different bits are balanced");
        // The half window's edge: PTHRW_MAX_READERS / 2 is 0x7fff_ff80.
        assert!(is_seqhigher(0x7fff_ff00, 0), "inside the half window");
        assert!(!is_seqhigher(0x8000_0000, 0) && is_seqlower(0x8000_0000, 0), "past it, so lower");
    }

    /// Review Focus 3 through the port. A cv balanced just below the wrap (L = U = 0xffff_ff00, S the
    /// same with its C bit) takes one waiter, whose increment wraps L to 0, and one signal, whose
    /// increment wraps U to 0. The waiter is woken and the cv cleared exactly as for an unwrapped
    /// cv. A raw-integer model refuses the wait (S "above" L) or finds nobody to wake.
    #[test]
    fn a_signal_across_the_sequence_wrap_wakes_the_waiter() {
        let mut p = Psynch::default();
        assert_eq!(p.cvwait(wait(0, 0xffff_ff01, 0xffff_ff00), 1).unwrap(), Outcome { ret: Ret::Block, woken: vec![] });
        assert_eq!(p.kwq(CV).unwrap().queue, vec![Kwe { state: KweState::InWait, lockseq: 0, count: 1, thread: Some(1) }]);
        // libpthread stored S without its bits at the wait; the signal passes the old U.
        let s = p.cvsignal(signal(0, 0xffff_ff00, 0xffff_ff00)).unwrap();
        assert_eq!(s, Outcome { ret: Ret::Word(0x101), woken: vec![woke(1)] },
            "T0(M4): one waiter woken, and S, wrapping to 0, balances L, so the cv is cleared");
        assert!(p.is_empty(), "a cleared cv with an empty queue is freed (T5-c)");
    }

    /// `__psynch_cvsignal` on a fresh cv with one waiter.
    #[test]
    fn a_signal_wakes_the_one_waiter_and_answers_one_increment_with_the_c_bit() {
        let mut p = Psynch::default();
        // A fresh cv's S is 1 (its C bit, set at init), so the first waiter passes S = 1, L = 0x100.
        assert_eq!(p.cvwait(wait(0x100, 1, 0), 1).unwrap().ret, Ret::Block);
        assert_eq!(p.kwq(CV).unwrap().sword, 1, "a fresh queue takes the caller's S, bits included (ksyn_wqfind)");
        let s = p.cvsignal(signal(0x100, 0, 0)).unwrap();
        assert_eq!(s.ret, Ret::Word(0x101), "T0(M4): one increment, and the C bit because S now equals L");
        assert_eq!(s.woken, vec![woke(1)], "a signalled waiter returns 0 (psynch_cvcontinue)");
        assert!(p.is_empty());
    }

    /// `ksyn_handle_cvbroad`: every waiter up to L, in queue order.
    #[test]
    fn a_broadcast_wakes_every_waiter_in_sequence_order_and_answers_their_count() {
        let mut p = Psynch::default();
        for (tid, l, s) in [(1, 0x100, 1), (2, 0x200, 0), (3, 0x300, 0)] {
            assert_eq!(p.cvwait(wait(l, s, 0), tid).unwrap().ret, Ret::Block, "thread {tid}");
        }
        let b = p.cvbroad(broad(0x300, 0, 0, 0x300), 4).unwrap();
        assert_eq!(b.ret, Ret::Word(0x301), "T0(M4): three increments and the C bit");
        assert_eq!(b.woken, vec![woke(1), woke(2), woke(3)]);
        assert!(p.is_empty(), "the broadcast entry ksyn_handle_cvbroad queued is freed by the L == S fixup");
    }

    /// `ksyn_queue_find_signalseq` prefers the waiter at the signal's own sequence (U + 1), which
    /// is what keeps two waiters in arrival order.
    #[test]
    fn a_signal_wakes_the_lowest_sequence_waiter_first() {
        let mut p = Psynch::default();
        p.cvwait(wait(0x100, 1, 0), 1).unwrap();
        p.cvwait(wait(0x200, 0, 0), 2).unwrap();
        assert_eq!(p.cvsignal(signal(0x200, 0, 0)).unwrap(), Outcome { ret: Ret::Word(0x100), woken: vec![woke(1)] },
            "the waiter at U + 1, and no C bit: thread 2 still waits");
        assert_eq!(p.kwq(CV).unwrap().sword, 0x101);
        // libpthread added the 0x100 to its S; the second signal advances U past the first.
        assert_eq!(p.cvsignal(signal(0x200, 0x100, 0x100)).unwrap(), Outcome { ret: Ret::Word(0x101), woken: vec![woke(2)] });
        assert!(p.is_empty());
    }

    /// `psynch_cvcontinue`'s timeout branch for the only waiter.
    #[test]
    fn a_deadline_on_a_lone_waiter_answers_etimedout_with_ecvcleared() {
        let mut p = Psynch::default();
        p.cvwait(wait(0x100, 1, 0), 1).unwrap();
        assert_eq!(p.time_out(CV, 1), TimedOut { word: 0x13c, woken: vec![] }, "T0(M4): ETIMEDOUT | ECVCLEARED (plan F11)");
        assert!(p.is_empty(), "its own count balanced L and S, so the cv is cleared and freed");
    }

    /// The same branch beside a second waiter: S counts the leaver, L stays ahead.
    #[test]
    fn a_deadline_beside_another_waiter_answers_plain_etimedout() {
        let mut p = Psynch::default();
        p.cvwait(wait(0x100, 1, 0), 1).unwrap();
        p.cvwait(wait(0x200, 0, 0), 2).unwrap();
        assert_eq!(p.time_out(CV, 1), TimedOut { word: 60, woken: vec![] },
            "nothing cleared, and a real waiter remains, so no ECVPREPOST");
        assert_eq!(p.kwq(CV).unwrap().queue.len(), 1);
        // libpthread counted the timeout in its own S (_pthread_cond_updateval), so U catches up from S.
        assert_eq!(p.cvsignal(signal(0x200, 0x100, 0)).unwrap(), Outcome { ret: Ret::Word(0x101), woken: vec![woke(2)] });
        assert!(p.is_empty());
    }

    /// `_ksyn_cvsignal_any` with no waiter at or below its sequence leaves a prepost, and the
    /// waiter it was meant for consumes it without blocking (`_psynch_cvwait`). Natively that waiter
    /// had incremented L and not yet entered the kernel; under the cooperative scheduler it follows
    /// a deadline wake whose thread has not yet run (T5-b).
    #[test]
    fn a_signal_with_no_waiter_preposts_and_the_next_wait_consumes_it() {
        let mut p = Psynch::default();
        assert_eq!(p.cvsignal(signal(0x100, 0, 0)).unwrap(), Outcome { ret: Ret::Word(PTH_RWS_CV_PBIT), woken: vec![] },
            "nothing woken, and only a fake entry queued: the P bit");
        assert_eq!(p.kwq(CV).unwrap().queue, vec![Kwe { state: KweState::Prepost, lockseq: 0x100, count: 1, thread: None }]);
        assert_eq!(p.cvwait(wait(0x100, 1, 0), 1).unwrap(), Outcome { ret: Ret::Word(0x101), woken: vec![] },
            "consumed at once: no block");
        assert!(p.is_empty());
    }

    /// `_ksyn_cvsignal_any`'s starvation guard: the only waiter sits below the signal's sequence, so
    /// the signal becomes a broadcast. It wakes that waiter and queues a broadcast entry for the
    /// waiter still on its way, which then returns without blocking.
    #[test]
    fn a_signal_whose_only_waiter_is_below_its_sequence_becomes_a_broadcast() {
        let mut p = Psynch::default();
        p.cvwait(wait(0x100, 1, 0), 1).unwrap();
        // U already counts the waiter at 0x100, and L counts a second that has not reached the kernel.
        assert_eq!(p.cvsignal(signal(0x200, 0, 0x100)).unwrap(),
            Outcome { ret: Ret::Word(PTHRW_INC | PTH_RWS_CV_PBIT), woken: vec![woke(1)] });
        assert_eq!(p.kwq(CV).unwrap().queue, vec![Kwe { state: KweState::Broadcast, lockseq: 0x200, count: 1, thread: None }]);
        assert_eq!(p.cvwait(wait(0x200, 0, 0), 2).unwrap(), Outcome { ret: Ret::Word(0x101), woken: vec![] },
            "the broadcast entry covers it");
        assert!(p.is_empty());
    }

    /// `_psynch_cvwait`'s timeout arithmetic (kern_synch.c:1270-1280) at 24 MHz (plan F6).
    #[test]
    fn the_timeout_decodes_as_kern_synch_does() {
        assert_eq!(timeout_ticks(0, 0), Ok(None), "pthread_cond_wait's {{0, 0}}: no deadline");
        assert_eq!(timeout_ticks(0, 1), Ok(Some(0)), "node's {{0, 1 ns}} is 0 ticks: a deadline already reached");
        assert_eq!(timeout_ticks(0, 5_000_000), Ok(Some(120_000)), "5 ms");
        assert_eq!(timeout_ticks(1, 986_000_000), Ok(Some(47_664_000)), "1.986 s");
        assert_eq!(timeout_ticks(0, 0x4000_0000), Ok(None), "nsec loses its top two bits before the zero test");
        assert_eq!(timeout_ticks(0, 0xffff_ffff_0000_0001), Ok(Some(0)), "the kernel's nsec is 32 bits");
        let e = timeout_ticks(u64::MAX, 0).unwrap_err();
        assert!(e.starts_with("M48: psynch ") && e.contains("sec -1"), "{e}");
    }

    /// R5 and T5-d: every shape no walk measured is refused by value, naming it, and the model is
    /// left as it was. One row per refusal, so deleting any one of them turns this test red.
    #[test]
    fn every_unmeasured_shape_is_refused_by_value_and_changes_nothing() {
        let mut p = Psynch::default();
        p.cvwait(wait(0x100, 1, 0), 1).unwrap();
        let before = p.clone();
        let mut mutexed = wait(0x200, 0, 0);
        mutexed[3] = 0x6000_1000;
        let mut shared = wait(0x200, 0, 0);
        shared[5] = 0x90; // PTHREAD_PROCESS_SHARED | firstfit
        let mut targeted = signal(0x100, 0, 0);
        targeted[3] = 0x1203;
        let cases: [(&str, Result<Outcome, String>); 7] = [
            ("with mutex 0x60001000", p.cvwait(mutexed, 2)),
            ("with flags 0x90", p.cvwait(shared, 2)),
            ("targets thread port 0x1203", p.cvsignal(targeted)),
            ("S 0x200 is not below L 0x200", p.cvwait(wait(0x200, 0x200, 0), 2)),
            ("S 0x300 are out of order", p.cvsignal(signal(0x100, 0x300, 0))),
            ("releases 32 waiters", p.cvbroad(broad(0x100, 0, 0, 0x2000), 4)),
            ("thread 1 already waits at sequence 0x100", p.cvwait(wait(0x100, 1, 0), 2)),
        ];
        for (want, got) in cases {
            let e = got.expect_err(want);
            assert!(e.starts_with("M48: psynch ") && e.contains(want), "{want}: {e}");
        }
        assert_eq!(p, before, "a refusal leaves the model untouched");
    }
}
