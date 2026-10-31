//! M54 t3 (spec 3c-3e): exec in place. `read_exec_call` reads the call out of guest memory, `exec_image`
//! replaces the image, and both record and replay call them with the same arguments (symmetry rule 1).
//! Everything that can refuse happens before `exec_image`: in `read_exec_call` (and, on record only, in
//! `retrace-core`'s `resolve`), so a failure past the point of no return is a retrace bug, never a
//! half-replaced guest.
use super::*;
use std::collections::BTreeSet;

/// What `execve` and `posix_spawn(POSIX_SPAWN_SETEXEC)` ask for, read out of guest memory.
///
/// task 4 fills `spawn`. `args_refusal` is the refusal of the argument vector (EFAULT, E2BIG, or the
/// start-stack wall): the kernel finds and checks the file BEFORE it copies the argument strings in
/// (measured natively: a missing path with a bad `argv` is ENOENT, garbage is ENOEXEC), so the record
/// arm resolves the file first and answers this only for an image it would have run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecCall {
    pub num: u64,
    pub path: Vec<u8>,
    pub argv: Vec<Vec<u8>>,
    pub envp: Vec<Vec<u8>>,
    pub spawn: Option<SpawnPlan>,
    pub args_refusal: Option<ExecRefusal>,
}

/// A `posix_spawn(POSIX_SPAWN_SETEXEC)`'s decoded attributes and file actions (spec 3f), the output of
/// `decode_spawn`. `sigdefault` is 0 unless SETSIGDEF is set; `sigmask` is `Some` only with SETSIGMASK.
/// `binpref` is entry 0 of `psa_binprefs` / `psa_subcpuprefs`: `(cputype, subcpupref)`, `(0, -1)` unset
/// (the brief's struct has no such field; `resolve` needs it to pick a slice, so it rides here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnPlan {
    pub flags: u16,
    pub sigmask: Option<u32>,
    pub sigdefault: u32,
    pub actions: Vec<FileAction>,
    pub binpref: (u32, i32),
}

/// One `posix_spawn_file_actions` entry. The descriptors are the guest's `int`s, zero-extended; a negative
/// one is a number no slot has, so it is EBADF like any other closed one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileAction {
    Open { fd: u32, path: Vec<u8>, oflag: i32, mode: u32 },
    Close { fd: u32 },
    Dup2 { from: u32, to: u32 },
    Inherit { fd: u32 },
    Chdir { path: Vec<u8> },
    Fchdir { fd: u32 },
}

const ATTR_SIZE: usize = 0xf8;
const ACT_STRIDE: usize = 1040;
const ACT_MAX: usize = 4096;

fn spawn_wall(what: &str) -> String { format!("posix_spawn with {what} (syscall 244)") }

/// The byte `posix_spawnattr_init` leaves at `off` of `_posix_spawnattr` (t4 addendum A, measured).
fn attr_init_byte(off: usize) -> u8 {
    match off { 0x38 => 1, 0x44..=0x4f | 0x70..=0x7f => 0xff, _ => 0 }
}

/// The field of A's table that holds attr byte `off`.
fn attr_field(off: usize) -> String {
    // Offsets from xnu `spawn_internal.h` (docs/sweep-evidence/.../xnu-spawn_internal.h-main-2025-10-04.h), laid out
    // by C alignment; the measured ones (0x38, 0x44..0x4f, 0x70..0x7f, the 0xc0 end) agree with `attrdump.txt`.
    match off {
        0x0c..=0x0f => "psa_pgroup".into(),
        0x20..=0x23 => "psa_pcontrol".into(),
        0x24..=0x27 => "psa_apptype".into(),
        0x28..=0x2f => "psa_cpumonitor_percent".into(),
        0x30..=0x37 => "psa_cpumonitor_interval".into(),
        0x38..=0x3f => "psa_reserved".into(),
        0x40..=0x41 => "psa_jetsam_flags".into(),
        0x44..=0x47 => "psa_priority".into(),
        0x48..=0x4b => "psa_memlimit_active".into(),
        0x4c..=0x4f => "psa_memlimit_inactive".into(),
        0x50..=0x57 => "psa_qos_clamp".into(),
        0x58..=0x5b => "psa_darwin_role".into(),
        0x5c..=0x5f => "psa_thread_limit".into(),
        0x60..=0x67 => "psa_max_addr".into(),
        0x68 => "psa_no_smt".into(),
        0x69 => "psa_tecs".into(),
        0x6c..=0x6f => "psa_platform".into(),
        0x80..=0x83 => "psa_options".into(),
        0x84..=0x87 => "psa_port_soft_limit".into(),
        0x88..=0x8b => "psa_port_hard_limit".into(),
        0x8c..=0x8f => "psa_filedesc_soft_limit".into(),
        0x90..=0x93 => "psa_filedesc_hard_limit".into(),
        0x94..=0x97 => "psa_crash_behavior".into(),
        0x98..=0x9b => "psa_dataless_iopolicy".into(),
        0xa0..=0xa7 => "psa_crash_behavior_deadline".into(),
        0xa8 => "psa_launch_type".into(),
        0xaa..=0xab => "psa_sec_flags".into(),
        0xac..=0xaf => "psa_crash_count".into(),
        0xb0..=0xb3 => "psa_throttle_timeout".into(),
        0xb4..=0xb7 => "psa_kqworkloop_soft_limit".into(),
        0xb8..=0xbb => "psa_kqworkloop_hard_limit".into(),
        0xbc..=0xbf => "psa_conclave_mem_limit".into(),
        o => format!("padding byte {o:#x}"),
    }
}

/// Decode a SETEXEC `posix_spawn`'s descriptor (144 bytes), attribute block and file-action block (spec 3f,
/// t4 addendum A to D). Pure. `Err` is the "what" of a wall by value, naming the field; every one is a model
/// gap of retrace's, never a claim about what macOS does with it. `actions` is the block read at
/// `file_actions`, `file_actions_size` bytes of it.
pub fn decode_spawn(desc: &[u8], attr: Option<&[u8]>, actions: Option<&[u8]>) -> Result<SpawnPlan, String> {
    if desc.len() < 144 { return Err(spawn_wall("a descriptor shorter than 144 bytes")); }
    let q = |o: usize| u64::from_le_bytes(desc[o..o + 8].try_into().unwrap());
    for (o, name) in [(32, "port actions"), (48, "MAC extensions"), (64, "a coalition"), (80, "a persona"),
                      (96, "POSIX credentials"), (112, "a subsystem root path"), (128, "a conclave id")] {
        if q(o) != 0 { return Err(spawn_wall(name)); }
    }
    let mut plan = SpawnPlan { flags: 0, sigmask: None, sigdefault: 0, actions: Vec::new(), binpref: (0, -1) };
    if let Some(a) = attr {
        if q(0) != ATTR_SIZE as u64 || a.len() != ATTR_SIZE {
            return Err(spawn_wall(&format!("an attribute block of {} bytes", q(0))));
        }
        let flags = u16::from_le_bytes([a[0], a[1]]);
        for bit in 0..16 {
            if let Some(name) = retrace_arch::spawn_refused_flag_name(1u16 << bit).filter(|_| flags & (1 << bit) != 0) {
                return Err(spawn_wall(&name));
            }
        }
        let u32_at = |o: usize| u32::from_le_bytes(a[o..o + 4].try_into().unwrap());
        let (has_def, has_mask) = (flags & retrace_arch::POSIX_SPAWN_SETSIGDEF != 0, flags & retrace_arch::POSIX_SPAWN_SETSIGMASK != 0);
        for (off, &byte) in a.iter().enumerate().take(0xc0) {
            let accepted = off < 2 || (4..8).contains(&off) && has_def || (8..12).contains(&off) && has_mask
                || (0x10..0x20).contains(&off) || (0x70..0x80).contains(&off);
            if !accepted && byte != attr_init_byte(off) { return Err(spawn_wall(&format!("{} set", attr_field(off)))); }
        }
        // Entry 0 decides (D); entries 1..3 must be unset (cputype 0, subcpupref -1).
        for i in 1..4 {
            if u32_at(0x10 + 4 * i) != 0 || u32_at(0x70 + 4 * i) as i32 != -1 {
                return Err(spawn_wall(&format!("a binary preference at entry {i}")));
            }
        }
        let (cpu, sub) = (u32_at(0x10), u32_at(0x70) as i32);
        if cpu == 0 && sub != -1 { return Err(spawn_wall("a subtype with no cputype in binary preference 0")); }
        plan.flags = flags;
        plan.binpref = (cpu, sub);
        if has_def { plan.sigdefault = u32_at(4); }
        if has_mask { plan.sigmask = Some(u32_at(8)); }
    }
    if let Some(b) = actions {
        let count = if b.len() >= 8 { i32::from_le_bytes(b[4..8].try_into().unwrap()) } else { -1 };
        if b.len() < 8 || count < 0 || count as usize > ACT_MAX || b.len() != 8 + count as usize * ACT_STRIDE
            || q(16) != b.len() as u64 {
            return Err(spawn_wall(&format!("file actions of {} bytes for a count of {count}", q(16))));
        }
        for i in 0..count as usize {
            let e = &b[8 + i * ACT_STRIDE..8 + (i + 1) * ACT_STRIDE];
            let i32_at = |o: usize| i32::from_le_bytes(e[o..o + 4].try_into().unwrap());
            let cstr = |from: usize, to: usize| e[from..to].iter().position(|&c| c == 0).map(|n| e[from..from + n].to_vec());
            let fd = i32_at(4) as u32;
            plan.actions.push(match i32_at(0) {
                0 => FileAction::Open { fd, oflag: i32_at(8), mode: u32::from(u16::from_le_bytes([e[12], e[13]])),
                    path: cstr(14, 14 + 1024).ok_or_else(|| spawn_wall("an open action whose path is not terminated"))? },
                1 => FileAction::Close { fd },
                2 => FileAction::Dup2 { from: fd, to: i32_at(8) as u32 },
                3 => FileAction::Inherit { fd },
                4 => return Err(spawn_wall("a fileport dup2 action")),
                5 => FileAction::Chdir { path: cstr(8, 8 + 1024).ok_or_else(|| spawn_wall("a chdir action whose path is not terminated"))? },
                6 => FileAction::Fchdir { fd },
                t => return Err(spawn_wall(&format!("a file action of type {t}"))),
            });
        }
    }
    Ok(plan)
}

/// A step's host work: `(action index, action, source host descriptor)` to the host descriptor a new slot takes.
pub type HostOp<'a> = dyn FnMut(usize, &FileAction, Option<i32>) -> Result<Option<i32>, u64> + 'a;

/// Why a spawn's actions end the call: an errno the program is answered (with the action's index), or a
/// wall by value (marked already).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnFail { Errno(usize, u64), Wall(String) }

/// A guest-slot effect of an action that the guest kqueue and pipe model must see, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotEffect { Close(u64), Dup(u64, u64) }

/// The state the actions run over: the descriptor table, the set of slots whose descriptor carries
/// `FD_CLOEXEC`, the guest kqueue slots, the slots an action names, and what each step displaced. Record's
/// `spawn_actions_host` runs it on a scratch clone; `exec_image` runs it again, on the live table, on both
/// sides, so the slot half is one function and cannot drift.
pub struct SpawnState {
    pub fds: FdTable,
    pub cloexec: BTreeSet<u32>,
    /// T4-h: the slots close-on-exec BEFORE the spawn that no action has replaced yet. Under SETEXEC a close,
    /// dup2 source or inherit of one is EBADF (measured); an open or dup2 into its slot undoes it.
    pub doomed: BTreeSet<u32>,
    pub kq: BTreeSet<u64>,
    pub named: BTreeSet<u32>,
    pub effects: Vec<SlotEffect>,
    /// Host descriptors a step displaced (an overwritten or closed slot's), for the caller to close if > 2.
    pub displaced: Vec<i32>,
}

impl SpawnState {
    pub fn new(fds: FdTable, cloexec: BTreeSet<u32>, kq: BTreeSet<u64>) -> SpawnState {
        SpawnState { doomed: cloexec.clone(), fds, cloexec, kq, named: BTreeSet::new(), effects: Vec::new(), displaced: Vec::new() }
    }

    /// Run `actions` in order. `host(i, action, source_host)` does action `i`'s host work and returns the
    /// host descriptor a new slot takes (open, dup2) or `None`; record passes the real calls, replay and
    /// the live run pass a lookup of what record made. The first failing action ends the run.
    pub fn run(&mut self, actions: &[FileAction],
               host: &mut HostOp) -> Result<(), SpawnFail> {
        const EBADF: u64 = 9;
        let bad = |fd: u32| (fd as i32) < 0 || u64::from(fd) >= DUP2_MAX_FD;
        for (i, a) in actions.iter().enumerate() {
            let fail = |e: u64| SpawnFail::Errno(i, e);
            match a {
                FileAction::Open { fd, oflag, .. } => {
                    if bad(*fd) { return Err(fail(EBADF)); }
                    let h = host(i, a, None).map_err(fail)?;
                    // T4-i (measured): xnu opens at the lowest free slot and dup2s to the target, which clears
                    // close-on-exec, so O_CLOEXEC sticks only when the target IS the lowest free slot.
                    let lowest = (0u64..).find(|&g| !self.fds.is_open(g)).unwrap_or(0);
                    self.displaced.extend(self.fds.set_open(u64::from(*fd), h));
                    if oflag & 0x0100_0000 != 0 && u64::from(*fd) == lowest { self.cloexec.insert(*fd); } else { self.cloexec.remove(fd); }
                    self.doomed.remove(fd);
                    self.kq.remove(&u64::from(*fd));
                    self.named.insert(*fd);
                    self.effects.push(SlotEffect::Close(u64::from(*fd)));
                }
                FileAction::Close { fd } => {
                    if !self.fds.is_open(u64::from(*fd)) || self.doomed.contains(fd) { return Err(fail(EBADF)); }
                    self.displaced.extend(self.fds.host(u64::from(*fd)));
                    self.fds.close(u64::from(*fd));
                    self.cloexec.remove(fd);
                    self.kq.remove(&u64::from(*fd));
                    self.effects.push(SlotEffect::Close(u64::from(*fd)));
                }
                FileAction::Dup2 { from, to } => {
                    if !self.fds.is_open(u64::from(*from)) || self.doomed.contains(from) || bad(*to) { return Err(fail(EBADF)); }
                    if from == to { self.named.insert(*to); continue; }
                    let src = self.fds.host(u64::from(*from));
                    let h = host(i, a, src).map_err(fail)?;
                    let (_, displaced) = self.fds.dup2(u64::from(*from), u64::from(*to), h).map_err(fail)?;
                    self.displaced.extend(displaced);
                    self.cloexec.remove(to);
                    self.doomed.remove(to);
                    if self.kq.contains(&u64::from(*from)) { self.kq.insert(u64::from(*to)); } else { self.kq.remove(&u64::from(*to)); }
                    self.named.insert(*to);
                    self.effects.push(SlotEffect::Dup(u64::from(*from), u64::from(*to)));
                }
                FileAction::Inherit { fd } => {
                    // R-M1a (measured): inherit does not clear FD_CLOEXEC; a close-on-exec descriptor is EBADF.
                    if !self.fds.is_open(u64::from(*fd)) || self.doomed.contains(fd) { return Err(fail(EBADF)); }
                    if self.kq.contains(&u64::from(*fd)) {
                        return Err(SpawnFail::Wall(retrace_arch::wall(WallCategory::ProcessCreation, spawn_wall(
                            &format!("an inherit action on descriptor {fd}, a guest kqueue")))));
                    }
                    self.named.insert(*fd);
                }
                FileAction::Chdir { .. } => { host(i, a, None).map_err(fail)?; }
                FileAction::Fchdir { fd } => {
                    if !self.fds.is_open(u64::from(*fd)) { return Err(fail(EBADF)); }
                    host(i, a, self.fds.host(u64::from(*fd))).map_err(fail)?;
                }
            }
        }
        Ok(())
    }

    /// The slots exec closes on the host's answer plus `CLOEXEC_DEFAULT`'s: every open slot that carries
    /// `FD_CLOEXEC`, and, with the flag, every one no action names.
    pub fn closed(&self, flags: u16) -> Vec<u32> {
        let cxd = flags & retrace_arch::POSIX_SPAWN_CLOEXEC_DEFAULT != 0;
        (0..self.fds.slots().len() as u32).filter(|&g| self.fds.is_open(u64::from(g))
            && (self.cloexec.contains(&g) || (cxd && !self.named.contains(&g)))).collect()
    }
}

/// What record's host half of a spawn's actions made and must either keep or undo (spec 3f, C1): the host
/// descriptors the open and dup2 actions created (by action index), the close set over the post-action
/// table, and a descriptor on the working directory when a chdir ran. `commit` after the exec is certain;
/// `rollback` when the image fails, which closes what was made and restores the cwd.
#[derive(Debug)]
pub struct SpawnTxn {
    pub host_new: Vec<Option<i32>>,
    pub closed: Vec<u32>,
    cwd: Option<i32>,
}

impl SpawnTxn {
    pub fn commit(self) {
        // SAFETY: closing the descriptor this transaction opened on the old cwd.
        if let Some(c) = self.cwd { unsafe { libc::close(c); } }
    }
    pub fn rollback(self) {
        for h in self.host_new.iter().flatten() {
            // SAFETY: closing a host descriptor this transaction made and nothing else holds.
            unsafe { libc::close(*h); }
        }
        if let Some(c) = self.cwd {
            // SAFETY: fchdir to the descriptor saved before the first chdir, then close it.
            unsafe { libc::fchdir(c); libc::close(c); }
        }
    }
}

/// A refusal before the point of no return: an errno the program is answered, or a wall (the string
/// is already marked by `retrace_arch::wall`, so `Err` of it ends the recording as one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecRefusal { Errno(u64), Wall(String) }

/// What `exec_image` installs: the parsed images (both sides build them from the same bytes), the
/// argument strings, the `executable_path=` apple string, and the descriptors exec closes.
pub struct ExecImage {
    pub exe: Loaded,
    pub dylinker: Loaded,
    pub argv: Vec<Vec<u8>>,
    pub envp: Vec<Vec<u8>>,
    pub exec_path: Vec<u8>,
    pub closed: Vec<u32>,
    pub spawn: Option<SpawnPlan>,
    /// Record only: the host descriptors the spawn's open and dup2 actions made, by action index (empty on
    /// replay, which has no host side).
    pub host_new: Vec<Option<i32>>,
}

/// xnu's `ARG_MAX`: the kernel refuses `E2BIG` when the argument and environment strings and their pointer
/// arrays (`sum(len + 1) + 8 * (argc + envc + 2)`) pass it (Ruling T0-d; the boundary is measured, the
/// formula inferred from one exact fit).
const EXEC_ARG_MAX: usize = 1_048_576;
const E2BIG: u64 = 7;
const ENAMETOOLONG: u64 = 63;
const PATH_MAX: usize = 1024;

impl Box_ {
    /// The `psa_flags` of a `posix_spawn`'s descriptor (`args[2]`: `attr_size` at +0, `attrp` at +8;
    /// `psa_flags` is a `short` at the attr's +0, t0 M3). `Some(None)` is a NULL descriptor or NULL `attrp`
    /// (no flags); `None` is a descriptor or attr the program could not have read.
    pub fn spawn_flags(&self, desc: u64) -> Option<Option<u16>> {
        if desc == 0 { return Some(None); }
        let d = self.read_va_checked(desc, 16)?;
        let attrp = u64::from_le_bytes(d[8..16].try_into().unwrap());
        if attrp == 0 { return Some(None); }
        let f = self.read_va_checked(attrp, 2)?;
        Some(Some(u16::from_le_bytes([f[0], f[1]])))
    }

    /// A NUL-terminated string at `va`, at most `max` bytes with its NUL, read through the checked reader a
    /// page at a time. `Err(EFAULT)` for memory the program could not read, `too_long` when no NUL comes
    /// within `max` bytes.
    fn read_exec_cstr(&self, va: u64, max: usize, too_long: u64) -> Result<Vec<u8>, ExecRefusal> {
        let mut out = Vec::new();
        let mut a = va;
        while out.len() < max {
            let n = (((a | (GRANULE as u64 - 1)) + 1 - a) as usize).min(max - out.len());
            let Some(chunk) = self.read_va_checked(a, n) else { return Err(ExecRefusal::Errno(retrace_arch::EFAULT)) };
            if let Some(nul) = chunk.iter().position(|&c| c == 0) {
                out.extend_from_slice(&chunk[..nul]);
                return Ok(out);
            }
            out.extend_from_slice(&chunk);
            a = a.wrapping_add(n as u64);
        }
        Err(ExecRefusal::Errno(too_long))
    }

    /// A NULL-terminated array of string pointers at `va` (`va == 0` is an empty array, as natively),
    /// appended to `out`. `total` is the running size in the kernel's formula; E2BIG once it, with the two
    /// terminators, passes `EXEC_ARG_MAX`.
    fn read_exec_strings(&self, va: u64, total: &mut usize, out: &mut Vec<Vec<u8>>) -> Result<(), ExecRefusal> {
        if va == 0 { return Ok(()); }
        let mut i = 0u64;
        loop {
            let p = self.read_va_checked(va.wrapping_add(i * 8), 8).ok_or(ExecRefusal::Errno(retrace_arch::EFAULT))?;
            let sp = u64::from_le_bytes(p.try_into().unwrap());
            if sp == 0 { return Ok(()); }
            let room = (EXEC_ARG_MAX + 1).saturating_sub(*total);
            let s = self.read_exec_cstr(sp, room.max(1), E2BIG)?;
            *total += s.len() + 1 + 8;
            if *total + 16 > EXEC_ARG_MAX { return Err(ExecRefusal::Errno(E2BIG)); }
            out.push(s);
            i += 1;
        }
    }

    /// Read an `execve` (59) or `posix_spawn` (244, SETEXEC) call out of guest memory. `Err(Errno)` is the
    /// path itself: `EFAULT` unreadable, `ENAMETOOLONG` past `PATH_MAX`. The argument vector's refusals ride in
    /// `ExecCall::args_refusal` (see its doc): E2BIG past native's limit, and the start-stack wall for a total
    /// past `START_ARGS_LIMIT` but under native's.
    ///
    /// A `posix_spawn`'s arguments are `(pid*, path, desc, argv, envp)`; its descriptor, attribute block and
    /// file-action block are read through the checked reader and decoded by `decode_spawn`, whose `Err` is a
    /// wall (the recording ends). The sizes come from the descriptor and are bounded before the read.
    pub fn read_exec_call(&self, num: u64, args: [u64; 8]) -> Result<ExecCall, ExecRefusal> {
        let efault = || ExecRefusal::Errno(retrace_arch::EFAULT);
        let (spawn, a) = if num == retrace_arch::SYS_POSIX_SPAWN {
            if args[2] == 0 { return Err(efault()); }
            let desc = self.read_va_checked(args[2], 144).ok_or_else(efault)?;
            let q = |o: usize| u64::from_le_bytes(desc[o..o + 8].try_into().unwrap());
            let (attr_size, attrp, fa_size, fa) = (q(0), q(8), q(16), q(24));
            let attr = if attrp == 0 { None } else {
                let n = if attr_size <= 0x1000 { attr_size as usize } else { 0 };
                Some(self.read_va_checked(attrp, n).ok_or_else(efault)?)
            };
            let acts = if fa == 0 || fa_size == 0 { None } else {
                let hdr = self.read_va_checked(fa, 8).ok_or_else(efault)?;
                let count = i32::from_le_bytes(hdr[4..8].try_into().unwrap());
                let want = if (0..=ACT_MAX as i32).contains(&count) { 8 + count as u64 * ACT_STRIDE as u64 } else { u64::MAX };
                Some(if fa_size == want { self.read_va_checked(fa, fa_size as usize).ok_or_else(efault)? } else { hdr })
            };
            let plan = decode_spawn(&desc, attr.as_deref(), acts.as_deref())
                .map_err(|m| ExecRefusal::Wall(retrace_arch::wall(WallCategory::ProcessCreation, m)))?;
            (Some(plan), [args[1], args[3], args[4]])
        } else { (None, [args[0], args[1], args[2]]) };
        let path = self.read_exec_cstr(a[0], PATH_MAX, ENAMETOOLONG)?;
        let (mut argv, mut envp, mut total) = (Vec::new(), Vec::new(), 0usize);
        let mut args_refusal = self.read_exec_strings(a[1], &mut total, &mut argv)
            .and_then(|()| self.read_exec_strings(a[2], &mut total, &mut envp)).err();
        if args_refusal.is_none() && total + 16 > START_ARGS_LIMIT {
            args_refusal = Some(ExecRefusal::Wall(retrace_arch::wall(WallCategory::ProcessCreation, format!(
                "{} with {} bytes of arguments, past retrace's 128 KiB start-stack limit",
                if num == retrace_arch::SYS_POSIX_SPAWN { "posix_spawn" } else { "execve" }, total + 16))));
        }
        if args_refusal.is_some() { argv.clear(); envp.clear(); }
        Ok(ExecCall { num, path, argv, envp, spawn, args_refusal })
    }

    /// Record only: the guest descriptors whose HOST descriptor carries `FD_CLOEXEC` right now, the host
    /// kernel's own answer to which ones exec closes (Ruling 3). A box model of `FD_CLOEXEC` across `open`,
    /// `fcntl`, `dup` and the rest would be a silently wrong close set the first time it missed a setter,
    /// which a determinism oracle cannot see. A slot with no host descriptor (replay has none) is skipped.
    pub fn cloexec_on_host(&self) -> Vec<u32> {
        let mut out = Vec::new();
        for gfd in 0..self.fds.slots().len() as u64 {
            if !self.fds.is_open(gfd) { continue; }
            let Some(host) = self.fds.host(gfd) else { continue };
            // SAFETY: fcntl(F_GETFD) on a descriptor number reads a flag word and writes nothing.
            let fl = unsafe { libc::fcntl(host, libc::F_GETFD) };
            if fl >= 0 && fl & libc::FD_CLOEXEC != 0 { out.push(gfd as u32); }
        }
        out
    }

    /// Record only, a SETEXEC spawn's host half (spec 3f, C1; t4 addendum B). Runs `plan.actions` in order on a
    /// scratch clone of the descriptor table, doing each one's host work as it goes: an `open` into a fresh host
    /// descriptor, a `dup2` as a host `dup`, `chdir`/`fchdir` on retrace's real cwd (so a later relative `open`
    /// resolves as natively). The first failure undoes everything made (closes the new host descriptors,
    /// `fchdir`s back) and is returned as the call's errno; a close, and the old host descriptor behind a
    /// `dup2` target, are never touched here, since a host close cannot be undone: `exec_image` closes them at
    /// the commit. The shape: PREPARE (this) returns a `SpawnTxn`, then the record arm resolves the image and
    /// either `commit`s it or `rollback`s it. The guest-visible table half is `SpawnState::run`, run again by
    /// `exec_image` on both sides.
    pub fn spawn_actions_host(&mut self, plan: &SpawnPlan) -> Result<SpawnTxn, SpawnFail> {
        let n = plan.actions.len();
        let slots = self.fds.slots().len() as u64;
        let kq = (0..slots).filter(|&g| self.gkq.is_kqueue(g)).collect();
        let mut st = SpawnState::new(self.fds.clone(), self.cloexec_on_host().into_iter().collect(), kq);
        let wants_cwd = plan.actions.iter().any(|a| matches!(a, FileAction::Chdir { .. } | FileAction::Fchdir { .. }));
        let cwd = wants_cwd.then(|| {
            // SAFETY: open(2) on a constant NUL-terminated path.
            let fd = unsafe { libc::open(c".".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
            assert!(fd >= 0, "spawn: cannot open the working directory to restore it: {}", std::io::Error::last_os_error());
            fd
        });
        let mut host_new = vec![None; n];
        let errno = || std::io::Error::last_os_error().raw_os_error().unwrap_or(9) as u64;
        let res = st.run(&plan.actions, &mut |i, a, src| match a {
            FileAction::Open { path, oflag, mode, .. } => {
                let c = std::ffi::CString::new(path.clone()).map_err(|_| 2u64)?;
                // SAFETY: open(2) on a NUL-terminated path with the guest's flags and mode, less O_CLOEXEC: whether the
                // slot is close-on-exec is `SpawnState::run`'s rule (it lands in place or not), carried to replay by
                // the recorded close set (`Event::Exec.closed`), never by the host flag.
                let h = unsafe { libc::open(c.as_ptr(), *oflag & !0x0100_0000, *mode as libc::c_uint) };
                if h < 0 { return Err(errno()); }
                host_new[i] = Some(h);
                Ok(Some(h))
            }
            FileAction::Dup2 { .. } => match src {
                // SAFETY: dup(2) of a host descriptor the table holds.
                Some(sh) => { let d = unsafe { libc::dup(sh) }; if d < 0 { return Err(errno()); } host_new[i] = Some(d); Ok(Some(d)) }
                None => Ok(None),
            },
            FileAction::Chdir { path } => {
                let c = std::ffi::CString::new(path.clone()).map_err(|_| 2u64)?;
                // SAFETY: chdir(2) on a NUL-terminated path, the call the guest's own chdir forwards.
                if unsafe { libc::chdir(c.as_ptr()) } != 0 { Err(errno()) } else { Ok(None) }
            }
            FileAction::Fchdir { .. } => match src {
                // SAFETY: fchdir(2) on a host descriptor the table holds.
                Some(h) => if unsafe { libc::fchdir(h) } != 0 { Err(errno()) } else { Ok(None) },
                None => Err(20), // ENOTDIR: a slot with no host descriptor is not a directory
            },
            _ => Ok(None),
        });
        let txn = SpawnTxn { host_new, closed: Vec::new(), cwd };
        match res {
            Ok(()) => Ok(SpawnTxn { closed: st.closed(plan.flags), ..txn }),
            Err(e) => { txn.rollback(); Err(e) }
        }
    }

    /// The pending signals that exec lets through (spec 2b, C5, t4 Ruling T4-f): scanned in ascending signal
    /// number, a default-ignored one is discarded (its bit comes back in the first member), a stop signal is a
    /// `Signals` wall by value (`Err`), and the first terminating one is the run's end (the second member).
    fn release_scan(pending: u32, mask: u32) -> Result<(u32, Option<u64>), String> {
        let mut discard = 0u32;
        for sig in 1..=31u64 {
            let bit = 1u32 << (sig - 1);
            if pending & bit == 0 || mask & bit != 0 { continue; }
            match retrace_arch::exec_release_class(sig) {
                retrace_arch::ExecRelease::Ignore => discard |= bit,
                retrace_arch::ExecRelease::Stop => return Err(retrace_arch::wall(WallCategory::Signals, format!(
                    "{} pending at exec, which stops the new image: stopping a process is not modelled",
                    retrace_arch::signal_name(sig)))),
                retrace_arch::ExecRelease::Terminate => return Ok((discard, Some(sig))),
            }
        }
        Ok((discard, None))
    }

    /// `SIGKILL` and `SIGSTOP` cannot be masked (`sigcantmask`); a `psa_sigmask` naming them does not hold them.
    const SIG_CANT_MASK: u32 = (1 << 8) | (1 << 16);

    /// Before the point of no return (step 4 of t4 Ruling T4-b): the signal the new image would die of at its
    /// first instruction, or the stop-signal wall, as a pure function of box state and the plan, so record
    /// and replay agree. `take_exec_release` is its twin after `exec_image`.
    pub fn exec_release_check(&self, plan: Option<&SpawnPlan>) -> Result<Option<u64>, String> {
        let ignored = self.sigtable.ignored_after_exec(plan.map_or(0, |p| p.sigdefault));
        let mask = plan.and_then(|p| p.sigmask).map(|m| m & !Self::SIG_CANT_MASK)
            .unwrap_or_else(|| self.threads.mask_of(self.threads.current()));
        Self::release_scan(self.threads.exec_pending(ignored), mask).map(|(_, t)| t)
    }

    /// C5, after `exec_image` (both sides): what the new image takes at once. A default-ignored pending signal
    /// is discarded; a terminating one comes back for the caller to end the run with (the exit is `128 + sig`).
    /// Signals the mask blocks stay pending into the new image.
    pub fn take_exec_release(&mut self) -> Result<Option<u64>, Unsupported> {
        let (pending, mask) = (self.threads.pending_of(0), self.threads.mask_of(0));
        let (discard, term) = Self::release_scan(pending, mask)
            .map_err(|m| Unsupported::from_marked(&m).expect("release_scan walls are marked"))?;
        self.threads.clear_pending_of(0, discard);
        Ok(term)
    }

    /// Replace the running image (spec 3c, in its order). Both sides call it with the same `img`.
    ///
    /// **The point of no return is the first line.** Every check that can refuse is in `read_exec_call` and
    /// the record side's `resolve`; past here a failure is a retrace bug (a panic: the banner, exit 70),
    /// never a half-replaced guest. The VM and vCPU are reused (HVF allows one VM per process).
    pub fn exec_image(&mut self, img: &ExecImage) -> Result<(), Unsupported> {
        // 1. Tear down every backing but the vector table and the two commpage granules. The commpages
        // came from the live host mapping when the recording started and are kept, so exec never puts host
        // bytes into the guest that replay's host may not have; the vector table is a pure function. The
        // page tables, the sign scratch and the TLBI stub go with the rest. Each is unmapped from stage 2
        // before its host pages are freed, which also frees every IPA `stage_image` maps again.
        let keep = [TRAMPOLINE_IPA, COMMPAGE_IPA, COMMPAGE2_IPA];
        let mut i = 0;
        while i < self.backings.len() {
            if keep.contains(&self.backings[i].ipa) { i += 1; continue; }
            let bk = self.backings.remove(i);
            self.vm.unmap(bk.ipa, bk.len).expect("hv_vm_unmap (exec): a backing the box maps is mapped");
            drop(bk);
        }
        self.reservations.clear();
        self.noaccess.clear();
        self.mmap_next = MMAP_BASE;
        self.tlbi_stub_ready = false;

        // 2. Stage the new image exactly as the start does, with the thread id the table will assign.
        let tid = self.threads.next_id();
        let st = Self::stage_image(&self.vm, &mut self.backings, &img.exe, &img.dylinker, &img.argv, &img.envp,
                                   &img.exec_path, tid, &mut |_, _| {});

        // 3. The posture, the page-table state, the registers. The pager is re-installed by the new dyld's
        // 294 or 536 on both sides, as at the start.
        Self::set_pac_keys(&self.vcpu);
        Self::start_sysregs(&self.vcpu, &st);
        for r in 0..31 { self.vcpu.set_reg(reg::x(r), 0).unwrap(); }
        for q in 0..32 { self.vcpu.set_simd(simd::q(q), 0).unwrap(); }
        self.vcpu.set_reg(reg::FPCR, 0).unwrap();
        self.vcpu.set_reg(reg::FPSR, 0).unwrap();
        let StagedImage { l2_host, next_l3, pac, .. } = st;
        self.pac_enabled = pac;
        self.stack_top = DYN_STACK_TOP;
        self.stack_size = DYN_STACK_SIZE;
        self.l2_host = l2_host;
        self.next_l3 = next_l3;
        self.cache = None;
        self.cache_refault_ipa = 0;
        self.cache_refault_count = 0;

        // 4. Every translation the old image made is stale: the guest issues the flush (M9).
        self.flush_guest_tlb();

        // 5. The descriptor table. A spawn's file actions run first, on the live table, through the same
        // `SpawnState::run` record's host half used on a scratch copy (record's host work is looked up from
        // `img.host_new`; replay has none). Then the host's `FD_CLOEXEC` set and `CLOEXEC_DEFAULT`'s
        // (`img.closed`) and every guest kqueue close, a kqueue having no host descriptor to ask. The host
        // descriptor of each goes on record; retrace's own 0/1/2 (a console slot's host side) are never
        // closed. The kqueues' tables go first, so a close in an action meets no waiter.
        let kq_fds = self.gkq.reset_for_exec();
        let mut closed: Vec<u64> = img.closed.iter().map(|&f| u64::from(f)).collect();
        if let Some(plan) = &img.spawn {
            let mut st = SpawnState::new(std::mem::replace(&mut self.fds, FdTable::new()), BTreeSet::new(),
                                         kq_fds.iter().copied().collect());
            let r = st.run(&plan.actions, &mut |i, _, _| Ok(img.host_new.get(i).copied().flatten()));
            if let Err(e) = r { panic!("exec: a spawn action fails past the point of no return ({e:?}): record checked it first"); }
            for e in &st.effects {
                match *e {
                    SlotEffect::Close(f) => self.gkq.close(f),
                    SlotEffect::Dup(a, b) => self.gkq.dup(a, b),
                }.unwrap_or_else(|m| panic!("exec: spawn action: {m}"));
            }
            for d in &st.displaced {
                // SAFETY: closing a host descriptor this box opened for the guest (record only).
                if *d > 2 { unsafe { libc::close(*d); } }
            }
            closed.extend(st.kq.iter().copied());
            self.fds = st.fds;
        } else {
            closed.extend(kq_fds);
        }
        closed.sort_unstable();
        closed.dedup();
        for gfd in closed {
            if let Some(host) = self.fds.host(gfd) {
                // SAFETY: closing a descriptor this box opened for the guest (record only; replay has none).
                if host > 2 { unsafe { libc::close(host); } }
            }
            self.fds.close(gfd);
            self.gkq.close(gfd).unwrap_or_else(|e| panic!("exec: closing descriptor {gfd}: {e}"));
        }

        // 6. What belonged to the old task's threads and memory. The pipes, `itimer`, `synthetic_tsc`,
        // `fall_throughs` and `bootstrap_port` stay (M4: register is stateless and lookup is a wall, so there
        // is no registered-port state to keep or reset).
        self.kq = kq::WorkqKqueue::default();
        self.jit = jit::JitSet::default();
        self.excl = None;
        self.psynch = psynch::Psynch::default();
        self.sems = sema::Semaphores::default();
        self.sleep = sleep::SleepModel::default();
        self.thread_start_pc = None;
        self.wq_thread_pc = None;
        self.pthread_size = None;

        // 7. The caller is the only thread, at index 0, with a fresh id; dispositions reset.
        let sigdefault = img.spawn.as_ref().map_or(0, |p| p.sigdefault);
        let ignored = self.sigtable.ignored_after_exec(sigdefault);
        self.sigtable.reset_for_exec(sigdefault);
        let caller = self.threads.current();
        let ctx = self.save_ctx();
        let id = self.threads.reset_for_exec(caller, ctx, ignored);
        // SETSIGMASK replaces the caller's mask (which `reset_for_exec` kept); `sigcantmask` bits never hold.
        if let Some(m) = img.spawn.as_ref().and_then(|p| p.sigmask) {
            self.threads.set_mask_of(0, retrace_arch::SIG_SETMASK, m & !Self::SIG_CANT_MASK);
        }
        debug_assert_eq!(id, tid, "the table assigned the id the TSD was staged with");
        self.reserve_believed_stack();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A descriptor with `attr_size` 0xf8 and `file_actions_size` `fa`, extensions empty.
    fn desc(fa: u64) -> Vec<u8> {
        let mut d = vec![0u8; 144];
        d[0..8].copy_from_slice(&0xf8u64.to_le_bytes());
        d[8..16].copy_from_slice(&0x1000u64.to_le_bytes());
        if fa != 0 { d[16..24].copy_from_slice(&fa.to_le_bytes()); d[24..32].copy_from_slice(&0x2000u64.to_le_bytes()); }
        d
    }
    /// `posix_spawnattr_init`'s bytes (t4 addendum A), with `flags`.
    fn attr(flags: u16) -> Vec<u8> {
        let mut a = vec![0u8; ATTR_SIZE];
        for (o, b) in a.iter_mut().enumerate().take(0xc0) { *b = attr_init_byte(o); }
        a[0..2].copy_from_slice(&flags.to_le_bytes());
        a
    }
    fn block(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut b = vec![0u8; 8];
        b[0..4].copy_from_slice(&(entries.len() as i32).to_le_bytes());
        b[4..8].copy_from_slice(&(entries.len() as i32).to_le_bytes());
        for e in entries { b.extend_from_slice(e); }
        b
    }
    fn entry(ty: i32, fd: i32, w8: i32, mode: u16, path_at: usize, path: &[u8]) -> Vec<u8> {
        let mut e = vec![0u8; ACT_STRIDE];
        e[0..4].copy_from_slice(&ty.to_le_bytes());
        e[4..8].copy_from_slice(&fd.to_le_bytes());
        e[8..12].copy_from_slice(&w8.to_le_bytes());
        e[12..14].copy_from_slice(&mode.to_le_bytes());
        e[path_at..path_at + path.len()].copy_from_slice(path);
        e
    }

    // Probe §2, and the bytes the controller measured for `posix_spawnattr_init` + `setflags(SETEXEC)` +
    // `setbinpref_np(ARM64)`: flags 0x40, binprefs[0] 0x0100000c, subcpuprefs all -1.
    #[test]
    fn decode_accepts_homebrews_launcher_attributes() {
        let mut a = attr(0x40);
        a[0x10..0x14].copy_from_slice(&0x0100_000cu32.to_le_bytes());
        let p = decode_spawn(&desc(0), Some(&a), None).expect("the launcher's attributes");
        assert_eq!((p.flags, p.binpref, p.sigmask, p.sigdefault), (0x40, (0x0100_000c, -1), None, 0));
        assert!(p.actions.is_empty());
        // setflags with every accepted bit, and the signal sets, read when their flag is set.
        let mut a = attr(0x404c);
        a[4..8].copy_from_slice(&(1u32 << 30).to_le_bytes());
        a[8..12].copy_from_slice(&1u32.to_le_bytes());
        let p = decode_spawn(&desc(0), Some(&a), None).unwrap();
        assert_eq!((p.sigdefault, p.sigmask), (1 << 30, Some(1)));
        // The binpref ANY and an arch preference's subtype.
        let mut a = attr(0x40);
        a[0x10..0x14].copy_from_slice(&u32::MAX.to_le_bytes());
        a[0x70..0x74].copy_from_slice(&2i32.to_le_bytes());
        assert_eq!(decode_spawn(&desc(0), Some(&a), None).unwrap().binpref, (u32::MAX, 2));
    }

    #[test]
    fn decode_names_each_refused_field() {
        let why = |d: &[u8], a: &[u8]| decode_spawn(d, Some(a), None).unwrap_err();
        for (flag, name) in [(0x1u16, "POSIX_SPAWN_RESETIDS"), (0x2, "POSIX_SPAWN_SETPGROUP"), (0x10, "POSIX_SPAWN_SETSCHEDPARAM"),
                             (0x20, "POSIX_SPAWN_SETSCHEDULER"), (0x80, "POSIX_SPAWN_START_SUSPENDED"),
                             (0x100, "_POSIX_SPAWN_DISABLE_ASLR"), (0x400, "POSIX_SPAWN_SETSID"), (0x200, "flag 0x200")] {
            assert_eq!(why(&desc(0), &attr(0x40 | flag)), format!("posix_spawn with {name} (syscall 244)"));
        }
        // The extension sizes, by name.
        for (o, name) in [(32, "port actions"), (48, "MAC extensions"), (64, "a coalition"), (80, "a persona"),
                          (96, "POSIX credentials"), (112, "a subsystem root path"), (128, "a conclave id")] {
            let mut d = desc(0);
            d[o] = 8;
            assert_eq!(why(&d, &attr(0x40)), format!("posix_spawn with {name} (syscall 244)"));
        }
        // A byte of A's init column that is not the default, with its field.
        let mut a = attr(0x40);
        a[0x24] = 1;
        assert_eq!(why(&desc(0), &a), "posix_spawn with psa_apptype set (syscall 244)");
        let mut a = attr(0x40);
        a[0x0c] = 1;
        assert_eq!(why(&desc(0), &a), "posix_spawn with psa_pgroup set (syscall 244)");
        // sigdefault bytes with the flag clear are not the default.
        let mut a = attr(0x40);
        a[4] = 1;
        assert!(why(&desc(0), &a).contains("padding byte 0x4"), "{}", why(&desc(0), &a));
        // A second binary preference, and the wrong block size.
        let mut a = attr(0x40);
        a[0x14] = 1;
        assert_eq!(why(&desc(0), &a), "posix_spawn with a binary preference at entry 1 (syscall 244)");
        let mut d = desc(0);
        d[0] = 0x80;
        assert_eq!(why(&d, &attr(0x40)), "posix_spawn with an attribute block of 128 bytes (syscall 244)");
        // File actions: a fileport dup2, an unknown type, and a size that disagrees with the count.
        let one = |ty| block(&[entry(ty, 3, 0, 0, 8, b"")]);
        let size = 8 + ACT_STRIDE as u64;
        assert_eq!(decode_spawn(&desc(size), Some(&attr(0x40)), Some(&one(4))).unwrap_err(), "posix_spawn with a fileport dup2 action (syscall 244)");
        assert_eq!(decode_spawn(&desc(size), Some(&attr(0x40)), Some(&one(9))).unwrap_err(), "posix_spawn with a file action of type 9 (syscall 244)");
        assert!(decode_spawn(&desc(size + 1), Some(&attr(0x40)), Some(&one(1))).unwrap_err().contains("file actions of"));
    }

    // Control (d): with the open action's path read at +16 instead of +14, this fails (the path loses two bytes).
    #[test]
    fn decode_reads_file_actions_in_order() {
        let acts = block(&[
            entry(0, 7, 0x1000000, 0o644, 14, b"/etc/hosts"),
            entry(1, 8, 0, 0, 8, b""),
            entry(2, 1, 9, 0, 8, b""),
            entry(3, 2, 0, 0, 8, b""),
            entry(5, 0, 0, 0, 8, b"/usr"),
            entry(6, 5, 0, 0, 8, b""),
        ]);
        let p = decode_spawn(&desc(acts.len() as u64), Some(&attr(0x40)), Some(&acts)).unwrap();
        assert_eq!(p.actions, vec![
            FileAction::Open { fd: 7, path: b"/etc/hosts".to_vec(), oflag: 0x1000000, mode: 0o644 },
            FileAction::Close { fd: 8 },
            FileAction::Dup2 { from: 1, to: 9 },
            FileAction::Inherit { fd: 2 },
            FileAction::Chdir { path: b"/usr".to_vec() },
            FileAction::Fchdir { fd: 5 },
        ]);
    }

    #[test]
    fn the_actions_run_on_a_scratch_table_and_a_failure_names_its_action() {
        let mut fds = FdTable::new();
        let g = fds.alloc();
        assert_eq!(g, 3);
        let mut st = SpawnState::new(fds, BTreeSet::new(), BTreeSet::new());
        let r = st.run(&[FileAction::Dup2 { from: 3, to: 9 }, FileAction::Close { fd: 3 }, FileAction::Close { fd: 3 }], &mut |_, _, _| Ok(None));
        assert_eq!(r, Err(SpawnFail::Errno(2, 9)), "the second close of 3 is EBADF, action 2");
        assert!(st.fds.is_open(9) && !st.fds.is_open(3));
        // CLOEXEC_DEFAULT closes what no action names; inherit of a cloexec slot is EBADF.
        let mut fds = FdTable::new();
        fds.alloc(); fds.alloc();
        let mut st = SpawnState::new(fds, [4u32].into_iter().collect(), BTreeSet::new());
        assert_eq!(st.run(&[FileAction::Inherit { fd: 4 }], &mut |_, _, _| Ok(None)), Err(SpawnFail::Errno(0, 9)));
        assert_eq!(st.run(&[FileAction::Inherit { fd: 0 }, FileAction::Inherit { fd: 3 }], &mut |_, _, _| Ok(None)), Ok(()));
        assert_eq!(st.closed(retrace_arch::POSIX_SPAWN_CLOEXEC_DEFAULT), vec![1, 2, 4]);
        assert_eq!(st.closed(0), vec![4], "without the flag, only the FD_CLOEXEC slots");
    }

    // T4-h, measured: a descriptor close-on-exec before the spawn is doomed; a close, a dup2 source or an inherit
    // of it is EBADF until an action replaces it.
    #[test]
    fn a_descriptor_close_on_exec_before_the_spawn_is_doomed() {
        let mk = || { let mut f = FdTable::new(); for _ in 0..9 { f.alloc(); } SpawnState::new(f, [9u32].into_iter().collect(), BTreeSet::new()) };
        let ok = &mut |_: usize, _: &FileAction, _: Option<i32>| Ok(None);
        assert_eq!(mk().run(&[FileAction::Close { fd: 9 }], ok), Err(SpawnFail::Errno(0, 9)));
        assert_eq!(mk().run(&[FileAction::Dup2 { from: 9, to: 10 }], ok), Err(SpawnFail::Errno(0, 9)));
        assert_eq!(mk().run(&[FileAction::Dup2 { from: 9, to: 9 }], ok), Err(SpawnFail::Errno(0, 9)));
        assert_eq!(mk().run(&[FileAction::Inherit { fd: 9 }], ok), Err(SpawnFail::Errno(0, 9)));
        assert_eq!(mk().run(&[FileAction::Fchdir { fd: 9 }], ok), Ok(()), "fchdir of it works");
        // A dup2 into its slot replaces it, so it may be closed (measured: adddup2(8, 9) then addclose(9)).
        assert_eq!(mk().run(&[FileAction::Dup2 { from: 8, to: 9 }, FileAction::Close { fd: 9 }], ok), Ok(()));
        // An open into its slot replaces it too, and an inherit of an action-opened O_CLOEXEC slot is fine.
        assert_eq!(mk().run(&[FileAction::Open { fd: 9, path: vec![], oflag: 0x0100_0000, mode: 0 }, FileAction::Inherit { fd: 9 }], ok), Ok(()));
    }

    // T4-i, measured: an open action's O_CLOEXEC sticks only when the target is the lowest free slot.
    #[test]
    fn an_open_actions_cloexec_sticks_only_at_the_lowest_free_slot() {
        let mut f = FdTable::new();
        f.alloc(); // 3
        let mut st = SpawnState::new(f, BTreeSet::new(), BTreeSet::new());
        let open = |fd| FileAction::Open { fd, path: vec![], oflag: 0x0100_0000, mode: 0 };
        st.run(&[open(4), open(9)], &mut |_, _, _| Ok(None)).unwrap();
        assert!(st.cloexec.contains(&4), "4 was the lowest free slot");
        assert!(!st.cloexec.contains(&9), "9 was not, so its close-on-exec is cleared");
        assert_eq!(st.closed(0), vec![4]);
        // The next lowest free slot is 5: an open at 5 sticks again, one at 6 does not.
        st.run(&[open(6), open(5)], &mut |_, _, _| Ok(None)).unwrap();
        assert!(!st.cloexec.contains(&6) && st.cloexec.contains(&5));
    }
}
