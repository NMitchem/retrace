//! M54 t3 (spec 3c-3e): exec in place. `read_exec_call` reads the call out of guest memory, `exec_image`
//! replaces the image, and both record and replay call them with the same arguments (symmetry rule 1).
//! Everything that can refuse happens before `exec_image`: in `read_exec_call` (and, on record only, in
//! `retrace-core`'s `resolve`), so a failure past the point of no return is a retrace bug, never a
//! half-replaced guest.
use super::*;

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

/// task 4's `posix_spawn` plan (file actions, attributes). Uninhabited until then, so an `ExecCall` can
/// only be an `execve`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnPlan {}

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

    /// Read an `execve` (59) or `posix_spawn` (244) call out of guest memory. `Err(Errno)` is the path
    /// itself: `EFAULT` unreadable, `ENAMETOOLONG` past `PATH_MAX`. The argument vector's refusals ride in
    /// `ExecCall::args_refusal` (see its doc): E2BIG past native's limit, and the start-stack wall for a total
    /// past `START_ARGS_LIMIT` but under native's.
    ///
    /// A `posix_spawn` answers `EFAULT` until task 4: its descriptor is read (`spawn_flags`) by the record arm
    /// that routes it here, and the SETEXEC plan is task 4's.
    pub fn read_exec_call(&self, num: u64, args: [u64; 8]) -> Result<ExecCall, ExecRefusal> {
        if num == retrace_arch::SYS_POSIX_SPAWN { return Err(ExecRefusal::Errno(retrace_arch::EFAULT)); }
        let path = self.read_exec_cstr(args[0], PATH_MAX, ENAMETOOLONG)?;
        let (mut argv, mut envp, mut total) = (Vec::new(), Vec::new(), 0usize);
        let mut args_refusal = self.read_exec_strings(args[1], &mut total, &mut argv)
            .and_then(|()| self.read_exec_strings(args[2], &mut total, &mut envp)).err();
        if args_refusal.is_none() && total + 16 > START_ARGS_LIMIT {
            args_refusal = Some(ExecRefusal::Wall(retrace_arch::wall(WallCategory::ProcessCreation, format!(
                "execve with {} bytes of arguments, past retrace's 128 KiB start-stack limit", total + 16))));
        }
        if args_refusal.is_some() { argv.clear(); envp.clear(); }
        Ok(ExecCall { num, path, argv, envp, spawn: None, args_refusal })
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

    /// Ruling T3-a (interim; task 4 replaces it with C5): a signal pending after exec that the kept mask
    /// does not block would be taken by the new image at once, natively. The wall is `Signals`, by value, a
    /// pure function of box state, asked by the record arm before anything is appended.
    pub fn exec_release_wall(&self) -> Option<String> {
        let ignored = self.sigtable.ignored_after_exec(0);
        self.threads.exec_unmasked_pending(self.threads.current(), ignored).map(|sig| retrace_arch::wall(
            WallCategory::Signals, format!("execve with pending signal {sig} unmasked, which the new image would take at once")))
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

        // 5. Close descriptors: the host's `FD_CLOEXEC` set (`img.closed`) and every guest kqueue, which
        // exec closes by rule (a kqueue has no host descriptor to ask). The host descriptor of each goes on
        // record; retrace's own 0/1/2 (a console slot's host side) are never closed.
        let mut closed: Vec<u64> = img.closed.iter().map(|&f| u64::from(f)).collect();
        closed.extend(self.gkq.reset_for_exec());
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
        let ignored = self.sigtable.ignored_after_exec(0);
        self.sigtable.reset_for_exec(0);
        let caller = self.threads.current();
        let ctx = self.save_ctx();
        let id = self.threads.reset_for_exec(caller, ctx, ignored);
        debug_assert_eq!(id, tid, "the table assigned the id the TSD was staged with");
        self.reserve_believed_stack();
        Ok(())
    }
}
