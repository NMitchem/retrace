pub mod machmsg;
pub mod symbols;

use std::path::Path;
use std::rc::Rc;
use retrace_box::{Box_, Stop};
use retrace_trace::{Writer, Event, Region};
use retrace_arch::SYS_EXIT;
pub use retrace_box::thread::{BlockReason, ThreadState};

/// M15: one row of the debugger's thread listing.
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadSummary { pub tid: u32, pub state: ThreadState, pub is_current: bool }

// mmap flag bit: set => anonymous (M1's guest_mmap path); clear => file-backed (Task 8's
// anon-staged path — dyld maps the shared cache + dylibs this way).
const MAP_ANON: u64 = 0x1000;

// dyld's inline `__mac_syscall("Sandbox", ...)` loads this magic value into x16 (movz x16,
// #0x8000,lsl#16) — NOT a normal syscall selector. Only a platform binary (real dyld) may issue
// it; a normal process — and our forwarder — takes SIGSEGV. So it is synthesized, never forwarded.
const MAC_SYSCALL_MAGIC: u64 = 0x8000_0000;
// BSD syscall: dyld asks the kernel for the base of an already-mapped shared cache region. We
// force it to fail so dyld takes the DYLD_SHARED_REGION=private path and maps the cache file
// itself (through our anon-staged file-mmap), instead of using the host's kernel-mapped shared
// region, which lives in retrace's address space and is not in the guest's stage-2.
// Mach vm traps (negative x16). dyld uses these to manage its OWN address space; they must act on
// GUEST memory, never be forwarded to the host task (whose address space is retrace's own). We
// intercept them and allocate/free/relabel guest IPAs, exactly like the BSD mmap special cases.
const MACH_VM_ALLOCATE:   u64 = (-10i64) as u64; // _kernelrpc_mach_vm_allocate_trap(target,&addr,size,flags)
const MACH_VM_DEALLOCATE: u64 = (-12i64) as u64; // _kernelrpc_mach_vm_deallocate_trap(target,addr,size)
const MACH_VM_PROTECT:    u64 = (-14i64) as u64; // _kernelrpc_mach_vm_protect_trap(target,addr,size,setmax,prot)
const MACH_VM_MAP:        u64 = (-15i64) as u64; // _kernelrpc_mach_vm_map_trap(target,&addr,size,mask,flags,prot)
const MACH_MSG2: u64 = (-47i64) as u64; // mach_msg2_trap(data, options, bits|send_size, dest|reply, voucher|id, desc|rcv_name, rcv_size|prio, timeout)
const MACH_TASK_SELF: u64 = (-28i64) as u64; // task_self_trap: its result names the guest's task port
const VM_FLAGS_ANYWHERE:  u64 = 0x1;
const PROT_EXEC:          u64 = 0x4;

// Extract (address-pointer, size, flags, cur_prot) for an anonymous mach_vm_map/allocate trap.
fn vm_map_args(num: u64, args: &[u64; 8]) -> (u64, u64, u64, u64) {
    if num == MACH_VM_MAP { (args[1], args[2], args[4], args[5]) }
    else                  { (args[1], args[2], args[3], 0x3 /*RW*/) } // allocate: always RW anon
}

/// True if this `sysctl` is `{CTL_KERN, KERN_USRSTACK64}` — a 2-element mib read out of guest
/// memory. Shared by record and replay so both classify the trap identically (symmetry rule 1);
/// the read is `read_guest_checked` so an unreadable/partial mib is simply "not our mib" (it
/// forwards like every other sysctl) rather than a panic.
fn is_usrstack64_mib(b: &retrace_box::Box_, args: [u64; 8]) -> bool {
    if args[1] != 2 { return false; }
    let Some(raw) = b.read_guest_checked(args[0], 8) else { return false };
    let name0 = u32::from_le_bytes(raw[0..4].try_into().unwrap());
    let name1 = u32::from_le_bytes(raw[4..8].try_into().unwrap());
    name0 == retrace_arch::CTL_KERN && name1 == retrace_arch::KERN_USRSTACK64
}

/// How a recorded (or replayed) run ended: a clean exit, or a guest synchronous fault (M6). The
/// triple is deterministic — identical guest state faults identically — so replay byte-compares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Exit { code: u64 },
    Crash { pc: u64, esr: u64, far: u64 },
    /// M11: terminated by a signal the guest raised on itself, whose disposition resolved to the
    /// default fatal action. Terminal in the same shape M6 gave a fault.
    Signal { sig: u64 },
}

pub struct RecordSummary { pub stdout: Vec<u8>, pub outcome: Outcome, pub events: usize }

pub fn record(loaded: &retrace_guest::Loaded, trace_path: &Path) -> Result<RecordSummary, String> {
    record_box(Box_::load(loaded), trace_path)
}

/// Dynamic record: run the exe through real dyld (mapped via `load_dynamic`) and record. Same
/// record loop as the static path — dyld's syscalls/mach-traps flow through the shared engine.
pub fn record_dynamic(exe: &retrace_guest::Loaded, dyld: &retrace_guest::Loaded, argv: &[String],
                      trace_path: &Path) -> Result<RecordSummary, String> {
    record_box(Box_::load_dynamic(exe, dyld, argv), trace_path)
}

/// M16 Task 9: the decision made about a thread's pending set at an unmask (`sigprocmask`/
/// `pthread_sigmask`) or, since M17, a wake (`__ulock_wake`) that just made a peer runnable.
/// Written ONCE and called by record's mask and wake arms and replay's mirrors with the SAME
/// `(b, tid)`. That identity is what makes "both sides materialise the same signal" true by
/// construction instead of by two matches that could drift while both stayed green (symmetry
/// rule 1).
///
/// `Some((sig, handler))` is the only shape that produces a second landmark. `None` covers BOTH
/// "nothing was pending" and "the signal was discarded", which are indistinguishable to the trace
/// precisely because neither writes an event — and discarding is the kernel's own behaviour for a
/// signal whose disposition is ignore: it never runs anything, and the pending bit is gone.
///
/// `ThreadTable::take_deliverable` CLEARS the bit it returns, so this must be called exactly ONCE
/// per unmask or wake on each side, and a signal can never be materialised twice.
fn take_pending_delivery(b: &mut Box_, tid: usize) -> Option<(u64, u64)> {
    let sig = b.threads_mut().take_deliverable(tid)?;
    match b.sigtable().action(sig).disp {
        retrace_box::Disposition::Handler(handler) => Some((sig, handler)),
        retrace_box::Disposition::Ign => None,
        retrace_box::Disposition::Dfl => match retrace_arch::default_action(sig) {
            retrace_arch::DefaultAction::Ignore => None,
            // UNMODELLED, and loud about it rather than silently dropped. A signal whose default
            // action is Terminate, pended while masked and then unblocked, must KILL the process
            // here — which means appending `Event::Signal` plus the final full-memory snapshot and
            // breaking the record loop, and the matching terminal mirror on the replay side, i.e.
            // the shape record's terminal raise arm already has. That is a real piece of work, not
            // a line, and no guest reaches it (nothing in dyld/libSystem startup raises a signal;
            // `abort()` unblocks SIGABRT before raising it). Dropping it silently would turn a
            // process death into a clean exit — the worst possible failure for a determinism
            // oracle, because the recording and the replay would agree with each other and both be
            // wrong. Both sides reach this panic at the same landmark.
            retrace_arch::DefaultAction::Terminate => panic!(
                "signal {sig} became deliverable at an unmask or a wake and its default action is \
                 Terminate: the process must die here, and the terminal path (Event::Signal + final \
                 snapshot + break, plus its replay mirror) is not modelled at either landmark. \
                 Implement it before a guest needs this; do not drop the signal."),
        },
    }
}

fn record_box(mut b: Box_, trace_path: &Path) -> Result<RecordSummary, String> {
    let mut w = Writer::create(trace_path).map_err(|e| format!("create trace: {e}"))?;
    let mut count = 0usize;
    w.append(&b.snapshot()).map_err(|e| format!("append snapshot: {e}"))?; count += 1;

    let mut stdout = Vec::new();
    let outcome;
    // Bring-up diagnostic (RETRACE_TRACE=1): log every trap the record loop dispatches, so a
    // forwarded syscall/mach-trap that misbehaves is identifiable from the last line printed.
    let trace_log = std::env::var_os("RETRACE_TRACE").is_some();
    // The guest's task-port NAME (the result of task_self_trap −28, still host-forwarded this
    // milestone): machmsg routing needs it to recognize task-destined kernel RPCs. Learned
    // identically on record (forwarded result) and replay (recorded result).
    let mut guest_task_port: Option<u64> = None;
    loop {
        let stop = b.run();
        // M15: the thread that produced this stop, captured ONCE and used by every append arm
        // below. Read here rather than at each append because this is the only point guaranteed to
        // be the trapping thread: `run()` reschedules at ENTRY, and no handler between here and the
        // append moves `current` (block/exit_current change state only). One source of the value,
        // ~35 uses — a stale or defaulted tag is the failure mode this ordering removes.
        let thread = b.threads().current() as u32;
        if trace_log {
            if let Stop::Syscall { num, args } = &stop {
                eprintln!("[trap] num={} (0x{:x}) pc={:#x} args=[{:#x},{:#x},{:#x},{:#x},{:#x},{:#x}]",
                    *num as i64, num, b.position(), args[0], args[1], args[2], args[3], args[4], args[5]);
                // Echo dyld's fd-1/2 diagnostics so a fatal error message is visible.
                if retrace_arch::is_console_write(*num, args[0]) {
                    let bytes = b.read_guest(args[1], args[2] as usize);
                    eprintln!("[fd{}] {}", args[0], String::from_utf8_lossy(&bytes));
                }
                // M2-mach diagnostic: decode + hexdump mach_msg2 sends (golden capture for the codec).
                if *num == MACH_MSG2 {
                    let send_size = ((args[2] >> 32) as usize).min(256);
                    eprintln!("[mach_msg2] msgh_id={} dest={:#x} reply={:#x} options={:#x} bits={:#x} send_size={} rcv_size={}",
                        args[4] >> 32, args[3] & 0xffff_ffff, args[3] >> 32, args[1],
                        args[2] & 0xffff_ffff, args[2] >> 32, args[6] & 0xffff_ffff);
                    for (i, chunk) in b.read_guest(args[0], send_size).chunks(16).enumerate() {
                        eprintln!("  send+{:03x}: {}", i * 16,
                            chunk.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" "));
                    }
                }
            }
            if let Stop::Fault { pc, esr, far } = &stop {
                // The terminal trap. CLAUDE.md advertises this flag as logging every dispatched
                // trap, and a crash is the one an M7-style bring-up most needs to see.
                eprintln!("[fault] pc={:#x} esr={:#x} far={:#x} ec={:#x}",
                    *pc, *esr, *far, (*esr >> 26) & 0x3f);
            }
        }
        match stop {
            Stop::Syscall { num, args } if num == SYS_EXIT => {
                let final_snap = b.snapshot();          // final-memory landmark
                // M17: the clean-exit path only — see `assert_no_stranded_signals`. The crash path
                // deliberately does not call this: a guest already dying must be diagnosed by its
                // crash, not by a secondary guard firing on top of it.
                b.assert_no_stranded_signals();
                w.append(&Event::Exit { code: args[0], thread }).map_err(|e| format!("append exit: {e}"))?; count += 1;
                w.append(&final_snap).map_err(|e| format!("append final snapshot: {e}"))?; count += 1;
                outcome = Outcome::Exit { code: args[0] };
                break;
            }
            // M6: a stage-1 guest fault ends the recording as a CRASH — a successful recording
            // (mirror: replay's crash verify in advance()). Same terminal shape as exit:
            // Event::Crash, then the final full-memory snapshot.
            //
            // M12: unless the guest installed a handler for the signal that fault maps to, in which
            // case it is DELIVERED instead. The disposition check comes FIRST. Before M12 this arm
            // never consulted sigtable at all, so a guest that installed a SIGSEGV handler and then
            // faulted was recorded as a terminal crash with its handler silently skipped.
            //
            // Only Stop::Fault is touched. Demand paging (page_in_cache, commit_reserved_page)
            // arrives as Stop::Other — a stage-2 abort — so this cannot steal a demand-paging case,
            // for exactly the reason M6's arm couldn't.
            Stop::Fault { pc, esr, far } => {
                let (sig, si_code) = retrace_arch::signal_of_esr(esr);
                if let retrace_box::Disposition::Handler(handler) = b.sigtable().action(sig).disp {
                    assert!(!b.threads().is_blocked_for(thread as usize, sig),
                        "raising blocked signal {sig} synchronously is not modelled: a fault cannot \
                         be deferred, POSIX leaves it undefined, and Darwin force-delivers. M11 \
                         models no pending set, so implement one — and revisit sigpending's \
                         always-empty answer — before a guest needs this.");
                    // C9: `deliver_signal_to(thread, …)`, not the `deliver_signal(…)` shorthand it
                    // delegates to. Behaviourally identical — the shorthand IS
                    // `deliver_signal_to(current)` — but replay's mirror spells the thread out, and
                    // this branch works hard to keep the two sides grep-identical so a reader can
                    // see symmetry rule 1 holding rather than having to chase a delegation to
                    // confirm it.
                    let (writes, resume_pc) =
                        b.deliver_signal_to(thread as usize, sig, si_code, far, esr, far);
                    // A hardware fault has no target port to resolve — it is always attributed to
                    // whichever thread's vCPU context trapped — so `current` is permanently correct
                    // on this path. Contrast the __pthread_kill delivery below, which resolves its
                    // target thread from the port the guest named (M16).
                    w.append(&Event::SignalDelivery { sig, si_code, si_addr: far, handler,
                                                      resume_pc, writes, thread })
                        .map_err(|e| format!("append signal delivery: {e}"))?; count += 1;
                    continue;
                }
                let final_snap = b.snapshot();
                w.append(&Event::Crash { pc, esr, far, thread }).map_err(|e| format!("append crash: {e}"))?; count += 1;
                w.append(&final_snap).map_err(|e| format!("append final snapshot: {e}"))?; count += 1;
                outcome = Outcome::Crash { pc, esr, far };
                break;
            }
            // Console writes are mirrored + faked (NOT forwarded) so record doesn't emit to the
            // record process's real stdout AND double the mirror; replay reproduces from the mirror.
            // `is_console_write` covers write AND write_nocancel — the shared predicate is what
            // keeps this arm and replay's mirror from drifting (M9; see its doc comment).
            Stop::Syscall { num, args } if retrace_arch::is_console_write(num, args[0]) => {
                stdout.extend_from_slice(&b.read_guest(args[1], args[2] as usize));
                let ret = args[2];
                w.append(&Event::Syscall { num, args, ret, err: false, writes: vec![], thread }).map_err(|e| format!("append write: {e}"))?; count += 1;
                b.set_x0_err_and_return(ret, false);
            }
            // A guest close of fd 0/1/2 is FAKED, never forwarded: those descriptors are retrace's
            // own (see `is_console_close`), so forwarding lets the guest close retrace's stdout out
            // from under it — after which the CLI prints the mirrored recording into a closed fd and
            // the run reports success having emitted nothing. Measured with jq, whose exit path does
            // exactly this. Reports success, which is what a real close(1) would do.
            //
            // No replay mirror arm is needed (contrast the console write): this appends an ordinary
            // recorded syscall whose (ret=0, err=false, no writes) replay reproduces through the
            // generic `apply_and_return`, and whose (num, args) the divergence oracle still checks.
            //
            // Deferred: retrace does not model the fd as CLOSED afterwards, so a guest that wrote to
            // fd 1 after closing it would see the write succeed instead of EBADF. No guest in the
            // gate does; modeling it means giving the box a real fd table.
            Stop::Syscall { num, args } if retrace_arch::is_console_close(num, args[0]) => {
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![], thread }).map_err(|e| format!("append close: {e}"))?; count += 1;
                b.set_x0_err_and_return(0, false);
            }
            // mmap is special-cased: it creates guest memory the program then writes with plain
            // stores (no syscall), so it cannot go through forward_and_diff. guest_mmap maps a
            // deterministically-addressed tracked backing and returns its IPA. Anon vs
            // file-backed is split on the MAP_ANON flag bit (dyld maps the shared cache +
            // dylibs file-backed; SPTM forbids ever hv_vm_map'ing a file page, so file-backed
            // mmap is anon-staged: pread the file into anon pages and record the bytes as writes).
            Stop::Syscall { num, args } if num == retrace_arch::SYS_MMAP && args[3] & MAP_ANON != 0 => {
                // Minor (b): an anonymous PROT_EXEC (JIT) mmap would need exec promotion but
                // guest_mmap installs plain RW+non-exec data pages. JIT is out of M2 scope; warn
                // loudly rather than silently hand back a non-exec page the guest can't run.
                if args[2] & 0x4 != 0 {
                    eprintln!("[retrace warn] anon PROT_EXEC mmap (len {:#x}) not promoted to exec (JIT out of M2 scope)", args[1]);
                }
                // A MAP_FIXED address the guest's own space cannot hold is refused with an errno —
                // the kernel's answer — and recorded as an ordinary failed syscall. Replay
                // recomputes the same verdict from the same pure geometry check and byte-compares
                // (ret, err) against the recording, so the symmetry is structural.
                let (ret, err) = match b.guest_mmap(args[0], args[1], args[2], args[3]) {
                    Ok(ipa) => (ipa, false),
                    Err(errno) => (errno, true),
                };
                w.append(&Event::Syscall { num, args, ret, err, writes: vec![], thread }).map_err(|e| format!("append mmap: {e}"))?; count += 1;
                b.set_x0_err_and_return(ret, err);
            }
            Stop::Syscall { num, args } if num == retrace_arch::SYS_MMAP => {
                // Same FIXED-address refusal as the anonymous path above; nothing was mapped, so
                // there is no region to promote and no staged bytes to record.
                let (ret, err, writes) = match b.guest_mmap_file(args[0], args[1], args[2], args[3], args[4] as i32, args[5]) {
                    Ok((ipa, writes)) => {
                        // PROT_EXEC (0x4): promote the freshly-mapped region to RO+exec (ATTR_CODE)
                        // stage-1 pages so the guest can execute from it under W^X (e.g. dyld
                        // mapping the shared cache's __TEXT). Done BEFORE resuming the guest, on
                        // record AND replay.
                        if args[2] & 0x4 != 0 { b.set_region_exec(ipa, args[1]); }
                        (ipa, false, writes)
                    }
                    Err(errno) => (errno, true, vec![]),
                };
                w.append(&Event::Syscall { num, args, ret, err, writes, thread }).map_err(|e| format!("append mmap_file: {e}"))?; count += 1;
                b.set_x0_err_and_return(ret, err);
            }
            // munmap/mprotect (debt #2): honor them for real — drop + hv_vm_unmap the backing on
            // munmap so a later mmap can reuse the address; best-effort hv_vm_protect on
            // mprotect. Neither writes guest memory itself (the guest's own subsequent stores
            // do), so they're recorded like mmap: ret=0, no writes, reproduced by re-execution.
            Stop::Syscall { num, args } if num == retrace_arch::SYS_MUNMAP => {
                b.guest_munmap(args[0], args[1]);
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![], thread }).map_err(|e| format!("append munmap: {e}"))?; count += 1;
                b.set_x0_err_and_return(0, false);
            }
            Stop::Syscall { num, args } if num == retrace_arch::SYS_MPROTECT => {
                b.guest_mprotect(args[0], args[1], args[2]);
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![], thread }).map_err(|e| format!("append mprotect: {e}"))?; count += 1;
                b.set_x0_err_and_return(0, false);
            }
            // sysctl({CTL_KERN, KERN_USRSTACK64}): answer from the guest's OWN stack top (M8-stack).
            // Forwarding this returns RETRACE's host-process stack address — ASLR'd, different every
            // run — which the guest then uses as a guest address (libstd derives its guard page from
            // it). That is semantically wrong independent of determinism, exactly like M2-cpuid's
            // TPIDR_EL0. Every other mib keeps forwarding, unchanged. The answer is deterministic, so
            // this takes the STANDARD symmetric posture: replay recomputes these same bytes and
            // byte-compares them against the recording (symmetry rule 1).
            Stop::Syscall { num, args } if num == retrace_arch::SYS_SYSCTL
                && is_usrstack64_mib(&b, args) => {
                let writes = b.usrstack64_reply(args);
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: writes.clone(), thread })
                    .map_err(|e| format!("append sysctl usrstack64: {e}"))?; count += 1;
                b.apply_and_return(0, false, &writes);
            }
            // getrlimit(RLIMIT_STACK): answer from the guest's own stack size. Forwarding returns
            // the HOST's limits (measured: 8176 KiB soft / 65520 KiB hard), and libstd subtracts
            // this from usrstack64 to locate its guard page — the two must describe the SAME stack
            // or the result is a wild address. The guest passes RLIMIT_STACK | _RLIMIT_POSIX_FLAG
            // (0x1003), so mask before comparing. Deterministic => STANDARD symmetric posture,
            // exactly like the usrstack64 arm above.
            Stop::Syscall { num, args } if num == retrace_arch::SYS_GETRLIMIT
                && (args[0] & !retrace_arch::RLIMIT_POSIX_FLAG) == retrace_arch::RLIMIT_STACK => {
                let writes = b.rlimit_stack_reply(args);
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: writes.clone(), thread })
                    .map_err(|e| format!("append getrlimit stack: {e}"))?; count += 1;
                b.apply_and_return(0, false, &writes);
            }
            // shared_region_check_np (#294): pin the cache slide to 0 by reporting the UNSLID base
            // (0x180000000) as the shared region's start — dyld then computes slide 0 and lays the
            // cache at exactly the VAs page_in_cache maps. Writes the base into the guest out-pointer
            // (arg0) and returns success; regenerated identically on replay via the generic apply.
            //
            // Reporting success tells dyld the cache is ALREADY mapped at this base, so dyld reads it
            // directly and never calls #536 — therefore the demand-pager must be installed HERE (not
            // deferred to #536, which never fires for a cache dyld already believes is present). Done
            // on record AND replay so both page identical bytes.
            Stop::Syscall { num, args } if num == retrace_arch::SYS_SHARED_REGION_CHECK_NP => {
                b.install_cache_pager();
                if b.is_mapped(args[0]) {
                    let writes = vec![Region { ipa: args[0], bytes: retrace_box::SHARED_REGION_START.to_le_bytes().to_vec() }];
                    w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: writes.clone(), thread }).map_err(|e| format!("append shared_region_check: {e}"))?; count += 1;
                    b.apply_and_return(0, false, &writes);
                } else {
                    // dyld's deliberate error path (e.g. `shared_region_check_np((void*)-1)` to
                    // return a failure code): the kernel's copyout to the bad pointer yields EFAULT.
                    // Reproduce it deterministically — carry set, x0 = EFAULT, no writes.
                    const EFAULT: u64 = 14;
                    w.append(&Event::Syscall { num, args, ret: EFAULT, err: true, writes: vec![], thread }).map_err(|e| format!("append shared_region_check(bad ptr): {e}"))?; count += 1;
                    b.set_x0_err_and_return(EFAULT, true);
                }
            }
            // shared_region_map_and_slide_2_np (#536): the kernel cache-mapping syscall. We do NOT
            // map here — the cache is lazily demand-paged (page_in_cache) on stage-2 faults. Install
            // the pager and return success. Installed on BOTH record and replay so both page
            // identical bytes; no cache bytes are ever written to the trace.
            Stop::Syscall { num, args } if num == retrace_arch::SYS_SHARED_REGION_MAP_AND_SLIDE_2_NP => {
                b.install_cache_pager();
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![], thread }).map_err(|e| format!("append shared_region_map: {e}"))?; count += 1;
                b.set_x0_err_and_return(0, false);
            }
            // dyld's inline __mac_syscall sandbox check (x16 = MAC_SYSCALL_MAGIC): cannot be
            // forwarded (host faults) — synthesize the unsandboxed result deterministically:
            // success (x0=0) and the out buffer (x2) cleared to 0 (= "not in a sandbox"). Recorded
            // as a normal syscall event so replay reproduces it via the generic apply path.
            Stop::Syscall { num, args } if num == MAC_SYSCALL_MAGIC => {
                eprintln!("[retrace warn] dyld __mac_syscall(Sandbox) synthesized as success/unsandboxed (not forwarded; host would fault)");
                // Clear the out-buffer (arg2) ONLY when it is a real mapped pointer — the on-disk
                // dyld passes a query buffer there, but the cache-resident dyld's check passes a null
                // arg2 (result is purely the x0 return). Writing 8 bytes to a null/unmapped arg2
                // would panic apply_and_return.
                let writes = if args[2] != 0 && b.is_mapped(args[2]) {
                    vec![Region { ipa: args[2], bytes: vec![0u8; 8] }]
                } else { vec![] };
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: writes.clone(), thread }).map_err(|e| format!("append mac_syscall: {e}"))?; count += 1;
                b.apply_and_return(0, false, &writes);
            }
            // mach_vm_allocate / mach_vm_map: allocate anonymous GUEST memory (never forward). The
            // kernel writes the chosen address into *args[1]; we allocate a deterministic guest IPA
            // and store it there, returning KERN_SUCCESS.
            Stop::Syscall { num, args } if num == MACH_VM_ALLOCATE || num == MACH_VM_MAP => {
                let (addr_ptr, size, flags, prot) = vm_map_args(num, &args);
                let anywhere = flags & VM_FLAGS_ANYWHERE != 0;
                let exec = prot & PROT_EXEC != 0;
                if exec { eprintln!("[retrace warn] mach_vm exec mapping (prot={prot:#x}) promoted to RO+exec"); }
                let req = if b.is_mapped(addr_ptr) { b.read_u64(addr_ptr) } else { 0 }; // hint (honored when free)
                // cur_protection == 0 => a PROT_NONE address-space reservation (bookkeeping only,
                // demand-committed page-by-page on first touch by commit_reserved_page); anything
                // else is an eagerly-backed map. Mirrors the MIG 4811 split below (guest_vm_reserve
                // vs guest_vm_map) so a reservation arriving via the trap route genuinely reserves
                // and is never eager-backed (fatal at 24 GiB). MACH_VM_ALLOCATE always carries RW.
                let ipa = if prot == 0 {
                    b.guest_vm_reserve(req, size, anywhere)
                } else {
                    b.guest_vm_map(req, size, anywhere, exec)
                };
                let writes = vec![Region { ipa: addr_ptr, bytes: ipa.to_le_bytes().to_vec() }];
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: writes.clone(), thread }).map_err(|e| format!("append mach_vm_map: {e}"))?; count += 1;
                b.apply_and_return(0, false, &writes);
            }
            // mach_vm_deallocate: free guest memory (drop the backing + stage-2 unmap).
            Stop::Syscall { num, args } if num == MACH_VM_DEALLOCATE => {
                b.guest_munmap(args[1], args[2]);
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![], thread }).map_err(|e| format!("append mach_vm_dealloc: {e}"))?; count += 1;
                b.set_x0_err_and_return(0, false);
            }
            // mach_vm_protect: M13 routes it into the box like mprotect(74), through the SAME
            // `guest_mprotect` so record and replay cannot drift. Only `prot == 0` changes anything
            // (see guest_mprotect); every other value keeps the pre-M13 no-op-success behavior, so
            // dyld's RW→RO fixup protects are unaffected — Task 2 measured hello_rust's 47 calls as
            // 0x1/0x3/0x13 and never 0. `set_maximum` (args[3]) is ignored: M13 models current
            // protection only. Writes nothing itself, so it records like mprotect.
            Stop::Syscall { num, args } if num == MACH_VM_PROTECT => {
                b.guest_mprotect(args[1], args[2], args[4]);
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![], thread }).map_err(|e| format!("append mach_vm_protect: {e}"))?; count += 1;
                b.set_x0_err_and_return(0, false);
            }
            // mach_msg2 (−47): MIG kernel RPCs. Address-space ops are serviced against GUEST
            // IPAs (forwarding them lets the host kernel mutate retrace's own address space —
            // the M2-mach wall); a decided read-only/create-once allowlist still forwards;
            // anything unrecognized fails loudly with its decoded name (spec §Mechanism).
            Stop::Syscall { num, args } if num == MACH_MSG2 => {
                let m = machmsg::Msg2::unpack(&args);
                assert!(m.send_size as usize <= 0x1000,
                    "mach_msg2 send_size {:#x} implausibly large", m.send_size);
                match machmsg::route(&m, guest_task_port) {
                    machmsg::Route::ServiceVmMap => {
                        let buf = b.read_guest(m.data, m.send_size as usize);
                        let req = machmsg::decode_vm_map(&buf)
                            .unwrap_or_else(|e| panic!("mach_vm_map (4811) decode: {e}"));
                        let anywhere = req.flags as u64 & VM_FLAGS_ANYWHERE != 0;
                        // cur_protection == 0 => a PROT_NONE address-space reservation (no backing,
                        // e.g. libmalloc's 24 GiB nano pointer range); anything else is a real
                        // backed map. See guest_vm_reserve / guest_vm_map.
                        let ipa = if req.cur_protection == 0 {
                            b.guest_vm_reserve(req.address, req.size, anywhere)
                        } else {
                            let exec = req.cur_protection as u64 & PROT_EXEC != 0;
                            b.guest_vm_map(req.address, req.size, anywhere, exec)
                        };
                        let writes = vec![Region { ipa: m.data,
                            bytes: machmsg::encode_vm_map_reply(m.reply_port, ipa) }];
                        w.append(&Event::Syscall { num, args, ret: machmsg::MACH_MSG_SUCCESS,
                            err: false, writes: writes.clone(), thread })
                            .map_err(|e| format!("append mach_msg2 vm_map: {e}"))?; count += 1;
                        b.apply_and_return(machmsg::MACH_MSG_SUCCESS, false, &writes);
                    }
                    machmsg::Route::ServiceGetSpecialPort => {
                        // task_get_special_port(3409): libxpc's initializer fetches TASK_BOOTSTRAP_PORT.
                        // Answer with a REAL kernel-valid send right minted in retrace's OWN IPC space
                        // (M2-xpcport) — never forwarded (that would hand over the host's real launchd
                        // port). The minted name is nondeterministic, so it is RECORDED here and replay
                        // applies it verbatim (the task_self posture). Only which==4 modeled.
                        let buf = b.read_guest(m.data, m.send_size as usize);
                        let which = machmsg::decode_get_special_port(&buf)
                            .unwrap_or_else(|e| panic!("task_get_special_port (3409) decode: {e}"));
                        assert_eq!(which, 4,
                            "only TASK_BOOTSTRAP_PORT (4) is modeled; got which={which}");
                        let name = b.mint_bootstrap_port();
                        let writes = vec![Region { ipa: m.data,
                            bytes: machmsg::encode_get_special_port_reply(m.reply_port, name) }];
                        w.append(&Event::Syscall { num, args, ret: machmsg::MACH_MSG_SUCCESS,
                            err: false, writes: writes.clone(), thread })
                            .map_err(|e| format!("append mach_msg2 get_special_port: {e}"))?; count += 1;
                        b.apply_and_return(machmsg::MACH_MSG_SUCCESS, false, &writes);
                    }
                    machmsg::Route::ServiceSetSpecialPort => {
                        // task_set_special_port(3410): libsystem_trace's initializer sets its
                        // TASK_DEBUG_CONTROL_PORT. No out-params → reply a mig_reply_error KERN_SUCCESS
                        // (id 3510) — never forwarded (would set retrace's OWN debug-control port); the
                        // inbound COPY_SEND descriptor is ignored. Only which==10 modeled. The reply is
                        // DETERMINISTIC → STANDARD symmetric posture (replay recomputes + byte-compares).
                        let buf = b.read_guest(m.data, m.send_size as usize);
                        let which = machmsg::decode_set_special_port(&buf)
                            .unwrap_or_else(|e| panic!("task_set_special_port (3410) decode: {e}"));
                        assert_eq!(which, 10,
                            "only TASK_DEBUG_CONTROL_PORT (10) is modeled; got which={which}");
                        let writes = vec![Region { ipa: m.data,
                            bytes: machmsg::encode_mig_error(m.msgh_id, m.reply_port, machmsg::KERN_SUCCESS) }];
                        w.append(&Event::Syscall { num, args, ret: machmsg::MACH_MSG_SUCCESS,
                            err: false, writes: writes.clone(), thread })
                            .map_err(|e| format!("append mach_msg2 set_special_port: {e}"))?; count += 1;
                        b.apply_and_return(machmsg::MACH_MSG_SUCCESS, false, &writes);
                    }
                    machmsg::Route::StubMigReply(retcode) => {
                        // Optional/no-op kernel routine (no out-params): reply with a mig_reply_error
                        // carrying `retcode` (chosen in route() — 4822 vm_reclaim => KERN_NOT_SUPPORTED
                        // so libmalloc takes its no-reclaim fallback; 8000 task_restartable => success).
                        // Retcode tolerance verified in the Task 7 walk.
                        let writes = vec![Region { ipa: m.data,
                            bytes: machmsg::encode_mig_error(m.msgh_id, m.reply_port, retcode) }];
                        w.append(&Event::Syscall { num, args, ret: machmsg::MACH_MSG_SUCCESS,
                            err: false, writes: writes.clone(), thread })
                            .map_err(|e| format!("append mach_msg2 stub: {e}"))?; count += 1;
                        b.apply_and_return(machmsg::MACH_MSG_SUCCESS, false, &writes);
                    }
                    machmsg::Route::Forward(name) => {
                        eprintln!("[retrace] forwarding mach_msg2 {name} (msgh_id {}) to host (decided allowlist)", m.msgh_id);
                        let (ret, err, writes) = b.forward_and_diff(num, args);
                        if trace_log {
                            eprintln!("[mach_msg2] host ret={ret:#x} err={err}");
                            for w_ in &writes {
                                let shown = &w_.bytes[..w_.bytes.len().min(256)];
                                for (i, chunk) in shown.chunks(16).enumerate() {
                                    eprintln!("  reply@{:#x}+{:03x}: {}", w_.ipa, i * 16,
                                        chunk.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" "));
                                }
                            }
                        }
                        w.append(&Event::Syscall { num, args, ret, err, writes, thread })
                            .map_err(|e| format!("append mach_msg2 fwd: {e}"))?; count += 1;
                        b.set_x0_err_and_return(ret, err);
                    }
                    machmsg::Route::Unsupported(why) => {
                        if trace_log { eprintln!("[regs]\n{}\n[bt]\n{}", b.dbg_regs(), b.dbg_backtrace(24)); }
                        return Err(format!("unsupported mach_msg2 at pc {:#x}: {why}", b.position()));
                    }
                }
            }
            // M18 Stage 2b: the mach semaphore WAIT (see `Box_::guest_sem_wait`). EMULATED, never
            // forwarded — forwarding it blocks retrace's OWN process forever on a semaphore only
            // the guest's worker could signal, which is exactly what both Stage 2a measurement runs
            // did (zero bytes of guest stdout; the one run whose exit code was captured was killed
            // by the external alarm). Writes nothing to guest memory — it only moves thread-table
            // state — so the event carries no writes, the same posture as the ulock pair's arms
            // below. `err` is hardcoded `false` because `guest_sem_wait` has no failure path; the
            // replay mirror recomputes that same constant and compares it, which is what keeps the
            // hardcode honest rather than merely quiet.
            //
            // `set_x0_err_and_return` is called even though the thread has just blocked, and that
            // is the `SYS_ULOCK_WAIT` arm's shape exactly: x0 is being set for the return this
            // thread will eventually resume THROUGH, and the next entry to `Box_::run()` sees
            // `needs_reschedule()` and switches to the worker. The switch itself stays below the
            // trace (symmetry rule 2), so nothing about the schedule is recorded.
            //
            // **The ORDER of these two arms is load-bearing.** They sit BEFORE the generic
            // negative-trap arm, whose first statement is Task 2's family-wide
            // `is_mach_semaphore_trap` guard; placed after it they would be dead code that compiles,
            // passes clippy, and silently hit the guard instead. The guard itself stays exactly
            // where it is: the other five stubs of the verified `-39..=-33` family are still
            // unserviced and must keep reaching it.
            Stop::Syscall { num, args } if num == retrace_arch::MACH_SEMAPHORE_WAIT => {
                let rc = b.guest_sem_wait(args);
                w.append(&Event::Syscall { num, args, ret: rc, err: false, writes: vec![], thread })
                    .map_err(|e| format!("append sem_wait: {e}"))?; count += 1;
                b.set_x0_err_and_return(rc, false);
            }
            // M18 Stage 2b: the mach semaphore SIGNAL (see `Box_::guest_sem_signal`). Never
            // forwarded, for the wait arm's reason exactly.
            //
            // This landmark does NOT appear once per guest signal, and nothing here may assume it
            // does. §5 item 7 of
            // `docs/superpowers/specs/2026-08-23-retrace-m18-stage2b-wqthread-measurements.md`
            // measured `dispatch_semaphore_signal`'s FAST path as a bare `ldaddl` on the count word
            // at `sem+0x30`, inside libdispatch's own object, issuing no trap at all — only the slow
            // path (a waiter exists) falls through to the trap. So an EMPTY `woken` is an ordinary
            // outcome here, not a lost wake, and neither this arm nor its mirror may read it as one.
            Stop::Syscall { num, args } if num == retrace_arch::MACH_SEMAPHORE_SIGNAL => {
                let (rc, woken) = b.guest_sem_signal(args);
                w.append(&Event::Syscall { num, args, ret: rc, err: false, writes: vec![], thread })
                    .map_err(|e| format!("append sem_signal: {e}"))?; count += 1;
                b.set_x0_err_and_return(rc, false);

                // M17 materialisation at this wake: **ASSERTED, not mirrored** — the deliberate
                // choice, made identically on both sides so record and replay cannot drift on it.
                //
                // `woken` may not simply be dropped. `Box_::should_pend_for` pends a signal on ANY
                // `Blocked(_)` target, `Sem { .. }` included, while `assert_no_stranded_signals`
                // scans `Blocked` threads ONLY — so a signal pended on a semaphore waiter that this
                // arm wakes would leave the thread Runnable with the bit still set and vanish, while
                // record and replay agreed with each other. That is the one failure class a
                // determinism oracle cannot see, which is why it gets a guard and not a comment.
                //
                // Why a guard rather than a copy of the `SYS_ULOCK_WAKE` arm below: that arm does
                // not merely deliver, it calls `complete_saved_syscall_before_delivery(wtid, false)`
                // against the woken thread's SAVED context, and that `false` is a MEASUREMENT — M17
                // Task 4b measured a `__ulock_wait`-blocked thread's saved x0 as 0 with its saved
                // SPSR left C-SET (`crates/retrace/tests/blockedctx.rs`). Nothing has measured the
                // equivalent for a thread parked in `semaphore_wait_trap`, and no fixture in this
                // tree pends a signal on one. Copying M17's correction here would be guessing at
                // unmeasured saved state — the thing this file refuses BY VALUE everywhere else
                // (`guest_workq_kernreturn`'s opcode refusal, the dup2 guard, the
                // `deliver_to.len() <= 1` bound). So it fails loud the day a fixture produces it,
                // naming the measurement that is owed first.
                let deliverable: Vec<usize> = woken.iter().copied()
                    .filter(|&t| b.threads().peek_deliverable(t).is_some())
                    .collect();
                assert!(deliverable.is_empty(),
                    "semaphore signal woke thread(s) {deliverable:?} carrying a pending deliverable \
                     signal. M18 Stage 2b deliberately does NOT materialise at this wake: unlike \
                     __ulock_wake, the saved context of a semaphore-parked thread is unmeasured, so \
                     the completed-syscall correction M17 applies there would be a guess here — and \
                     waking the thread without materialising strands the signal where \
                     assert_no_stranded_signals cannot see it. Measure the parked thread's saved \
                     x0/SPSR (the blockedctx.rs shape) and mirror the SYS_ULOCK_WAKE arm on BOTH \
                     sides before allowing this.");
            }
            // Mach traps arrive as `svc #0x80` with a NEGATIVE trap number in x16. They forward +
            // memory-diff exactly like a BSD syscall (a negative x16 is a valid mach-trap selector
            // to the kernel; the reply is either in x0 — captured as `ret` — or written into a
            // guest message buffer — captured as `writes`). Special cases that hand back fresh
            // kernel state the diff can't reproduce (ports mapped into the guest, allocations that
            // must land in guest IPA space) are added here as they are discovered.
            Stop::Syscall { num, args } if (num as i64) < 0 => {
                // M18 Stage 2b: no mach semaphore trap may reach here. Forwarding one is not
                // whole-process-fatal the way forwarding the workq pair is, but it is
                // whole-process-HANGING, which is just as fatal to a recording: both Stage 2a
                // measurement runs blocked here forever and produced zero bytes of guest stdout.
                //
                // Guarded by FAMILY (`-39..=-33`), not by the wait/signal pair alone — fix round 1,
                // finding 5. The other five stubs hang identically, and a hang is expensive to
                // diagnose precisely because it produces no output to diagnose it with.
                //
                // This guard's SHAPE is the one the workq pair uses on the generic BSD forward arm,
                // but deliberately not its LOCATION: that arm is BSD-only and a negative trap
                // number never reaches it (measurements doc §2, corrected in 60cea11). Negative
                // traps are caught here, so the guard belongs here — and it sits BEFORE the
                // forward, which is the whole point: after it, the process is already blocked and
                // there is nothing left to assert from.
                assert!(!retrace_arch::is_mach_semaphore_trap(num),
                    "M18 Stage 2b: mach semaphore trap {num:#x} ({}) reached the generic forward \
                     arm — it must be serviced by its dedicated arm above (Box_::guest_sem_wait / \
                     guest_sem_signal). Forwarding it blocks retrace's own process forever on a \
                     semaphore only the guest's worker could signal. args={args:#x?}",
                    num as i64);
                let (ret, err, writes) = b.forward_and_diff(num, args);
                // Learn the guest's task-port name from task_self_trap (−28) so machmsg routing can
                // recognize task-destined kernel RPCs. Mirrored on replay from the recorded result.
                if num == MACH_TASK_SELF && !err { guest_task_port = Some(ret); }
                w.append(&Event::Syscall { num, args, ret, err, writes, thread }).map_err(|e| format!("append mach-trap: {e}"))?; count += 1;
                b.set_x0_err_and_return(ret, err);
            }
            // ---- M11-signals ---------------------------------------------------------------
            // Placed ABOVE the generic forward arm on purpose: that ordering is what keeps
            // forward_and_diff — which issues a raw svc in RETRACE's process — from ever seeing a
            // signal syscall. Before M11, `__pthread_kill(self, SIGABRT)` killed the recorder,
            // `sigaction` installed a guest VA as the RECORDER's handler (measured: hello_rust's
            // SIGSEGV query read back retrace's own libstd handler), and `kill` reached any host
            // pid. All three are gone by construction here, not by guard.
            //
            // Serviced state calls. Never forwarded; each synthesizes its own writeback and appends
            // an ordinary Event::Syscall, so the divergence oracle still checks (num, args) and
            // RETRACE_TRACE=1 still shows the sequence. Replay mirrors these.
            Stop::Syscall { num, args } if num == retrace_arch::SYS_SIGACTION => {
                let sig = args[0];
                let new = if args[1] != 0 {
                    Some(retrace_box::decode_act(&b.read_guest(args[1], 24)))
                } else { None };
                let old = match new {
                    Some(a) => b.sigtable_mut().set_action(sig, a),
                    None => b.sigtable().action(sig),
                };
                // oldact is `struct sigaction` — 16 bytes, NOT the 24-byte input struct.
                let writes = if args[2] != 0 {
                    vec![Region { ipa: args[2], bytes: retrace_box::encode_oldact(old).to_vec() }]
                } else { vec![] };
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: writes.clone(), thread })
                    .map_err(|e| format!("append sigaction: {e}"))?; count += 1;
                b.apply_and_return(0, false, &writes);
            }
            Stop::Syscall { num, args }
                if num == retrace_arch::SYS_SIGPROCMASK || num == retrace_arch::SYS_PTHREAD_SIGMASK => {
                // (how, set*, oldset*). A NULL `set` is a pure query — read the mask, change nothing.
                // M16: the mask belongs to the CALLING thread, which is `thread` here.
                let old = if args[1] != 0 {
                    let set = u32::from_le_bytes(b.read_guest(args[1], 4).try_into().unwrap());
                    b.threads_mut().set_mask_of(thread as usize, args[0], set)
                } else {
                    b.threads().mask_of(thread as usize)
                };
                let writes = if args[2] != 0 {
                    vec![Region { ipa: args[2], bytes: old.to_le_bytes().to_vec() }]
                } else { vec![] };
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: writes.clone(), thread })
                    .map_err(|e| format!("append sigprocmask: {e}"))?; count += 1;
                b.apply_and_return(0, false, &writes);
                // M16 Task 9: THE ANCHOR, and the design's load-bearing choice. A signal raised
                // while this thread's mask blocked it is materialised HERE, at the unmask landmark
                // — never at the scheduler's switch point, which lives inside `Box_::run()`, below
                // the trace, where a `SignalDelivery` could not be emitted at all. That is exactly
                // the argument M15 used to DELETE `Event::Sched` rather than reserve it, and it is
                // why the arm appends TWO landmarks (this Syscall, then the delivery) instead of
                // teaching the scheduler to write events.
                //
                // The limit this accepts: a signal left pending on a thread that never touches its
                // mask again is delivered NEVER, where a real kernel would deliver it at the next
                // opportunity.
                //
                // The target is the CALLING thread — the one holding the vCPU — so
                // `deliver_signal_to`'s Runnable guard is satisfied by construction. Worth stating
                // because the OTHER materialisation shape someone will reach for (delivering a
                // PEER's pending signal when its mask is changed for it) has no such invariant and
                // would hit that guard's panic.
                if let Some((psig, handler)) = take_pending_delivery(&mut b, thread as usize) {
                    // The frame must record that this sigprocmask SUCCEEDED, and `apply_and_return`
                    // alone cannot say so: the frame's PSTATE comes from SPSR_EL1, which
                    // `set_x0_err_and_return` never writes (it writes reg::CPSR). Same call, same
                    // reason, as the caught-raise arm below — omit it and the handler returns
                    // through `sigreturn` into a stale carry, where libc's syscall stub reads the
                    // unmask as having failed.
                    b.complete_syscall_before_delivery(0, false);
                    let (dwrites, resume_pc) =
                        b.deliver_signal_to(thread as usize, psig, retrace_arch::SI_USER, 0, 0, 0);
                    // `thread` is both caller and receiver here, so the tag is unambiguous — but it
                    // is the RECEIVER that `Event::SignalDelivery.thread` promises, which is what
                    // `mirror_delivery` compares its recomputed target against.
                    w.append(&Event::SignalDelivery { sig: psig, si_code: retrace_arch::SI_USER,
                                                      si_addr: 0, handler, resume_pc,
                                                      writes: dwrites, thread })
                        .map_err(|e| format!("append pending delivery: {e}"))?; count += 1;
                }
            }
            Stop::Syscall { num, args } if num == retrace_arch::SYS_SIGPENDING => {
                // M16 Task 9: the real pending set. This used to write a constant zero, justified
                // as "Always empty, and TRUE by construction: raising a blocked signal asserts
                // below, so no signal can ever be pending. These two decisions stand or fall
                // together." Task 7 removed that assert and gave the box a per-thread pending set,
                // so the second decision fell — and the comment, having named its own expiry
                // condition, is the reason this one did not quietly outlive it.
                //
                // The CALLING thread's set, like the mask. POSIX's sigpending is the union of the
                // thread's pending signals and the PROCESS's; retrace models no process-wide
                // pending set (nothing pends one — `kill` targets the caller), so the union is the
                // per-thread set and this is exact, not an approximation.
                let writes = if args[0] != 0 {
                    let pending = b.threads().pending_of(thread as usize);
                    vec![Region { ipa: args[0], bytes: pending.to_le_bytes().to_vec() }]
                } else { vec![] };
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: writes.clone(), thread })
                    .map_err(|e| format!("append sigpending: {e}"))?; count += 1;
                b.apply_and_return(0, false, &writes);
            }
            Stop::Syscall { num, args } if num == retrace_arch::SYS_SIGALTSTACK => {
                // Fast-follow: decode/encode moved to `retrace_box::decode_stack`/`encode_oldstack`
                // (one shared pair, like `decode_act`/`encode_oldact` for `sigaction`) — no
                // behaviour change, this arm wrote the identical bytes by hand before.
                let new = if args[0] != 0 {
                    Some(retrace_box::decode_stack(&b.read_guest(args[0], 24)))
                } else { None };
                // M16: the alternate stack belongs to the CALLING thread.
                let old = match new {
                    Some(ss) => b.threads_mut().set_altstack_of(thread as usize, Some(ss)),
                    None => b.threads().altstack_of(thread as usize),
                };
                let writes = if args[1] != 0 {
                    vec![Region {
                        ipa: args[1],
                        bytes: retrace_box::encode_oldstack(old.unwrap_or((0, 0, 0))).to_vec(),
                    }]
                } else { vec![] };
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: writes.clone(), thread })
                    .map_err(|e| format!("append sigaltstack: {e}"))?; count += 1;
                b.apply_and_return(0, false, &writes);
            }

            // The raise path. `kill(pid, sig)` and `__pthread_kill(port, sig)` differ only in how
            // the target is validated; the disposition decision below is shared.
            Stop::Syscall { num, args }
                if num == retrace_arch::SYS_KILL || num == retrace_arch::SYS_PTHREAD_KILL => {
                if num == retrace_arch::SYS_KILL {
                    // A SAFETY boundary, not a fidelity one: forwarding this would signal a REAL
                    // host process. getpid is not intercepted, so the guest's pid IS retrace's --
                    // measured, not assumed (M11 Task 1 Step 0 answer 2).
                    let self_pid = std::process::id() as u64;
                    assert_eq!(args[0], self_pid,
                        "kill to a pid other than the guest's own ({} != {self_pid}) is not \
                         modelled: the guest has no children and no other process it may signal, \
                         and forwarding would signal a REAL host process. Implement a guest pid \
                         namespace before a guest needs this.", args[0]);
                }
                // M16: __pthread_kill's thread-port operand IS validated now, by `thread_of_port`,
                // which reads each live thread's own kport out of its own pthread struct — main
                // included, with no special case. It is fail-loud (panics rather than defaulting
                // to the current thread) because defaulting to the current thread is the exact
                // latent bug M16 exists to close: it would silently run the target's handler on
                // whoever happened to call __pthread_kill. Before this task the operand went
                // unchecked because 328 fired in no gate guest (measured: zero across
                // hello_dyn/hello_rust/jq) and the guest had exactly one thread on one vCPU, so any
                // port it could name was that thread; SIGTHREAD (Task 5) is the first guest that
                // exercises this. The path is not confined to that fixture, either: libc's abort()
                // issues __pthread_kill(self_kport, SIGABRT), so thread_of_port now runs on the
                // abort path of EVERY guest — measured with RETRACE_TRACE=1 on `panicky`:
                // `[trap] num=328 (0x148) pc=0x1804b65e8 args=[0x103,0x6,0x0,0x0,0x0,0x1]`, where
                // 0x103 is main's own kport. That is why panic_e2e now covers the target==caller
                // resolution case on every run, and it means whoever next tunes thread_of_port's
                // matching rules has a blast radius of "every aborting guest", not just SIGTHREAD.
                //
                // M16: __pthread_kill names a TARGET THREAD; kill names the process. A
                // process-directed signal may go to any thread with it unblocked, and retrace picks
                // the caller — which is what every pre-M16 gate already assumes.
                let target = if num == retrace_arch::SYS_PTHREAD_KILL {
                    b.thread_of_port(args[0] as u32)
                } else {
                    b.threads().current()
                };
                let sig = args[1];
                let act = b.sigtable().action(sig);

                // M17: the pend condition is now `should_pend_for` — mask OR not-Runnable — and it
                // is a `Box_` method precisely so replay's mirror consults the identical predicate
                // rather than a second copy of the same `||`.
                if b.should_pend_for(target, sig) {
                    // M16 replaces M11's assert: M11 modelled no pending set (measured: no gate
                    // guest raised a blocked signal; abort() unblocks SIGABRT before raising), so
                    // it could only refuse this case, not handle it. The signal goes PENDING on the
                    // TARGET thread and is materialised at the next unmask — a syscall landmark,
                    // which is what keeps delivery visible to both dispatch loops. Note
                    // sigpending's always-empty answer stops being true now that a pending set
                    // exists.
                    w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![], thread })
                        .map_err(|e| format!("append pended raise: {e}"))?; count += 1;
                    b.threads_mut().pend(target, sig);
                    b.set_x0_err_and_return(0, false);
                } else {
                    match act.disp {
                        // M12: the self-raise counterpart of the fault path. The ordinary Syscall
                        // event is appended FIRST, so the divergence oracle still checks (num, args)
                        // and the kill safety boundary above still runs; the delivery is a second
                        // landmark. esr/far are 0: no hardware fault happened, and inventing a
                        // syndrome would be the lie M11 refused when it kept Event::Signal out of
                        // Event::Crash.
                        retrace_box::Disposition::Handler(handler) => {
                            w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![], thread })
                                .map_err(|e| format!("append caught raise: {e}"))?; count += 1;
                            // The raise SUCCEEDS, and the CALLER's frame must say so — regardless of
                            // whether the caller is also the receiver. This delivery happens at a
                            // syscall boundary, so the context the kernel snapshots on the CALLER is
                            // the POST-return one: x0 = 0 and PSTATE.C clear, not the pid the guest
                            // passed and whatever flags it happened to carry. Measured in
                            // spikes/sigraisex0.c; see complete_syscall_before_delivery, which exists
                            // because the frame's PSTATE comes from SPSR_EL1 rather than from the
                            // reg::CPSR that set_x0_err_and_return writes. It always operates on the
                            // live vCPU (the caller), never on `target`'s saved context, so it needs
                            // no split for a cross-thread delivery.
                            b.complete_syscall_before_delivery(0, false);
                            let (writes, resume_pc) =
                                b.deliver_signal_to(target, sig, retrace_arch::SI_USER, 0, 0, 0);
                            // M16: `thread` here is the TARGET, not the caller — the receiving
                            // thread the trace-format doc comment on Event::SignalDelivery promises.
                            w.append(&Event::SignalDelivery { sig, si_code: retrace_arch::SI_USER,
                                                              si_addr: 0, handler, resume_pc, writes,
                                                              thread: target as u32 })
                                .map_err(|e| format!("append signal delivery: {e}"))?; count += 1;
                        }
                        retrace_box::Disposition::Ign => {
                            w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![], thread })
                                .map_err(|e| format!("append ignored raise: {e}"))?; count += 1;
                            b.set_x0_err_and_return(0, false);
                        }
                        retrace_box::Disposition::Dfl => match retrace_arch::default_action(sig) {
                            retrace_arch::DefaultAction::Ignore => {
                                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![], thread })
                                    .map_err(|e| format!("append default-ignored raise: {e}"))?; count += 1;
                                b.set_x0_err_and_return(0, false);
                            }
                            // TERMINAL. Same shape as the Exit and Crash arms above: the event, then
                            // the final full-memory snapshot, then break. `thread` is the CALLER
                            // here, not `target` (in scope above, for __pthread_kill): Event::Signal's
                            // format doc (retrace-trace) permanently defines `thread` as the RAISING
                            // thread, so that it names the same event as `pc`, the raise site.
                            retrace_arch::DefaultAction::Terminate => {
                                let pc = b.position();
                                let final_snap = b.snapshot();
                                w.append(&Event::Signal { sig, pc, thread })
                                    .map_err(|e| format!("append signal: {e}"))?; count += 1;
                                w.append(&final_snap)
                                    .map_err(|e| format!("append final snapshot: {e}"))?; count += 1;
                                outcome = Outcome::Signal { sig };
                                break;
                            }
                        },
                    }
                }
            }

            // Unmodelled, and loud about it. Each of these would otherwise reach forward_and_diff
            // and execute against RETRACE's process — 520/521 are live recorder-killing hazards
            // today, which is why they are asserted even though modelling them is out of scope.
            // M12: sigreturn(184) — the handler returning. Serviced, never forwarded. Its register
            // restore is recomputed identically on both sides by Box_::sigreturn_restore, so the
            // event carries no writes; (num, args) is still oracle-checked.
            //
            // Deliberately NOT followed by set_x0_err_and_return: sigreturn returns no value, and
            // that call would overwrite the x0 and pc just restored from the frame.
            Stop::Syscall { num, args } if num == retrace_arch::SYS_SIGRETURN => {
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![], thread })
                    .map_err(|e| format!("append sigreturn: {e}"))?; count += 1;
                b.sigreturn_restore(args[0], args[2]);
            }
            Stop::Syscall { num, .. }
                if num == retrace_arch::SYS_SIGSUSPEND || num == retrace_arch::SYS_SIGWAIT => panic!(
                "syscall {num} (sigsuspend/__sigwait) blocks until a signal arrives, and the guest \
                 has ONE thread on ONE vCPU with nothing to wake it — servicing it would deadlock. \
                 Implement threads before a guest needs this."),
            Stop::Syscall { num, .. }
                if num == retrace_arch::SYS_TERMINATE_WITH_PAYLOAD
                || num == retrace_arch::SYS_ABORT_WITH_PAYLOAD => panic!(
                "syscall {num} (terminate/abort_with_payload) is a terminal path that bypasses \
                 signal disposition entirely and is not modelled (measured: unexercised by any gate \
                 guest). It is asserted rather than forwarded because forwarding it kills the \
                 RECORDER. Model it as a second terminal event shape if a guest needs it."),

            // M18 t5: bsdthread_register is EMULATED, never forwarded (see
            // Box_::guest_bsdthread_register for both reasons — the guest's registration is the
            // guest's, AND forwarding hands guest addresses to the host kernel as retrace's own
            // thread-start functions).
            //
            // `writes` is empty and that is deliberate: the call writes no guest memory, and its
            // return is a compile-time constant that the replay mirror recomputes identically.
            // The byte-compare there IS the oracle (symmetry rule 1).
            Stop::Syscall { num, args } if num == retrace_arch::SYS_BSDTHREAD_REGISTER => {
                let rc = b.guest_bsdthread_register(args);
                w.append(&Event::Syscall { num, args, ret: rc, err: false, writes: vec![], thread })
                    .map_err(|e| format!("append bsdthread_register: {e}"))?; count += 1;
                b.set_x0_err_and_return(rc, false);
            }
            // M18 Stage 2a: workq_open is EMULATED, never forwarded (see Box_::guest_workq_open).
            // Forwarding brings up a real kernel workqueue for retrace's own process, which with
            // the REQTHREADS below makes the host create a worker thread INSIDE the recorder —
            // measured in a crash report, M18 Task 6.
            //
            // `writes` is empty and that is deliberate: the call writes no guest memory, and its
            // return is a constant the replay mirror recomputes identically. The byte-compare
            // there IS the oracle (symmetry rule 1).
            Stop::Syscall { num, args } if num == retrace_arch::SYS_WORKQ_OPEN => {
                let rc = b.guest_workq_open(args);
                w.append(&Event::Syscall { num, args, ret: rc, err: false, writes: vec![], thread })
                    .map_err(|e| format!("append workq_open: {e}"))?; count += 1;
                b.set_x0_err_and_return(rc, false);
            }
            // M18 Stage 2a: workq_kernreturn is EMULATED, never forwarded — same reason. Note this
            // arm may PANIC by design: `guest_workq_kernreturn` refuses every operation word no run
            // has measured BY VALUE, so the recorder stops here naming the opcode rather than
            // handing the syscall to the host kernel. (Stage 2b t2 removed the REQTHREADS panic
            // this comment used to name; that opcode now builds the worker.)
            Stop::Syscall { num, args } if num == retrace_arch::SYS_WORKQ_KERNRETURN => {
                let rc = b.guest_workq_kernreturn(args);
                w.append(&Event::Syscall { num, args, ret: rc, err: false, writes: vec![], thread })
                    .map_err(|e| format!("append workq_kernreturn: {e}"))?; count += 1;
                b.set_x0_err_and_return(rc, false);
            }
            // M14 Task 7: bsdthread_create is EMULATED, never forwarded — the host would create a
            // real thread inside retrace's own process at a guest address (see
            // Box_::guest_bsdthread_create).
            //
            // M14 t11: it DOES now write guest memory — the child's kport into the guest's pthread
            // struct at +0xf8, the write `pthread_join` is unusable without — and the event STILL
            // carries no writes. That is deliberate, not an oversight. The value is
            // `GUEST_THREAD_PORT_BASE | tid`, a pure function of the guest's own syscall sequence,
            // and the replay arm below calls the same `guest_bsdthread_create` with identical args:
            // both sides therefore recompute the identical byte at the identical address (symmetry
            // rule 1), so recording it would be recording a constant. The exit-time full-memory
            // comparison still covers it, which is what keeps this honest rather than merely quiet.
            Stop::Syscall { num, args } if num == retrace_arch::SYS_BSDTHREAD_CREATE => {
                let rc = b.guest_bsdthread_create(args);
                w.append(&Event::Syscall { num, args, ret: rc, err: false, writes: vec![], thread })
                    .map_err(|e| format!("append bsdthread_create: {e}"))?; count += 1;
                b.set_x0_err_and_return(rc, false);
            }
            // M14 Task 8: a guest thread's exit (see Box_::guest_bsdthread_terminate). Never
            // forwarded — same reasoning as bsdthread_create above. `rc` is bound and recorded as
            // `ret` (fix round 1, I-2) even though it is always 0 today, exactly like
            // `bsdthread_create`'s neighbour arm below — the byte-compare on replay IS the
            // divergence oracle, and a hardcoded `ret: 0` would leave it permanently vacuous.
            Stop::Syscall { num, args } if num == retrace_arch::SYS_BSDTHREAD_TERMINATE => {
                let rc = b.guest_bsdthread_terminate(args);
                w.append(&Event::Syscall { num, args, ret: rc, err: false, writes: vec![], thread })
                    .map_err(|e| format!("append bsdthread_terminate: {e}"))?; count += 1;
                // Task 8 fix round 1, M-4 panicked here because "no scheduler exists yet to switch
                // the vCPU away from it". M14 TASK 9 IS THAT SCHEDULER, so the panic is now
                // replaced by its intended behaviour — but note what is still ABSENT:
                // `set_x0_err_and_return` is deliberately NOT called. `guest_bsdthread_terminate`'s
                // doc is explicit ("Does not return"), and the real kernel never returns to a
                // thread that just terminated. The thread is now `Exited`, so the next entry to
                // `Box_::run()` sees `needs_reschedule()` and switches to a runnable thread instead
                // of resuming this one. The vCPU state left parked here is the exiting thread's and
                // is never resumed — `pick_next` skips `Exited` threads.
            }
            // M14 Task 8: the join blocking primitive (see Box_::guest_ulock_wait). Never
            // forwarded: syscall 515 is a real futex-shaped wait, and forwarding it would block
            // retrace's OWN process on a guest address — the same hazard class Task 1 measured
            // as fatal for bsdthread_create. Writes nothing to guest memory itself (the box only
            // READS the compared word), so the event carries no writes. An unmapped guest address
            // answers EFAULT — a legal guest behaviour — instead of panicking the box (fix round 1,
            // M-3).
            //
            // `err` is hardcoded `false`, and that is the FIX for round 1's I-2, not an oversight.
            // Both operation words `guest_ulock_wait` admits carry `ULF_NO_ERRNO` (bit 24), under
            // which the kernel reports failure as `-errno` in x0 with `PSTATE.C` CLEAR — measured
            // on this host, see that function's doc. The earlier `Result<u64, u64>` shape mapped
            // `Err(14)` to `set_x0_err_and_return(14, true)`, which set carry and sent the guest
            // into libsyscall's `cerror`; `__pthread_join`'s own `cmn w0, #0x4` then missed and it
            // re-waited forever. `guest_ulock_wait` now returns the raw signed x0 word, so there is
            // no `Err` variant left for this arm to turn into `err: true`.
            Stop::Syscall { num, args } if num == retrace_arch::SYS_ULOCK_WAIT => {
                let rc = b.guest_ulock_wait(args);
                w.append(&Event::Syscall { num, args, ret: rc, err: false, writes: vec![], thread })
                    .map_err(|e| format!("append ulock_wait: {e}"))?; count += 1;
                b.set_x0_err_and_return(rc, false);
            }
            // M14 Task 9: the wake half of the pair 515 services (see Box_::guest_ulock_wake).
            // Task 8 left a fail-loud placeholder here because the exit-side wake address was
            // unmeasured; Task 9 measured it (task-9-measurements.md) and this is the real handler.
            // Still never forwarded, for Task 8's reason unchanged: forwarding would issue a real
            // __ulock_wake from retrace's OWN process against a guest address. Writes nothing to
            // guest memory (it only moves thread-table state), so the event carries no writes.
            Stop::Syscall { num, args } if num == retrace_arch::SYS_ULOCK_WAKE => {
                let (rc, woken) = b.guest_ulock_wake(args);
                w.append(&Event::Syscall { num, args, ret: rc, err: false, writes: vec![], thread })
                    .map_err(|e| format!("append ulock_wake: {e}"))?; count += 1;
                b.set_x0_err_and_return(rc, false);

                // M17: THE ANCHOR. A signal pended on a thread because it could not run is
                // materialised HERE, at the wake landmark that made it runnable — the same argument
                // the mask arm above makes for the unmask landmark, and for the same reason: the
                // scheduler's switch point lives inside `Box_::run()`, below the trace, where a
                // `SignalDelivery` could not be emitted at all.
                //
                // NO `complete_syscall_before_delivery` here, and that is the difference from the
                // other two materialisation sites. That call fixes SPSR_EL1 on the LIVE vCPU, which
                // is the CALLER — and here the caller is the WAKER, not the receiver. Calling it
                // here would corrupt the waker's PSTATE instead of the receiver's.
                //
                // But the receiver's OWN PSTATE still needs the equivalent fix, applied to its
                // SAVED context instead of the live vCPU: Task 1 measured its saved x0 as already 0
                // (`__ulock_wait`'s completed return value), and Task 4b measured its saved SPSR as
                // correspondingly UNPATCHED rather than "also completed" — C **set**
                // (`0x60000000`, `crates/retrace/tests/blockedctx.rs`), disagreeing with the
                // completed x0 sitting next to it. `complete_saved_syscall_before_delivery` is that
                // fix, targeted at `wtid`'s saved context rather than the live vCPU. Both halves are
                // load-bearing: the live version stays absent because the live vCPU is the waker;
                // the saved version is called because the receiver's own PSTATE needs the same
                // completed-syscall correction the kernel applies, just against different state.
                let deliver_to: Vec<usize> = woken.iter().copied()
                    .filter(|&t| b.threads().peek_deliverable(t).is_some())
                    .collect();
                assert!(deliver_to.len() <= 1,
                    "one wake made {} threads deliverable at once ({deliver_to:?}); that needs N+1 \
                     landmarks at a single stop and a decision about their order, which M17 does \
                     not model. No fixture produces it — measure the guest before modelling it.",
                    deliver_to.len());
                if let Some(&wtid) = deliver_to.first() {
                    // M17 fix round 5: the sibling of the `deliver_to.len() <= 1` bound above, on
                    // the other axis, and it is there for the same reason. That one bounds how many
                    // THREADS one wake can make deliverable; this one bounds how many SIGNALS one
                    // woken thread can have. `take_pending_delivery` consumes exactly ONE bit, and
                    // the woken thread is Runnable by now — so `assert_no_stranded_signals`, which
                    // `continue`s on anything that is not `Blocked(_)`, would never see the
                    // leftover. A signal the guest was owed would simply vanish while record and
                    // replay agreed with each other: the one failure a determinism oracle cannot
                    // see, and the exact class that guard exists to catch.
                    //
                    // Deliberately OUTSIDE the `Some`/`None` match on the result, because the
                    // `None` case swallows just as silently: a disposition of `Ign` makes
                    // `take_pending_delivery` return `None` AFTER `take_deliverable` has already
                    // cleared the bit.
                    //
                    // Reachable in principle — two `pthread_kill`s at one blocked target — but no
                    // fixture in the tree produces it, so queueing at a wake is left unmodelled and
                    // LOUD rather than guessed.
                    let taken = take_pending_delivery(&mut b, wtid);
                    let second = b.threads().peek_deliverable(wtid);
                    assert!(second.is_none(),
                        "woken thread {wtid} still has deliverable signal {second:?} after this \
                         wake materialised one: M17 materialises at most ONE signal per wake, so \
                         the second is silently swallowed — the thread is Runnable now, and \
                         assert_no_stranded_signals scans Blocked threads only. Queueing at a wake \
                         is deliberately unmodelled because no guest in the tree measures it; \
                         measure one before modelling it.");
                    if let Some((psig, handler)) = taken {
                        b.complete_saved_syscall_before_delivery(wtid, false); // always false: the receiver's own wait succeeded (its saved x0 is 0)
                        let (dwrites, resume_pc) =
                            b.deliver_signal_to(wtid, psig, retrace_arch::SI_USER, 0, 0, 0);
                        // The tag is the RECEIVER — the woken thread — not `thread`, which is the
                        // waker whose syscall this landmark belongs to. They always differ here, so
                        // this is the sharpest case of the rule `Event::SignalDelivery.thread`
                        // states, and `mirror_delivery`'s inline check is what enforces it.
                        w.append(&Event::SignalDelivery { sig: psig, si_code: retrace_arch::SI_USER,
                                                          si_addr: 0, handler, resume_pc,
                                                          writes: dwrites, thread: wtid as u32 })
                            .map_err(|e| format!("append woken delivery: {e}"))?; count += 1;
                    }
                }
            }

            // Every other syscall goes through the general memory-diff engine (forwarded once).
            Stop::Syscall { num, args } => {
                // M11 correctness invariant: no signal syscall may reach forward_and_diff, which
                // issues a raw svc in retrace's own process. If one does, an arm above is missing.
                assert!(!retrace_arch::is_signal_syscall(num),
                    "signal syscall {num} reached the generic forward arm — it must be serviced or \
                     asserted above (M11 correctness invariant)");
                // M10: dup2 names its own target descriptor rather than taking the lowest free one,
                // so the table would have to honour an arbitrary slot. No guest in the gate calls it
                // (measured: zero in the jq run), so fail loudly rather than model it wrong — a
                // silently mis-modelled dup2 aliases the wrong file.
                assert!(num != retrace_arch::SYS_DUP2,
                    "dup2 is not modelled by the M10 fd table (unexercised by any gate guest); \
                     implement target-slot allocation before a guest uses it");
                // M18 Stage 2a: the workqueue pair must never reach here. Forwarding them is not
                // merely wrong but whole-process fatal for the RECORDER: the host kernel brings up
                // a workqueue for retrace's own process and then creates a real worker thread in
                // it, entering `start_wqthread` -> `_pthread_wqthread`, which jumps through a
                // dispatch function pointer that is NULL in this process and dies at address 0.
                // Measured in a crash report, M18 Task 6 (stage2-measurements.md §3). The arms
                // above service both; this assert is what stops a later edit from removing one and
                // silently restoring the hazard — the same shape as the dup2 guard above.
                assert!(num != retrace_arch::SYS_WORKQ_OPEN && num != retrace_arch::SYS_WORKQ_KERNRETURN,
                    "workq syscall {num} reached the generic forward arm — it must be emulated \
                     above (M18 Stage 2a). Forwarding it creates a real host worker thread inside \
                     the recorder and takes a SIGSEGV at address 0.");
                // M10: `ret` is already a GUEST descriptor when this syscall produced one, and a
                // successful close has already retired its slot — forward_and_diff owns both halves
                // of the fd contract so no caller has to remember the second one.
                let (ret, err, writes) = b.forward_and_diff(num, args);
                w.append(&Event::Syscall { num, args, ret, err, writes, thread }).map_err(|e| format!("append syscall: {e}"))?; count += 1;
                b.set_x0_err_and_return(ret, err);
            }
            // A cache-window stage-2 fault: stage/fixup/re-sign/map the page (page_in_cache) and
            // re-run. Regenerated deterministically here on record AND replay, so nothing about the
            // cache page goes into the trace. A non-cache fault (page_in_cache returns false) is a
            // real bring-up failure — decode the ESR class + faulting IPA so it names itself.
            Stop::Other { esr } => {
                if b.page_in_cache(b.fault_ipa()) { continue; }
                if b.commit_reserved_page(b.fault_ipa()) {
                    // M13 t2: the retained PROT_NONE-reservation deviation is quantified rather
                    // than hand-waved (see the M13 spec). Record-only, like every other
                    // RETRACE_TRACE line.
                    if trace_log { eprintln!("[commit] ipa={:#x}", b.fault_ipa()); }
                    continue;
                }
                if trace_log { eprintln!("[regs]\n{}\n[bt]\n{}", b.dbg_regs(), b.dbg_backtrace(24)); }
                return Err(b.describe_stop(esr));
            }
            // Stop::Step is only produced by Box_::step(); record_box drives run(), never step().
            Stop::Step => unreachable!("record_box drives run(), which never single-steps"),
        }
    }
    Ok(RecordSummary { stdout, outcome, events: count })
}

#[derive(Debug)]
pub struct ReplayReport { pub stdout: Vec<u8>, pub outcome: Outcome }
#[derive(Debug)]
pub struct Divergence { pub landmark: usize, pub pc: u64, pub detail: String }

/// A resumable replay engine. `open` restores the guest from a trace's leading snapshot; `advance`
/// consumes exactly one recorded landmark at a time — verifying each trap against the recording
/// (the divergence oracle) and applying the recorded kernel writes, NEVER executing a syscall.
/// `replay()` drives it to exit; the M3 reverse-debugger drives it to arbitrary landmarks. The
/// dispatch is identical whether it runs to the end or is stepped, so both share one engine.
pub struct ReplaySession {
    b: Box_,
    events: Vec<Event>,
    idx: usize,
    stdout: Vec<u8>,
    // Mirror of record's task-port learning (see record_box): learned from the RECORDED
    // task_self_trap (−28) result so routing decides identically on replay.
    guest_task_port: Option<u64>,
    // open_checked dropped a torn/corrupt tail; surfaced only in the "expected recorded syscall"
    // diagnostic below. (Not one of the four state locals — a diagnostic carried alongside them.)
    truncated: bool,
}

// Manual Debug (the box is not Debug and dumping full guest memory would be useless): show only the
// position bookkeeping. Needed so `Result<ReplaySession, _>::unwrap_err` can format an unexpected Ok.
impl std::fmt::Debug for ReplaySession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReplaySession")
            .field("idx", &self.idx)
            .field("events", &self.events.len())
            .field("truncated", &self.truncated)
            .finish_non_exhaustive()
    }
}

/// The outcome of one `advance`: exactly one trace event was consumed (`Event`); the guest reached
/// a terminal outcome — a normal `exit` OR a crash — carrying the `ReplayReport` whose `outcome`
/// field discriminates which (`Exited`, run done); or a hardware breakpoint fired mid-window
/// (`Break`, M3 debugger only — carries nothing: the caller reads `landmark()`/`pc()`). `Break` is
/// unreachable under the plain `replay()` oracle, which never arms breakpoints.
/// M15 Task 5: both watch variants name the thread whose store triggered them. `Watch` was a unit
/// variant until now; a watch hit that cannot say WHO wrote is half an answer once a guest has more
/// than one thread. The two are populated at two independent sites — `Watch` in the `Stop::Other`
/// watchpoint arm, `WatchSyscall` in `finish_event` — both reading the live thread table, never a
/// cached copy, so the id is the scheduler's own answer at the instant of the hit.
pub enum Advance {
    Event,
    Exited(ReplayReport),
    Break,
    Watch { thread: u32 },
    WatchSyscall { watched: u64, thread: u32 },
}

impl ReplaySession {
    pub fn open(trace_path: &Path) -> Result<Self, String> {
        // open_checked keeps every whole, CRC-valid record and drops a torn/corrupt tail; a
        // missing/unreadable file, an empty/torn trace, or a lost leading Snapshot each become
        // a named error (the caller turns it into a landmark-0 Divergence, exit 3) rather than a panic.
        let (events, truncated) = retrace_trace::Reader::open_checked(trace_path)
            .map_err(|e| format!("cannot open trace: {e}"))?;
        if events.is_empty() {
            return Err("empty/torn trace: no readable records".into());
        }
        let (regs, mem) = match events.first() {
            Some(Event::Snapshot { regs, mem }) => (regs.clone(), mem.clone()),
            _ => return Err("trace missing leading Snapshot".into()),
        };
        // Rebuild the guest from the snapshot's exact regions (includes stack + trampoline);
        // restore maps only those regions and re-establishes fixed sysregs + captured registers.
        let b = Box_::restore(&mem, &regs);
        // events[0] is the initial snapshot; the first landmark to consume is events[1].
        Ok(ReplaySession { b, events, idx: 1, stdout: Vec::new(), guest_task_port: None, truncated })
    }

    /// M12: recompute a signal delivery and byte-compare the frame against the recorded landmark.
    /// That comparison **is** the divergence check (symmetry rule 1) — an asymmetry between
    /// record's delivery arms and these mirrors surfaces here as a named divergence instead of as
    /// silent corruption of the guest's stack.
    ///
    /// Comparing after `deliver_signal_to` has written is too late to prevent the write but not too
    /// late to detect it: the session has not advanced, so a mismatch aborts replay at the right
    /// landmark with both byte strings in hand.
    ///
    /// M16 Task 8: `tid` is the RECEIVING thread, and every call site recomputes its own — the
    /// raise mirror through `thread_of_port`, the fault mirror as the current thread. One shared
    /// helper rather than a per-arm copy precisely because it owns the byte-compare that IS the
    /// divergence check: two copies could drift while both stayed green, which is the defect M13
    /// Task 8 shipped. The recorded `thread` tag is compared against `tid` here, after the frame
    /// compare, so a genuine frame divergence still reports as itself.
    // The parameter list is `deliver_signal_to`'s, verbatim, plus the `pc` every Divergence needs
    // to name its landmark. Symmetry rule 1 is "the same method with the same arguments", so
    // bundling them into a struct here would put a translation step on exactly the path whose
    // whole job is to have none.
    #[allow(clippy::too_many_arguments)]
    fn mirror_delivery(&mut self, tid: usize, sig: u64, si_code: u64, si_addr: u64, esr: u64,
                       far: u64, pc: u64)
        -> Result<Advance, Divergence> {
        let (rsig, rwrites, rthread) = match self.events.get(self.idx) {
            Some(Event::SignalDelivery { sig: rsig, writes, thread, .. }) =>
                (*rsig, writes.clone(), *thread),
            other => return Err(Divergence { landmark: self.idx, pc, detail: format!(
                "expected recorded SignalDelivery, got {other:?} (live: sig={sig} far={far:#x})") }),
        };
        // Fast-follow: refuse an undeliverable target with a named `Divergence` rather than let
        // `deliver_signal_to`'s own assertion abort the process. Same condition and the same single
        // definition in `Box_`; only the reaction differs. On this side the condition means the
        // live schedule put the target in a state the recorded schedule did not — a divergence,
        // which is precisely what this function exists to report.
        //
        // Unreachable by trace mutation for the same structural reason as the port-resolution arm
        // in `advance`'s raise mirror (see its comment): the target's state is recomputed from the
        // live schedule, which no recorded field steers.
        //
        // M17 Task 7 un-parked `sigblocked_e2e`, so this FUNCTION is now genuinely called — via
        // replay's `SYS_ULOCK_WAKE` arm, on the woken-delivery path. But the Err branch above it is
        // still not exercised, for a different reason than "the gate can't run": it now can, and it
        // is not this arm that it reaches. `deliver_to` (both dispatch loops') is filtered from
        // `woken`, which `guest_ulock_wake`'s own `unblock_waiters_on` has already transitioned to
        // Runnable as part of the SAME call that produced `woken` — so every `wtid` this function is
        // called with is already Runnable, on record and replay alike, before `check_deliverable`
        // ever runs. Firing this arm needs a genuine live/recorded schedule mismatch, which this
        // gate's own correct schedule cannot produce by construction — same as the port-resolution
        // arm it echoes. Ruling out that mismatch is the gate's job, not manufacturing one.
        if let Err(d) = self.b.check_deliverable(tid) {
            return Err(Divergence { landmark: self.idx, pc, detail: format!(
                "signal delivery target not deliverable on replay: {d}") });
        }
        let (mine, _resume_pc) = self.b.deliver_signal_to(tid, sig, si_code, si_addr, esr, far);
        if sig != rsig || mine != rwrites {
            // Name the first differing byte: a frame mismatch is usually one field, and the
            // offset identifies which one far faster than two 976-byte dumps do.
            let where_ = mine.first().zip(rwrites.first()).and_then(|(m, r)| {
                m.bytes.iter().zip(r.bytes.iter()).position(|(a, b)| a != b)
                    .map(|i| format!("; first differing byte at frame+{i}: recomputed {:#04x} != \
                                      recorded {:#04x}", m.bytes[i], r.bytes[i]))
            }).unwrap_or_default();
            return Err(Divergence { landmark: self.idx, pc, detail: format!(
                "signal frame mismatch: live sig={sig} recorded sig={rsig}; recomputed {} bytes at \
                 {:#x}, recorded {} bytes at {:#x}{where_}",
                mine.first().map_or(0, |r| r.bytes.len()), mine.first().map_or(0, |r| r.ipa),
                rwrites.first().map_or(0, |r| r.bytes.len()), rwrites.first().map_or(0, |r| r.ipa)) });
        }
        // M16 Task 8: and the RECEIVER must match too. Ordered AFTER the frame compare on purpose:
        // delivering to the wrong thread builds the frame on the wrong stack, so the frame compare
        // is the more specific report of the same fault and must not be masked by this one. Note
        // this tag is NOT the caller — for `pthread_kill(child, sig)` the recorded Syscall landmark
        // names the caller and this one names the target, and they differ.
        if rthread != tid as u32 {
            return Err(Divergence { landmark: self.idx, pc, detail: format!(
                "signal delivery thread mismatch: live {tid}, recorded {rthread} — the signal was \
                 delivered to a different thread than the recording did") });
        }
        self.finish_event()
    }

    /// M15 Task 4, fix round 1: the thread comparison shared by every point in `advance` that
    /// consumes a recorded landmark and returns before reaching the generic dispatch below (itself
    /// one of the call sites). There are SEVEN call sites now, not three — each added because a new
    /// mirror was found to return before ever reaching the generic dispatch, so it needed its own
    /// call:
    ///   - M15 Task 4's original three: the generic dispatch below (`Event::Syscall`), the
    ///     caught-raise mirror (`Disposition::Handler` under `SYS_KILL` / `SYS_PTHREAD_KILL`), and
    ///     the `SYS_SIGRETURN` mirror. A threaded guest that also takes a caught signal is the only
    ///     guest shape that ever reaches either of the latter two, so a hole in just those two paths
    ///     was invisible to every gate that doesn't combine both — which is exactly how the first
    ///     cut of this check missed them.
    ///   - M16 Task 8's fourth, whose landmark is not an `Event::Syscall` at all: the terminal-raise
    ///     mirror's `Event::Signal`, whose `thread` the trace format also defines as the RAISING
    ///     thread.
    ///   - M16 Task 9's fifth: the sigprocmask/pthread_sigmask mask mirror, hoisted into its own arm.
    ///   - M16 Task 11's sixth and seventh: the `Exit` and `Crash` mirrors.
    ///
    /// Each call site invokes this AFTER its own local comparison (num/args, sig/pc, exit code, or
    /// crash triple), for the same reason the check is ordered that way inline: a genuine mismatch
    /// should be reported as itself, not masked by the thread mismatch it caused.
    ///
    /// What this helper never checks is a `SignalDelivery`'s tag — that one names the RECEIVER,
    /// which for `pthread_kill(child, sig)` is a different thread from the caller, so it is compared
    /// against the recomputed target inside `mirror_delivery` instead.
    fn verify_thread(&self, rthread: u32, pc: u64) -> Result<(), Divergence> {
        if self.current_thread() != rthread {
            return Err(Divergence { landmark: self.idx, pc, detail: format!(
                "thread {} on replay, {} recorded — the schedule diverged. Two \
                 threads running the same code issue identical (num, args), which \
                 is exactly the case this check exists to catch",
                self.current_thread(), rthread) });
        }
        Ok(())
    }

    /// Finish consuming one trace event: bump idx and report it — as `WatchSyscall` if this event's
    /// applied writes overlapped an armed watch range (the event is consumed identically either
    /// way; only the report differs), else as plain `Event`.
    fn finish_event(&mut self) -> Result<Advance, Divergence> {
        self.idx += 1;
        if let Some((watched, _ipa)) = self.b.take_syscall_watch_hit() {
            return Ok(Advance::WatchSyscall { watched, thread: self.current_thread() });
        }
        Ok(Advance::Event)
    }

    /// Consume exactly ONE trace event (returning `Advance::Event`), or drive the guest to a
    /// terminal outcome — `exit` OR a crash — (returning `Advance::Exited`, carrying the
    /// `ReplayReport` whose `outcome` field discriminates which). Non-event stops — a cache-window
    /// page-in or a reservation commit — are handled internally and the guest re-run, so `advance`
    /// returns only on event consumption or a terminal outcome. Once it has returned
    /// `Advance::Exited` the run is complete; calling `advance` again is unspecified (the guest is
    /// past its final landmark) — callers must not.
    pub fn advance(&mut self) -> Result<Advance, Divergence> {
        loop {
            match self.b.run() {
                Stop::Syscall { num, args } => {
                    let pc = self.b.position();
                    if num == SYS_EXIT {
                        // Verify Exit, then the final-memory landmark.
                        match self.events.get(self.idx) {
                            Some(Event::Exit { code, thread: rthread }) => {
                                if args[0] != *code {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!("exit code mismatch: live {} != recorded {}", args[0], code) });
                                }
                                // M16 Task 11: the thread oracle (see `verify_thread`'s doc). Placed
                                // AFTER the exit-code compare above, not before, for the usual
                                // reason: a genuine code mismatch should be reported as itself, not
                                // masked by the thread mismatch it caused.
                                self.verify_thread(*rthread, pc)?;
                                match self.events.get(self.idx + 1) {
                                    Some(Event::Snapshot { mem: final_mem, .. }) => {
                                        if let Some(d) = self.b.diff_memory(final_mem) {
                                            return Err(Divergence { landmark: self.idx + 1, pc, detail: d });
                                        }
                                        return Ok(Advance::Exited(ReplayReport {
                                            stdout: std::mem::take(&mut self.stdout),
                                            outcome: Outcome::Exit { code: *code } }));
                                    }
                                    other => return Err(Divergence { landmark: self.idx + 1, pc,
                                        detail: format!("expected final memory Snapshot, got {other:?}") }),
                                }
                            }
                            other => return Err(Divergence { landmark: self.idx, pc,
                                detail: format!("expected recorded Exit, got {other:?}") }),
                        }
                    }
                    // M11 mirror of record's terminal raise. Structure copied from the Exit verify
                    // above: compare the event, then the final-memory landmark. A signal arrives as
                    // a Stop::Syscall, not a Stop::Fault, so this must sit BEFORE the generic
                    // recorded-Event::Syscall lookup — mirroring how record's raise arm precedes its
                    // generic arm. Placing it after yields "expected recorded syscall, got Signal",
                    // a confusing divergence that looks like a recording bug and is a dispatch bug.
                    //
                    // The disposition is recomputed from the REPLAY-side table, which the serviced
                    // mirrors below keep in step. That is what makes the sigign guest replay
                    // correctly: without the sigaction mirror this table would still read Dfl for
                    // SIGABRT and would wrongly terminate a guest that had ignored it.
                    if num == retrace_arch::SYS_KILL || num == retrace_arch::SYS_PTHREAD_KILL {
                        // M16 Task 8: resolve the TARGET by the same route record used — the same
                        // `Box_` methods with the same arguments (symmetry rule 1), which is what
                        // makes "both sides pick the same thread" true by construction. Recomputed,
                        // never read out of the recording: consuming the recorded `thread` would
                        // leave replay unable to disagree with record, i.e. would delete the
                        // divergence check while leaving a gate that still passed. `thread_of_port`
                        // is fail-loud on this side too, deliberately — a "fall back to the current
                        // thread" path here would silently resurrect the exact bug M16 closes.
                        let target = if num == retrace_arch::SYS_PTHREAD_KILL {
                            // Fast-follow: the FALLIBLE form on this side. A port that resolves to
                            // no live thread means retrace's model is wrong on the record side, but
                            // here it means the live schedule disagrees with the recorded one — and
                            // nothing else would name it, because this landmark's own
                            // `verify_thread` checks the CALLER, not the target. Aborting would end
                            // the process with a panic where every other replay-side failure
                            // reports a `Divergence` at its landmark. The scan itself is unchanged
                            // and shared with record, so symmetry rule 1 still holds.
                            //
                            // NOT reachable by trace mutation, and the reason is structural rather
                            // than a gap in the fixtures: this mirror resolves the LIVE `args[0]`,
                            // and every mirror likewise recomputes from live guest state — the trace
                            // supplies recorded values only to compare against. So no recorded field
                            // exists whose corruption makes the replay-side table disagree with the
                            // port the live guest passes. Measured, not assumed (see
                            // `.superpowers/sdd/kport-probe-findings.md`): main's kport IS covered by
                            // the initial Snapshot's Region, but libpthread rewrites it with a plain
                            // guest store that replay re-executes, so the corruption does not
                            // survive; the child's kport is covered by no Region at all, because
                            // `guest_bsdthread_create`'s write is deliberately never recorded. This
                            // arm therefore fires only when retrace itself has a real schedule bug —
                            // exactly the case where a named landmark beats a process abort.
                            match self.b.try_thread_of_port(args[0] as u32) {
                                Ok(t) => t,
                                Err(d) => return Err(Divergence { landmark: self.idx, pc, detail:
                                    format!("pthread_kill target unresolvable on replay: {d}") }),
                            }
                        } else {
                            // kill names the process, not a thread; a process-directed signal may
                            // go to any thread with it unblocked and retrace picks the caller,
                            // exactly as record does.
                            self.b.threads().current()
                        };
                        let sig = args[1];
                        let act = self.b.sigtable().action(sig);
                        // M16 Task 8: record tests the TARGET's mask FIRST and only then consults
                        // the disposition, so this side must too. Until this task replay had no
                        // pended concept at all and decided terminal-vs-caught straight off the
                        // disposition, so a guest raising a BLOCKED signal whose disposition is
                        // Dfl+Terminate made record pend it and carry on while replay took the
                        // terminal path and hunted an Event::Signal that was never recorded. That
                        // surfaced as a divergence rather than as corruption — loud, which is why
                        // it was not a Task 7 defect — but it was still the two dispatch loops
                        // disagreeing about control flow, and the order below closes it.
                        if self.b.should_pend_for(target, sig) {
                            // A pended raise produces ONE landmark — the ordinary Syscall — and no
                            // delivery, so it falls through to the generic dispatch below, which
                            // already verifies (num, args), checks the thread tag, and applies the
                            // recorded return (record's `set_x0_err_and_return(0, false)` is that
                            // arm's `apply_and_return` with ret=0, err=false, writes=[]). Only the
                            // mask side-effect belongs here.
                            self.b.threads_mut().pend(target, sig);
                        } else if matches!(act.disp, retrace_box::Disposition::Dfl)
                            && retrace_arch::default_action(sig) == retrace_arch::DefaultAction::Terminate {
                            match self.events.get(self.idx) {
                                Some(Event::Signal { sig: rsig, pc: rpc, thread: rthread }) => {
                                    if sig != *rsig || pc != *rpc {
                                        return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                            "signal mismatch: live (sig={sig}, pc={pc:#x}) != \
                                             recorded (sig={rsig}, pc={rpc:#x})") });
                                    }
                                    // M16 Task 8: Event::Signal's `thread` is the RAISING thread —
                                    // the trace format fixes it that way so it names the same event
                                    // as `pc`, the raise site — so it is checked against the CALLER
                                    // (`verify_thread`, i.e. the current thread), NOT against
                                    // `target`. A terminal disposition kills the whole PROCESS, so
                                    // which thread the raise named stops mattering; what the event
                                    // pins down is where the process stopped. Do not "simplify"
                                    // this to `target`. Placed after the sig/pc compare for the
                                    // usual reason: report the specific divergence, not the thread
                                    // mismatch it caused.
                                    self.verify_thread(*rthread, pc)?;
                                    match self.events.get(self.idx + 1) {
                                        Some(Event::Snapshot { mem: final_mem, .. }) => {
                                            if let Some(d) = self.b.diff_memory(final_mem) {
                                                return Err(Divergence { landmark: self.idx + 1, pc, detail: d });
                                            }
                                            return Ok(Advance::Exited(ReplayReport {
                                                stdout: std::mem::take(&mut self.stdout),
                                                outcome: Outcome::Signal { sig: *rsig } }));
                                        }
                                        other => return Err(Divergence { landmark: self.idx + 1, pc,
                                            detail: format!("expected final memory Snapshot after Signal, got {other:?}") }),
                                    }
                                }
                                other => return Err(Divergence { landmark: self.idx, pc,
                                    detail: format!("expected recorded Signal, got {other:?} (live raise: sig={sig})") }),
                            }
                        // M12: the CAUGHT counterpart of the terminal arm above. Record appended
                        // TWO events at this one stop — the ordinary Syscall, then the delivery —
                        // so both are consumed here. Letting the generic arm below take the first
                        // would apply_and_return past the svc, and the delivery landmark would then
                        // be met by the next unrelated stop ("expected recorded Exit, got
                        // SignalDelivery", which is what this looked like before the mirror existed).
                        } else if matches!(act.disp, retrace_box::Disposition::Handler(_)) {
                            match self.events.get(self.idx) {
                                Some(Event::Syscall { num: rn, args: ra, thread: rthread, .. })
                                    if *rn == num && *ra == args => {
                                    // M15 Task 4, fix round 1: this arm consumes a recorded
                                    // Event::Syscall landmark and RETURNS before ever reaching the
                                    // generic dispatch below, so it needs its own call to the thread
                                    // oracle — see `verify_thread`'s doc.
                                    self.verify_thread(*rthread, pc)?;
                                }
                                other => return Err(Divergence { landmark: self.idx, pc, detail:
                                    format!("expected the recorded caught raise, got {other:?} \
                                             (live: num={num}, args={args:?})") }),
                            }
                            self.idx += 1;
                            // Record completes the syscall BEFORE building the frame, so the frame
                            // carries the raise's success (x0 = 0, PSTATE.C clear) rather than the
                            // pid argument and a stale carry — measured in spikes/sigraisex0.c.
                            // Omit this and every caught self-raise diverges on those two fields.
                            self.b.complete_syscall_before_delivery(0, false);
                            return self.mirror_delivery(target, sig, retrace_arch::SI_USER, 0, 0, 0, pc);
                        }
                    }
                    // M12 mirror of record's sigreturn arm. Its OWN arm rather than a hook inside
                    // the serviced block below, mirroring record: sigreturn returns no value, so it
                    // must not go through apply_and_return, which would overwrite the x0 and pc
                    // just restored from the frame (Task 5's carry-forward, on this side too).
                    if num == retrace_arch::SYS_SIGRETURN {
                        match self.events.get(self.idx) {
                            Some(Event::Syscall { num: rn, args: ra, thread: rthread, .. })
                                if *rn == num && *ra == args => {
                                // M15 Task 4, fix round 1: this arm consumes a recorded
                                // Event::Syscall landmark and RETURNS before ever reaching the
                                // generic dispatch below, so it needs its own call to the thread
                                // oracle — see `verify_thread`'s doc.
                                self.verify_thread(*rthread, pc)?;
                            }
                            other => return Err(Divergence { landmark: self.idx, pc, detail:
                                format!("expected recorded sigreturn, got {other:?} \
                                         (live args={args:?})") }),
                        }
                        self.b.sigreturn_restore(args[0], args[2]);
                        return self.finish_event();
                    }
                    // M16 Task 9 mirror of record's mask arm — and its OWN arm, rather than the
                    // hook inside the serviced block below where it lived until this task. It had
                    // to be HOISTED: record now appends TWO landmarks at an unmasking call (the
                    // ordinary Syscall, then the materialised SignalDelivery), and the serviced
                    // block ends by falling through to `finish_event()`, which consumes exactly
                    // ONE. Left there, the delivery landmark is met by the next unrelated stop and
                    // reports as "expected recorded syscall, got SignalDelivery" at some landmark
                    // well past the unmask (measured before the hoist: landmark 280 of the
                    // sigthread trace, whose unmask is at 271) — the same confusing failure the
                    // caught-raise mirror above was written to prevent, and for the same reason.
                    // Hoisted, record's arm and this one are the same shape in the same order, so
                    // symmetry rule 1 is structurally visible here and not merely behavioural.
                    if num == retrace_arch::SYS_SIGPROCMASK
                        || num == retrace_arch::SYS_PTHREAD_SIGMASK {
                        let (rret, rerr, rwrites) = match self.events.get(self.idx) {
                            Some(Event::Syscall { num: rn, args: ra, ret, err, writes,
                                                  thread: rthread }) if *rn == num && *ra == args => {
                                // This arm consumes a recorded Event::Syscall landmark and RETURNS
                                // before ever reaching the generic dispatch, so — exactly like the
                                // caught-raise and sigreturn mirrors above — it needs its own call
                                // to the thread oracle (see `verify_thread`'s doc). Ordered after
                                // the (num, args) match for the usual reason: report a genuine
                                // syscall divergence as itself, not as the thread mismatch it
                                // caused.
                                self.verify_thread(*rthread, pc)?;
                                (*ret, *err, writes.clone())
                            }
                            other => return Err(Divergence { landmark: self.idx, pc, detail:
                                format!("expected the recorded sigprocmask, got {other:?} \
                                         (live: num={num}, args={args:?})") }),
                        };
                        // M16: the mask belongs to the CALLING (current) thread. Recomputed with
                        // the same `Box_` calls and the same arguments record used, and the oldset
                        // writeback byte-compared against the recording — that comparison IS the
                        // divergence check (symmetry rule 1). The recorded bytes are what
                        // `apply_and_return` then writes, exactly as the generic path does: the
                        // mirror's job is to keep the TABLE in step and prove the bytes match, not
                        // to re-perform the write.
                        let cur = self.current_thread() as usize;
                        let old = if args[1] != 0 {
                            let set = u32::from_le_bytes(
                                self.b.read_guest(args[1], 4).try_into().unwrap());
                            self.b.threads_mut().set_mask_of(cur, args[0], set)
                        } else {
                            self.b.threads().mask_of(cur)
                        };
                        if args[2] != 0 {
                            let mine = old.to_le_bytes().to_vec();
                            let recorded = rwrites.iter().find(|r| r.ipa == args[2])
                                .map(|r| r.bytes.clone()).unwrap_or_default();
                            if mine != recorded {
                                return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                    "sigprocmask oldset mismatch at {:#x}: recomputed {mine:02x?} \
                                     != recorded {recorded:02x?}", args[2]) });
                            }
                        }
                        self.b.apply_and_return(rret, rerr, &rwrites);
                        // THE ANCHOR's mirror. `take_pending_delivery` is the SAME function record
                        // calls, with the SAME `(b, tid)` — that identity is what makes "both sides
                        // materialise the same signal" true by construction instead of by two
                        // matches that could drift while both stayed green. It CLEARS the bit it
                        // takes, so it is called exactly ONCE per unmask on this side too, and a
                        // signal can never be materialised twice.
                        //
                        // The target is the CALLING thread — the one holding the vCPU — so
                        // `deliver_signal_to`'s Runnable guard is satisfied by construction, the
                        // same argument the record arm states.
                        return match take_pending_delivery(&mut self.b, cur) {
                            Some((psig, _handler)) => {
                                // Consume the Syscall landmark by hand, because this stop consumes
                                // TWO and `finish_event` consumes one: `mirror_delivery` takes the
                                // second. Same shape as the caught-raise mirror above.
                                self.idx += 1;
                                // Record completes the syscall BEFORE building the frame — the
                                // frame's PSTATE comes from SPSR_EL1, which `apply_and_return`
                                // never writes — so this side must too, or every materialised
                                // delivery diverges on that field.
                                self.b.complete_syscall_before_delivery(0, false);
                                // `mirror_delivery` owns both the frame byte-compare and the
                                // recorded-`thread` comparison; this is its THIRD call site and
                                // deliberately not a fourth copy of that logic. `_handler` is the
                                // recomputed handler entry, unused here for the same reason the
                                // caught-raise mirror does not compare one: the frame bytes are
                                // what the oracle checks.
                                self.mirror_delivery(cur, psig, retrace_arch::SI_USER, 0, 0, 0, pc)
                            }
                            // Nothing materialised — a masking call, a query, or an unmask with an
                            // empty pending set. ONE landmark, finished exactly as the generic
                            // path would have finished it.
                            None => self.finish_event(),
                        };
                    }
                    match self.events.get(self.idx) {
                        Some(Event::Syscall { num: rn, args: ra, ret, err, writes, thread: rthread }) => {
                            if num != *rn || args != *ra {
                                return Err(Divergence { landmark: self.idx, pc,
                                    detail: format!("syscall mismatch: live (num={num}, args={args:?}) != recorded (num={rn}, args={ra:?})") });
                            }
                            // M15 Task 4: the thread oracle (see `verify_thread`'s doc for why this
                            // is one of three call sites). Placed AFTER the (num, args) check above,
                            // not before — a genuine syscall divergence usually also produces a
                            // thread mismatch (the wrong thread issuing the wrong call), and it must
                            // be reported as the syscall divergence it is rather than masked by a
                            // thread error it caused. This is the check M14's Status section named
                            // as the oracle's sharpest limit: two threads running the SAME code
                            // issue byte-identical (num, args), so without this, a replay that
                            // schedules the wrong thread onto identical code continues in silence.
                            self.verify_thread(*rthread, pc)?;
                            // M11 mirror of record's serviced-signal arms. Recompute the SAME table
                            // transition and the SAME writeback bytes, then byte-compare against
                            // the recording — that comparison IS the divergence check (symmetry
                            // rule 1), so an asymmetry surfaces as a Divergence rather than as
                            // silent corruption. The recorded writes are then applied by the
                            // existing apply_and_return path, exactly as for any other syscall: the
                            // mirror's job is to keep the TABLE in step and prove the bytes match,
                            // not to re-perform the write.
                            if num == retrace_arch::SYS_SIGACTION {
                                let new = if args[1] != 0 {
                                    Some(retrace_box::decode_act(&self.b.read_guest(args[1], 24)))
                                } else { None };
                                let old = match new {
                                    Some(a) => self.b.sigtable_mut().set_action(args[0], a),
                                    None => self.b.sigtable().action(args[0]),
                                };
                                if args[2] != 0 {
                                    let mine = retrace_box::encode_oldact(old).to_vec();
                                    let recorded = writes.iter().find(|r| r.ipa == args[2])
                                        .map(|r| r.bytes.clone()).unwrap_or_default();
                                    if mine != recorded {
                                        return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                            "sigaction oldact mismatch at {:#x}: recomputed {mine:02x?} \
                                             != recorded {recorded:02x?}", args[2]) });
                                    }
                                }
                            }
                            if num == retrace_arch::SYS_SIGALTSTACK {
                                // Fast-follow: a real mirror of record's arm (`SYS_SIGALTSTACK` in
                                // `record_box`, above), on the model of the `sigaction` mirror just
                                // above this one. The table update stays guarded on `args[0]` —
                                // installing a new stack is the only thing that changes it — but
                                // the byte-compare belongs under its OWN guard, `args[1]`, so a pure
                                // query `sigaltstack(NULL, &old)` now enters this compare too instead
                                // of skipping it. `rthread` (not `current_thread()`) is the thread
                                // index, for identity with record — `verify_thread` above already
                                // proved the two agree, but taking the same value record took is
                                // what makes the identity structural rather than incidental.
                                let tid = *rthread as usize;
                                let new = if args[0] != 0 {
                                    Some(retrace_box::decode_stack(&self.b.read_guest(args[0], 24)))
                                } else { None };
                                // `old` comes from `set_altstack_of`'s RETURN value, exactly as
                                // record takes it — by the time a read-back would run, the update
                                // has already overwritten the table entry it would read.
                                let old = match new {
                                    Some(ss) => self.b.threads_mut().set_altstack_of(tid, Some(ss)),
                                    None => self.b.threads().altstack_of(tid),
                                };
                                if args[1] != 0 {
                                    let mine = retrace_box::encode_oldstack(old.unwrap_or((0, 0, 0))).to_vec();
                                    let recorded = writes.iter().find(|r| r.ipa == args[1])
                                        .map(|r| r.bytes.clone()).unwrap_or_default();
                                    if mine != recorded {
                                        return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                            "sigaltstack oldstack mismatch at {:#x}: recomputed {mine:02x?} \
                                             != recorded {recorded:02x?}", args[1]) });
                                    }
                                }
                            }
                            // M16 Task 9, fix round 1: sigpending's mirror. Record's arm used to
                            // write a constant zero, so there was nothing to recompute and nothing
                            // to check; Task 9 made it write real per-thread state
                            // (`pending_of`), and symmetry rule 1 then applies with teeth —
                            // recompute the SAME value with the SAME call on the SAME thread and
                            // byte-compare it against the recording, because that comparison IS
                            // the divergence check. Without it a divergent pending set would be
                            // painted over with the recorded bytes and the run would continue;
                            // it would surface, if at all, only later and only if it changed what
                            // the next unmask materialised (which the `SignalDelivery` mirror does
                            // compare) — a landmark after the one that could have named it.
                            //
                            // This one stays a HOOK here, deliberately, while the mask mirror
                            // above was hoisted into its own arm. The hoist was forced by landmark
                            // arithmetic: an unmasking call appends TWO landmarks (the Syscall,
                            // then a materialised SignalDelivery) and this block ends in
                            // `finish_event()`, which consumes exactly ONE. `sigpending` appends
                            // exactly ONE landmark and materialises nothing, so the generic path
                            // finishes it correctly and an arm of its own would be structure
                            // copied for its own sake.
                            //
                            // Guarded on args[0] != 0 for the same reason record is: a NULL
                            // out-pointer means the guest asked for nothing, record wrote no
                            // region, and there is nothing to compare.
                            if num == retrace_arch::SYS_SIGPENDING && args[0] != 0 {
                                // The CALLING thread's set, exactly as record's arm takes it.
                                let cur = self.current_thread() as usize;
                                let mine = self.b.threads().pending_of(cur).to_le_bytes().to_vec();
                                let recorded = writes.iter().find(|r| r.ipa == args[0])
                                    .map(|r| r.bytes.clone()).unwrap_or_default();
                                if mine != recorded {
                                    return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                        "sigpending set mismatch at {:#x}: recomputed {mine:02x?} \
                                         != recorded {recorded:02x?}", args[0]) });
                                }
                            }
                            // Learn the guest's task-port name (mirror of record) from the recorded −28 result.
                            if num == MACH_TASK_SELF && !*err { self.guest_task_port = Some(*ret); }
                            // Mirror fd-1/2 write output (the buffer is already filled by prior applied reads).
                            // Same predicate as record's arm — see `is_console_write`.
                            if retrace_arch::is_console_write(num, args[0]) {
                                self.stdout.extend_from_slice(&self.b.read_guest(args[1], args[2] as usize));
                            }
                            // mach_msg2: re-service (the mapping must exist on replay too), verify
                            // the recomputed reply byte-equals the recording (divergence landmark),
                            // then apply. Forwarded allowlist entries just apply recorded writes.
                            if num == MACH_MSG2 {
                                let m = machmsg::Msg2::unpack(&args);
                                match machmsg::route(&m, self.guest_task_port) {
                                    machmsg::Route::ServiceVmMap => {
                                        let buf = self.b.read_guest(m.data, m.send_size as usize);
                                        let req = machmsg::decode_vm_map(&buf).map_err(|e| Divergence {
                                            landmark: self.idx, pc, detail: format!("replay vm_map decode: {e}") })?;
                                        let anywhere = req.flags as u64 & VM_FLAGS_ANYWHERE != 0;
                                        // Same reservation/commit split as record (must reproduce the
                                        // identical returned address for the byte-equality check below).
                                        let ipa = if req.cur_protection == 0 {
                                            self.b.guest_vm_reserve(req.address, req.size, anywhere)
                                        } else {
                                            let exec = req.cur_protection as u64 & PROT_EXEC != 0;
                                            self.b.guest_vm_map(req.address, req.size, anywhere, exec)
                                        };
                                        let reply = machmsg::encode_vm_map_reply(m.reply_port, ipa);
                                        if writes.len() != 1 || writes[0].bytes != reply {
                                            return Err(Divergence { landmark: self.idx, pc,
                                                detail: format!("mach_vm_map reply mismatch: replay ipa {ipa:#x}") });
                                        }
                                        self.b.apply_and_return(*ret, *err, writes);
                                    }
                                    machmsg::Route::ServiceGetSpecialPort => {
                                        // The reply carries a REAL, nondeterministic minted port name
                                        // (M2-xpcport, task_self posture): apply the recorded reply VERBATIM
                                        // — do NOT recompute/byte-compare (the name cannot be regenerated;
                                        // re-adding the byte-compare would guarantee a divergence). The
                                        // decode+assert(which==4) stays as a cheap deterministic guard.
                                        let buf = self.b.read_guest(m.data, m.send_size as usize);
                                        let which = machmsg::decode_get_special_port(&buf).map_err(|e| Divergence {
                                            landmark: self.idx, pc, detail: format!("replay get_special_port decode: {e}") })?;
                                        assert_eq!(which, 4,
                                            "only TASK_BOOTSTRAP_PORT (4) is modeled; got which={which}");
                                        self.b.apply_and_return(*ret, *err, writes);
                                    }
                                    machmsg::Route::ServiceSetSpecialPort => {
                                        // Deterministic mig_reply_error reply (M2-setport) → STANDARD
                                        // symmetric posture: recompute and byte-compare against the
                                        // recording (the divergence oracle), then apply. (Contrast
                                        // ServiceGetSpecialPort, whose nondeterministic minted name forces
                                        // verbatim-apply — do NOT copy that here.)
                                        let buf = self.b.read_guest(m.data, m.send_size as usize);
                                        let which = machmsg::decode_set_special_port(&buf).map_err(|e| Divergence {
                                            landmark: self.idx, pc, detail: format!("replay set_special_port decode: {e}") })?;
                                        assert_eq!(which, 10,
                                            "only TASK_DEBUG_CONTROL_PORT (10) is modeled; got which={which}");
                                        let reply = machmsg::encode_mig_error(m.msgh_id, m.reply_port, machmsg::KERN_SUCCESS);
                                        if writes.len() != 1 || writes[0].bytes != reply {
                                            return Err(Divergence { landmark: self.idx, pc,
                                                detail: "task_set_special_port reply mismatch".into() });
                                        }
                                        self.b.apply_and_return(*ret, *err, writes);
                                    }
                                    machmsg::Route::StubMigReply(retcode) => {
                                        let reply = machmsg::encode_mig_error(m.msgh_id, m.reply_port,
                                                                              retcode);
                                        if writes.len() != 1 || writes[0].bytes != reply {
                                            return Err(Divergence { landmark: self.idx, pc,
                                                detail: "mach_msg2 stub reply mismatch".into() });
                                        }
                                        self.b.apply_and_return(*ret, *err, writes);
                                    }
                                    machmsg::Route::Forward(_) => self.b.apply_and_return(*ret, *err, writes),
                                    machmsg::Route::Unsupported(why) => {
                                        return Err(Divergence { landmark: self.idx, pc,
                                            detail: format!("unsupported mach_msg2 on replay: {why}") });
                                    }
                                }
                                return self.finish_event();
                            }
                            // mmap: recreate the mapping deterministically (the guest reproduces its own
                            // stores by re-execution). The IPA must match the recording exactly.
                            if num == retrace_arch::SYS_MMAP && args[3] & MAP_ANON != 0 {
                                // A rejected MAP_FIXED address is recomputed here, not replayed
                                // blindly: `fixed_fits` is a pure function of the request and the
                                // fixed IPA geometry, so replay must reach the SAME verdict. The
                                // (ret, err) comparison below is that divergence check.
                                let (ipa, failed) = match self.b.guest_mmap(args[0], args[1], args[2], args[3]) {
                                    Ok(ipa) => (ipa, false),
                                    Err(errno) => (errno, true),
                                };
                                if (ipa, failed) != (*ret, *err) {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!("mmap mismatch: replay (ret {ipa:#x}, err {failed}) != recorded (ret {ret:#x}, err {err})") });
                                }
                                self.b.set_x0_err_and_return(*ret, *err);
                                return self.finish_event();
                            }
                            // file-backed mmap (Task 8): anon-alloc + address identically (no file
                            // access), verify the recreated IPA equals the recorded ret (this is what
                            // makes MAP_FIXED correct on replay), then stage the recorded bytes.
                            if num == retrace_arch::SYS_MMAP {
                                let (ipa, failed) = match self.b.guest_mmap_replay(args[0], args[1], args[2], args[3]) {
                                    Ok(ipa) => (ipa, false),
                                    Err(errno) => (errno, true),
                                };
                                if (ipa, failed) != (*ret, *err) {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!("mmap_file mismatch: replay (ret {ipa:#x}, err {failed}) != recorded (ret {ret:#x}, err {err})") });
                                }
                                // Same exec promotion as record: the guest executes the mmap'd code on
                                // replay too (replay runs the guest, only faking syscall results), so the
                                // exec pages must exist here as well — before the recorded bytes are staged.
                                // Nothing was mapped on the rejected path, so nothing to promote.
                                if !failed && args[2] & 0x4 != 0 { self.b.set_region_exec(ipa, args[1]); }
                                self.b.apply_and_return(*ret, *err, writes);
                                return self.finish_event();
                            }
                            // mach_vm_allocate / mach_vm_map: recreate the guest allocation
                            // deterministically (so the memory exists in stage-2 for the guest to use),
                            // then apply the recorded IPA write + KERN_SUCCESS. The recomputed IPA must
                            // equal what was recorded (bump allocator is deterministic).
                            if num == MACH_VM_ALLOCATE || num == MACH_VM_MAP {
                                let (addr_ptr, size, flags, prot) = vm_map_args(num, &args);
                                let anywhere = flags & VM_FLAGS_ANYWHERE != 0;
                                let exec = prot & PROT_EXEC != 0;
                                let req = if self.b.is_mapped(addr_ptr) { self.b.read_u64(addr_ptr) } else { 0 }; // hint (honored when free)
                                // Same reservation/commit split as record (cur_protection == 0 =>
                                // reserve, else eagerly back); must reproduce the identical returned IPA
                                // for the byte-equality check below.
                                let ipa = if prot == 0 {
                                    self.b.guest_vm_reserve(req, size, anywhere)
                                } else {
                                    self.b.guest_vm_map(req, size, anywhere, exec)
                                };
                                let recorded_ipa = writes.first()
                                    .map(|w| u64::from_le_bytes(w.bytes[..8].try_into().unwrap())).unwrap_or(ipa);
                                if ipa != recorded_ipa {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!("mach_vm_map ipa mismatch: replay {ipa:#x} != recorded {recorded_ipa:#x}") });
                                }
                                self.b.apply_and_return(*ret, *err, writes);
                                return self.finish_event();
                            }
                            if num == MACH_VM_DEALLOCATE {
                                self.b.guest_munmap(args[1], args[2]);
                                self.b.set_x0_err_and_return(*ret, *err);
                                return self.finish_event();
                            }
                            // M13: the record arm's mirror. Same call, same args, so the protection
                            // state the replay guest runs against is recomputed rather than
                            // recorded (symmetry rule 1) — without this the replay guest survives
                            // the store the recording died on.
                            if num == MACH_VM_PROTECT {
                                self.b.guest_mprotect(args[1], args[2], args[4]);
                                self.b.set_x0_err_and_return(*ret, *err);
                                return self.finish_event();
                            }
                            // M18 t5: the record arm's mirror (symmetry rule 1). Same method, same
                            // args, so both sides capture the identical three addresses and compute
                            // the identical return. The byte-compare below is the divergence check;
                            // it is vacuous while the return is a constant and becomes the oracle
                            // the moment it is not — the same shape as bsdthread_create's mirror.
                            if num == retrace_arch::SYS_BSDTHREAD_REGISTER {
                                let rc = self.b.guest_bsdthread_register(args);
                                if rc != *ret {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!("bsdthread_register rc mismatch: replay {rc:#x} != recorded {ret:#x}") });
                                }
                                self.b.set_x0_err_and_return(*ret, *err);
                                return self.finish_event();
                            }
                            // M18 Stage 2a: the record arms' mirrors (symmetry rule 1). Same
                            // method, same args, so both sides compute the identical return; the
                            // byte-compare below is the divergence check. Placed HERE, with the
                            // other `if num ==` mirrors, deliberately: this arm already called
                            // `verify_thread` at the top of the arm, before the whole chain, so these
                            // inherit the thread oracle and must NOT add their own. See the spec's
                            // "Stage 2, split by what is measured" for the measurement behind that.
                            if num == retrace_arch::SYS_WORKQ_OPEN {
                                let rc = self.b.guest_workq_open(args);
                                if rc != *ret {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!("workq_open rc mismatch: replay {rc:#x} != recorded {ret:#x}") });
                                }
                                self.b.set_x0_err_and_return(*ret, *err);
                                return self.finish_event();
                            }
                            if num == retrace_arch::SYS_WORKQ_KERNRETURN {
                                let rc = self.b.guest_workq_kernreturn(args);
                                if rc != *ret {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!("workq_kernreturn rc mismatch: replay {rc:#x} != recorded {ret:#x}") });
                                }
                                self.b.set_x0_err_and_return(*ret, *err);
                                return self.finish_event();
                            }
                            // M14 Task 7: the record arm's mirror (symmetry rule 1). Record and
                            // replay must call `guest_bsdthread_create` with IDENTICAL args so both
                            // build an identical thread table — omit this and replay runs a
                            // one-thread table against a two-thread recording, surfacing as a
                            // divergence at the child's first syscall rather than as a clean error.
                            if num == retrace_arch::SYS_BSDTHREAD_CREATE {
                                let rc = self.b.guest_bsdthread_create(args);
                                // Same divergence-check shape as the mach_vm_map arm above: bind the
                                // recomputed return and byte-compare it against the recording rather
                                // than discarding it. Vacuous today (guest_bsdthread_create always
                                // returns 0), but silently wrong the moment the emulator's return
                                // becomes conditional — that comparison, not the constant, IS the
                                // oracle (symmetry rule 1).
                                if rc != *ret {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!("bsdthread_create rc mismatch: replay {rc:#x} != recorded {ret:#x}") });
                                }
                                self.b.set_x0_err_and_return(*ret, *err);
                                return self.finish_event();
                            }
                            // M14 Task 8: the record arm's mirror (symmetry rule 1). `rc` is bound
                            // and compared against the recorded `ret` (fix round 1, I-2) — the
                            // same divergence-check shape as `bsdthread_create`'s mirror above;
                            // vacuous today (guest_bsdthread_terminate always returns 0) but is
                            // the oracle itself the moment that changes.
                            if num == retrace_arch::SYS_BSDTHREAD_TERMINATE {
                                let rc = self.b.guest_bsdthread_terminate(args);
                                if rc != *ret {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!("bsdthread_terminate rc mismatch: replay {rc:#x} != recorded {ret:#x}") });
                                }
                                // M14 Task 9: same posture as record's mirror, and the same
                                // deliberate ABSENCE — no `set_x0_err_and_return`. The thread is
                                // `Exited`, so the next `Box_::run()` reschedules instead of
                                // resuming it. Task 8's fail-loud panic stood here only until a
                                // scheduler existed to switch away; it now does, below the trace,
                                // where record and replay reach it identically.
                                return self.finish_event();
                            }
                            // M14 Task 8: the record arm's mirror (symmetry rule 1). Record and
                            // replay must call `guest_ulock_wait` with IDENTICAL args so both
                            // land the SAME thread in the SAME state (Runnable vs Blocked) —
                            // omit this and a replay guest schedules differently from the
                            // recording the moment Task 9 wires the scheduler in.
                            //
                            // Fix round 1, I-2: `rc` is now the raw signed x0 word (`ULF_NO_ERRNO`
                            // returns `-errno` with carry clear — see `Box_::guest_ulock_wait`),
                            // so replay's recomputed `err` is the CONSTANT `false` record's arm
                            // writes. It is still bound and compared rather than skipped: a
                            // recorded `err: true` on this syscall could only come from a trace
                            // some other build wrote, and that is a divergence, not something to
                            // pass silently into `set_x0_err_and_return`. The `rc` half is the
                            // live oracle — it distinguishes the EFAULT return from the normal one.
                            if num == retrace_arch::SYS_ULOCK_WAIT {
                                let (rc, rerr) = (self.b.guest_ulock_wait(args), false);
                                if rc != *ret || rerr != *err {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!(
                                            "ulock_wait mismatch: replay ({rc:#x},{rerr}) != recorded ({ret:#x},{err})") });
                                }
                                self.b.set_x0_err_and_return(*ret, *err);
                                return self.finish_event();
                            }
                            // M14 Task 9: the record arm's mirror (symmetry rule 1). Record and
                            // replay must call `guest_ulock_wake` with IDENTICAL args so both wake
                            // the SAME set of threads — omit this and replay leaves the joiner
                            // blocked, `pick_next()` returns None, and a run that recorded cleanly
                            // deadlocks on replay. The rc byte-compare is vacuous today
                            // (guest_ulock_wake always returns 0, which is what the guest's own
                            // `__pthread_joiner_wake` accepts) but IS the oracle the moment that
                            // becomes conditional.
                            //
                            // Fix round 1, M-1: `err` is compared too, not just `rc` — the same
                            // shape as the 515 mirror above, and for the same reason. Record's arm
                            // hardcodes `err: false` (`guest_ulock_wake` has no failure path; if it
                            // grows one it must answer `-errno` with carry clear, since its
                            // measured operation word also carries `ULF_NO_ERRNO`), so replay
                            // recomputes that constant and checks it. The neighbouring comment
                            // argues the byte-compare "IS the oracle the moment that becomes
                            // conditional" — leaving `err` out would have made that false for half
                            // the pair.
                            if num == retrace_arch::SYS_ULOCK_WAKE {
                                let ((rc, woken), rerr) = (self.b.guest_ulock_wake(args), false);
                                if rc != *ret || rerr != *err {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!(
                                            "ulock_wake mismatch: replay ({rc:#x},{rerr}) != recorded ({ret:#x},{err})") });
                                }
                                self.b.set_x0_err_and_return(*ret, *err);
                                // M17: record's wake arm materialises a signal pended on a thread
                                // it just woke, appending a SECOND landmark. This side must consume
                                // both — `finish_event` takes one, `mirror_delivery` takes the
                                // other — exactly as the mask arm at :1478-1501 does. Getting this
                                // wrong does not corrupt anything quietly: the delivery landmark
                                // would be met by the next unrelated stop and reported as "expected
                                // recorded syscall, got SignalDelivery" at some landmark past the
                                // wake.
                                //
                                // The same `Box_` calls with the same arguments as record, in the
                                // same order, so which signal materialises on which thread is
                                // identical by construction rather than by two matches agreeing.
                                let deliver_to: Vec<usize> = woken.iter().copied()
                                    .filter(|&t| self.b.threads().peek_deliverable(t).is_some())
                                    .collect();
                                // M17 fix round 5: BOTH bounds here PANIC on replay rather than
                                // returning a named `Divergence`, which is what the neighbouring
                                // impossible-ish condition does (`check_deliverable`, in
                                // `mirror_delivery`). The difference is deliberate: both of these
                                // are recomputed entirely from live state that no recorded field
                                // steers, and record asserts the same two bounds first, so no
                                // recordable trace can reach either — firing one means retrace's own
                                // model is wrong, not that this replay diverged from its recording.
                                assert!(deliver_to.len() <= 1,
                                    "one wake made {} threads deliverable at once ({deliver_to:?}) \
                                     — record asserts the same bound; see its arm",
                                    deliver_to.len());
                                return match deliver_to.first() {
                                    Some(&wtid) => {
                                        // The second bound, and record's mirror: at most ONE signal
                                        // materialises per wake. Checked OUTSIDE the `Some`/`None`
                                        // match below, because the `None` (`Ign`) case swallows a
                                        // leftover just as silently — record's arm carries the full
                                        // reasoning. Same check, same message, same position in the
                                        // call sequence as record's, which is symmetry rule 1: the
                                        // two sides cannot drift on what they consume per wake.
                                        let taken = take_pending_delivery(&mut self.b, wtid);
                                        let second = self.b.threads().peek_deliverable(wtid);
                                        assert!(second.is_none(),
                                            "woken thread {wtid} still has deliverable signal \
                                             {second:?} after this wake materialised one: M17 \
                                             materialises at most ONE signal per wake, so the \
                                             second is silently swallowed — the thread is Runnable \
                                             now, and assert_no_stranded_signals scans Blocked \
                                             threads only. Queueing at a wake is deliberately \
                                             unmodelled because no guest in the tree measures it; \
                                             measure one before modelling it.");
                                        match taken {
                                            Some((psig, _handler)) => {
                                                // The SAME Box_ calls record makes, in the SAME
                                                // order, with the SAME arguments — that identity IS
                                                // symmetry rule 1 holding by construction rather
                                                // than by two matches happening to agree.
                                                //
                                                // `complete_saved_syscall_before_delivery` IS
                                                // called and `complete_syscall_before_delivery` is
                                                // NOT, for the reason record's arm spells out at
                                                // length: the live vCPU here is the WAKER, so the
                                                // live version would correct the wrong thread's
                                                // PSTATE — while the receiver's own saved SPSR was
                                                // measured (0x60000000, C set,
                                                // `crates/retrace/tests/blockedctx.rs`) to disagree
                                                // with the completed x0 beside it. Omit this call
                                                // and replay's frame bytes differ from record's,
                                                // which surfaces as a divergence in
                                                // `mirror_delivery`'s byte-compare rather than as
                                                // silent corruption.
                                                self.b.complete_saved_syscall_before_delivery(wtid, false);
                                                // Consume the Syscall landmark by hand;
                                                // mirror_delivery takes the SignalDelivery.
                                                self.idx += 1;
                                                self.mirror_delivery(wtid, psig, retrace_arch::SI_USER,
                                                                     0, 0, 0, pc)
                                            }
                                            None => self.finish_event(),
                                        }
                                    }
                                    None => self.finish_event(),
                                };
                            }
                            // M18 Stage 2b: the record arms' mirrors (symmetry rule 1). Both call
                            // the SAME `Box_` method with IDENTICAL args, so both sides move the
                            // thread table identically — that identity is what makes the rule hold
                            // by construction rather than by two matches happening to agree, and the
                            // recorded `(num, args)` byte-compare at the top of this arm IS the
                            // divergence check. Omit the wait mirror and replay leaves main Runnable
                            // against a recording in which it blocked; omit the signal mirror and
                            // replay leaves main blocked, `pick_next` returns None, and a run that
                            // recorded cleanly deadlocks — the same failure the ulock pair's mirrors
                            // just above describe.
                            //
                            // NO `verify_thread` of their own, for the reason the Stage 2a mirrors
                            // above state: this arm already called it, before the whole `if num ==`
                            // chain, so every mirror in the chain inherits the thread oracle. A
                            // second call would re-ask an identical question about an identical
                            // landmark — neither `guest_sem_wait` nor `guest_sem_signal` switches
                            // the vCPU (the switch happens below the trace, in `Box_::run()`), so
                            // `current_thread()` is unchanged across both.
                            if num == retrace_arch::MACH_SEMAPHORE_WAIT {
                                let (rc, rerr) = (self.b.guest_sem_wait(args), false);
                                // `err` is bound and compared, not skipped — the same shape as the
                                // `SYS_ULOCK_WAIT` mirror above and for its reason: record's arm
                                // hardcodes `false`, so a recorded `err: true` here could only come
                                // from a trace some other build wrote, which is a divergence rather
                                // than something to pass silently into `set_x0_err_and_return`.
                                if rc != *ret || rerr != *err {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!(
                                            "sem_wait mismatch: replay ({rc:#x},{rerr}) != recorded ({ret:#x},{err})") });
                                }
                                self.b.set_x0_err_and_return(*ret, *err);
                                return self.finish_event();
                            }
                            if num == retrace_arch::MACH_SEMAPHORE_SIGNAL {
                                let ((rc, woken), rerr) = (self.b.guest_sem_signal(args), false);
                                if rc != *ret || rerr != *err {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!(
                                            "sem_signal mismatch: replay ({rc:#x},{rerr}) != recorded ({ret:#x},{err})") });
                                }
                                self.b.set_x0_err_and_return(*ret, *err);
                                // Record's arm ASSERTS on `woken` rather than materialising a
                                // pended signal, and this side makes the IDENTICAL choice with the
                                // identical check after the identical call sequence. The two sides
                                // must agree on what happens to `woken` or replay diverges from
                                // record on a path the byte-compare cannot see: neither side appends
                                // a second landmark either way, so nothing in the trace would show
                                // the disagreement.
                                //
                                // PANICS rather than returning a named `Divergence`, for the reason
                                // the `SYS_ULOCK_WAKE` mirror's own two bounds do: it is recomputed
                                // entirely from live state that no recorded field steers, and record
                                // asserts the same bound first, so no recordable trace can reach it.
                                // Firing it means retrace's own model is wrong, not that this replay
                                // diverged from its recording.
                                let deliverable: Vec<usize> = woken.iter().copied()
                                    .filter(|&t| self.b.threads().peek_deliverable(t).is_some())
                                    .collect();
                                assert!(deliverable.is_empty(),
                                    "semaphore signal woke thread(s) {deliverable:?} carrying a \
                                     pending deliverable signal — record asserts the same bound \
                                     first; see its arm for why M18 Stage 2b refuses this by value \
                                     rather than modelling it on unmeasured saved state");
                                return self.finish_event();
                            }
                            // shared_region_check_np (#294): install the demand-pager on replay too
                            // (record installed it here), so cache faults regenerate identical pages, then
                            // apply the recorded base write via the generic path.
                            if num == retrace_arch::SYS_SHARED_REGION_CHECK_NP {
                                self.b.install_cache_pager();
                                self.b.apply_and_return(*ret, *err, writes);
                                return self.finish_event();
                            }
                            // shared_region_map_and_slide_2_np (#536): install the demand-pager on
                            // replay too (record installed it here), so cache faults regenerate identical
                            // pages.
                            if num == retrace_arch::SYS_SHARED_REGION_MAP_AND_SLIDE_2_NP {
                                self.b.install_cache_pager();
                                self.b.set_x0_err_and_return(*ret, *err);
                                return self.finish_event();
                            }
                            // munmap/mprotect (debt #2): honor them for real on replay too, so a later
                            // mmap in the trace can reuse the address exactly like it did on record.
                            if num == retrace_arch::SYS_MUNMAP {
                                self.b.guest_munmap(args[0], args[1]);
                                self.b.set_x0_err_and_return(0, false);
                                return self.finish_event();
                            }
                            if num == retrace_arch::SYS_MPROTECT {
                                self.b.guest_mprotect(args[0], args[1], args[2]);
                                self.b.set_x0_err_and_return(0, false);
                                return self.finish_event();
                            }
                            // sysctl(KERN_USRSTACK64) mirror: recompute the SAME reply from the box's
                            // own stack geometry and byte-compare it against the recording — that
                            // comparison IS the divergence check (symmetry rule 1). The geometry is
                            // re-derived by restore() from the snapshot, so a static trace recomputes
                            // the static answer and a dynamic one the dynamic answer.
                            if num == retrace_arch::SYS_SYSCTL && is_usrstack64_mib(&self.b, args) {
                                let recomputed = self.b.usrstack64_reply(args);
                                if recomputed != *writes {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!(
                                            "sysctl usrstack64 reply mismatch: replay {recomputed:?} != recorded {writes:?}") });
                                }
                                self.b.apply_and_return(*ret, *err, writes);
                                return self.finish_event();
                            }
                            // getrlimit(RLIMIT_STACK) mirror — same posture as the usrstack64 mirror
                            // above: recompute from the box's own geometry, byte-compare against the
                            // recording (that comparison IS the divergence check), then apply.
                            if num == retrace_arch::SYS_GETRLIMIT
                                && (args[0] & !retrace_arch::RLIMIT_POSIX_FLAG) == retrace_arch::RLIMIT_STACK {
                                let recomputed = self.b.rlimit_stack_reply(args);
                                if recomputed != *writes {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!(
                                            "getrlimit stack reply mismatch: replay {recomputed:?} != recorded {writes:?}") });
                                }
                                self.b.apply_and_return(*ret, *err, writes);
                                return self.finish_event();
                            }
                            // M10 fd mirror. Guest fd numbers are a pure function of the guest's own
                            // open/dup/close sequence, so replay can recompute what the allocator
                            // WOULD have produced and byte-compare it against the recording — that
                            // comparison IS the divergence check (symmetry rule 1, the standard
                            // posture). This is deliberately NOT the ServiceGetSpecialPort
                            // verbatim-apply exception: that one applies blindly because a minted
                            // port name is nondeterministic and cannot be regenerated. An fd can.
                            //
                            // Replay keeps the guest-visible half of the table only; it opens no
                            // host fd, so there is nothing to bind.
                            if !*err && retrace_arch::allocates_fd(num) {
                                let expect = self.b.fds_mut().alloc();
                                if expect != *ret {
                                    return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                        "fd divergence: recording says syscall {num} returned fd {ret}, \
                                         but the guest's own open/close sequence yields {expect}. A \
                                         recorded HOST fd (typically >= 16) means the trace predates \
                                         M10's fd table.") });
                                }
                            }
                            if !*err && (num == retrace_arch::SYS_CLOSE
                                      || num == retrace_arch::SYS_CLOSE_NOCANCEL) {
                                // Mirror record's slot retirement so the two tables stay in step —
                                // otherwise the next alloc diverges. fd 0/1/2 never reach here
                                // (is_console_close handles them in the arm above).
                                self.b.fds_mut().close(args[0]);
                            }
                            // Apply recorded kernel writes + feed ret; NO real syscall executes.
                            self.b.apply_and_return(*ret, *err, writes);
                            return self.finish_event();
                        }
                        other => return Err(Divergence { landmark: self.idx, pc,
                            detail: format!("expected recorded syscall, got {other:?} (truncated={})", self.truncated) }),
                    }
                }
                Stop::Fault { pc, esr, far } => {
                    // M12 mirror of record's fault-delivery arm, and it must come FIRST: the
                    // disposition decides whether this fault is a crash or a delivery, exactly as
                    // it does on the record side. Placing it after the recorded-Crash verify would
                    // report "expected recorded Crash, got SignalDelivery" — a confusing divergence
                    // that reads as a recording bug and is a dispatch bug (M11 line 757's lesson).
                    //
                    // The recomputed disposition comes from the REPLAY-side table, which the
                    // serviced sigaction mirror keeps in step; that is what makes a guest which
                    // installed a handler take this branch on both sides.
                    let (sig, si_code) = retrace_arch::signal_of_esr(esr);
                    if matches!(self.b.sigtable().action(sig).disp,
                                retrace_box::Disposition::Handler(_)) {
                        // M16 Task 8: a hardware fault has no target port to resolve — it is always
                        // attributed to whichever thread's vCPU context trapped — so `current` is
                        // permanently correct here, matching record's fault arm, which tags its
                        // SignalDelivery with the live `thread`.
                        let cur = self.b.threads().current();
                        return self.mirror_delivery(cur, sig, si_code, far, esr, far, pc);
                    }
                    // M6 mirror of record's crash arm. The triple compare IS the divergence check
                    // (symmetry rule 1); then the final-memory landmark, exactly like Exit.
                    match self.events.get(self.idx) {
                        Some(Event::Crash { pc: rpc, esr: resr, far: rfar, thread: rthread }) => {
                            if pc != *rpc || esr != *resr || far != *rfar {
                                return Err(Divergence { landmark: self.idx, pc,
                                    detail: format!("crash mismatch: live (pc={pc:#x}, esr={esr:#x}, far={far:#x}) != recorded (pc={rpc:#x}, esr={resr:#x}, far={rfar:#x})") });
                            }
                            // M16 Task 11: the thread oracle (see `verify_thread`'s doc). Placed
                            // AFTER the (pc, esr, far) compare above, not before, for the usual
                            // reason: a genuine crash mismatch should be reported as itself, not
                            // masked by the thread mismatch it caused.
                            self.verify_thread(*rthread, pc)?;
                            match self.events.get(self.idx + 1) {
                                Some(Event::Snapshot { mem: final_mem, .. }) => {
                                    if let Some(d) = self.b.diff_memory(final_mem) {
                                        return Err(Divergence { landmark: self.idx + 1, pc, detail: d });
                                    }
                                    return Ok(Advance::Exited(ReplayReport {
                                        stdout: std::mem::take(&mut self.stdout),
                                        outcome: Outcome::Crash { pc, esr, far } }));
                                }
                                other => return Err(Divergence { landmark: self.idx + 1, pc,
                                    detail: format!("expected final memory Snapshot after Crash, got {other:?}") }),
                            }
                        }
                        other => return Err(Divergence { landmark: self.idx, pc,
                            detail: format!("expected recorded Crash, got {other:?} (live fault: pc={pc:#x} far={far:#x})") }),
                    }
                }
                Stop::Other { esr } => {
                    // A hardware breakpoint (M3 debugger `continue`/scan) delivers here with an
                    // ESR_EL2 breakpoint class; surface it as `Advance::Break` BEFORE the fault
                    // fallbacks so it is not misread as a stage-2 abort. Only the debugger arms
                    // breakpoints, so this is unreachable under the plain `replay()` oracle.
                    if matches!(retrace_arch::ec_of(esr), retrace_arch::Ec::Breakpoint) {
                        return Ok(Advance::Break);
                    }
                    // A hardware watchpoint (M5 debugger) delivers here identically; surface it
                    // BEFORE the fault fallbacks. Only the debugger arms watchpoints.
                    if matches!(retrace_arch::ec_of(esr), retrace_arch::Ec::Watchpoint) {
                        return Ok(Advance::Watch { thread: self.current_thread() });
                    }
                    // Cache-window fault: page it in (regenerated identically to record) and re-run.
                    if self.b.page_in_cache(self.b.fault_ipa()) { continue; }
                    if self.b.commit_reserved_page(self.b.fault_ipa()) { continue; }
                    return Err(Divergence { landmark: self.idx, pc: self.b.pc(), detail: self.b.describe_stop(esr) });
                }
                // Stop::Step is only produced by Box_::step(); advance drives run(), never step().
                Stop::Step => unreachable!("replay drives run(), which never single-steps"),
            }
        }
    }

    /// Advance to exactly landmark `n` (idx == n). Errors (never re-seeks backward, and never
    /// runs past the guest's exit) as a Divergence, so the debugger's positioning is fail-loud.
    pub fn advance_to_landmark(&mut self, n: usize) -> Result<(), Divergence> {
        if n < self.idx {
            return Err(Divergence { landmark: self.idx, pc: self.position(),
                detail: format!("cannot seek backward to landmark {n} (already at {})", self.idx) });
        }
        while self.idx < n {
            if let Advance::Exited(_) = self.advance()? {
                return Err(Divergence { landmark: self.idx, pc: self.position(),
                    detail: format!("run exited before landmark {n}") });
            }
        }
        // M12: a caught raise writes TWO landmarks at ONE stop (the syscall, then the delivery),
        // so the coordinate between them names a position the guest never occupies — the syscall
        // is completed and the frame written as one indivisible transition. M16 Task 9 gives the
        // unmasking `sigprocmask` that same two-landmark shape, for the same reason and with the
        // same consequence here, so this check now covers two arms rather than one. Overshooting
        // such a coordinate is the honest outcome; overshooting it SILENTLY is not, and every caller here is a debugger
        // seek whose whole contract is landing where it was asked to. The terminal Exit/Signal
        // pairs cannot reach this: they report through the `Exited` arm above.
        if self.idx != n {
            return Err(Divergence { landmark: self.idx, pc: self.position(), detail: format!(
                "landmark {n} is not a resumable position: it falls inside a two-event landmark \
                 pair (a caught raise's syscall and its delivery are written at one stop); the \
                 session is now at {}", self.idx) });
        }
        Ok(())
    }

    /// The current landmark index (how many trace events have been consumed).
    pub fn landmark(&self) -> usize { self.idx }
    /// M15: which guest thread is running at this position. The thread is a DERIVED property of
    /// `(N, K)` — a switch happens only at a clean stop boundary between windows — so this is a
    /// query about the current position, not a coordinate the caller supplies.
    ///
    /// Reads what the box already computes. The schedule is a pure function of the guest's own
    /// syscall sequence (M14), recomputed identically on replay, so this needs nothing recorded.
    ///
    /// **At a landmark boundary `(N, 0)` this names the thread that ISSUED landmark `N`'s syscall,
    /// which after a BLOCKING one (`__ulock_wait`, `bsdthread_terminate`) is the thread that just
    /// blocked or exited — not the one that will retire the next instruction.** `Box_::run()` and
    /// `step()` switch on ENTRY, after the dispatch arm has marked the thread `Blocked`/`Exited`,
    /// which is exactly where M15's R1 invariant is pinned; so from `K >= 1` onward this names the
    /// running thread, and only the `K == 0` boundary shows the outgoing one. That is a
    /// definitional choice, not a lag: it keeps this in agreement with `Event::Syscall.thread` for
    /// that same landmark, which is what the divergence oracle compares against. A caller
    /// rendering it (`where`, `threads`) will therefore mark a `Blocked` thread as current at such
    /// a boundary — correct, and surprising the first time you see it.
    pub fn current_thread(&self) -> u32 { self.b.threads().current() as u32 }
    /// M15: every thread the guest has created, in stable index order. Exited threads STAY in the
    /// table (a `join` may arrive after the exit), so they appear here too — that is information the
    /// debugger's user wants, not noise.
    pub fn thread_summaries(&self) -> Vec<ThreadSummary> {
        let t = self.b.threads();
        (0..t.len()).map(|i| ThreadSummary {
            tid: i as u32, state: t.state_of(i), is_current: i == t.current(),
        }).collect()
    }
    /// M15: a specific thread's registers, including a BLOCKED one — impossible before this
    /// milestone. `None` for an out-of-range id, which the CLI turns into a usage error.
    pub fn dbg_regs_of(&self, tid: usize) -> Option<String> { self.b.dbg_regs_of(tid) }
    /// M16 Task 1: `Box_::kport_of`, for the R1 measurement gate. Test-only, like `dbg_regs_of`.
    #[doc(hidden)]
    pub fn dbg_kport_of(&self, tid: usize) -> Option<u32> { self.b.kport_of(tid) }
    /// M16 Task 4: `Box_::thread_of_port`, for the port->tid resolution gate. Test-only, like
    /// `dbg_kport_of`.
    #[doc(hidden)]
    pub fn dbg_thread_of_port(&self, port: u32) -> usize { self.b.thread_of_port(port) }

    /// Fast-follow: the fallible form, so a test can observe the FAILURE without the process
    /// aborting under it. `dbg_thread_of_port` above still panics, matching record.
    pub fn dbg_try_thread_of_port(&self, port: u32) -> Result<usize, String> {
        self.b.try_thread_of_port(port)
    }
    /// How many threads the guest has created so far. Test-only.
    #[doc(hidden)]
    pub fn b_thread_count(&self) -> usize { self.b.threads().len() }
    /// Peek the NEXT trace event to be consumed: its `(num, args)` when it is a `Syscall`, else
    /// `None` (a `Snapshot`/`Exit`, or past the last event). Read-only — does NOT advance the guest.
    /// Lets a discovery session recognize a target syscall landmark (e.g. `write(1, …)`) before
    /// choosing to `advance()` past it, without executing further.
    pub fn peek_syscall(&self) -> Option<(u64, [u64; 8])> {
        match self.events.get(self.idx) {
            Some(Event::Syscall { num, args, .. }) => Some((*num, *args)),
            _ => None,
        }
    }
    /// The landmark anchor: ELR_EL1 at a syscall trap (the last trap's return address), matching
    /// `Box_::position()`. Coincides with `pc()` only at a landmark boundary (K=0).
    pub fn position(&self) -> u64 { self.b.position() }
    /// The live instruction pointer (reg PC) — the true position at an arbitrary (N, K) coordinate,
    /// matching `Box_::pc()`. This differs from `position()` (ELR_EL1, a syscall's return address):
    /// they coincide only at a landmark boundary (K=0); mid-window, at the initial snapshot, and at a
    /// hardware breakpoint hit, only reg PC names where the guest actually is. The M3 debugger reports this.
    pub fn pc(&self) -> u64 { self.b.pc() }
    /// Arm one hardware instruction breakpoint per address (one DBGBVR slot each) so a mid-window PC
    /// match surfaces from `advance()` as `Advance::Break`. The 6-slot hardware limit is enforced
    /// upstream by the debugger's `break` command, so this asserts rather than silently truncating.
    /// Cleared by `clear_breakpoints` or by dropping the session.
    pub fn arm_breakpoints(&mut self, addrs: &[u64]) {
        assert!(addrs.len() <= 6, "break command enforces the limit");
        for (slot, &va) in addrs.iter().enumerate() {
            self.b.arm_hw_breakpoint(slot, va);
        }
    }
    /// Disarm every hardware breakpoint (return the vcpu to a clean, single-step-safe state).
    pub fn clear_breakpoints(&mut self) { self.b.clear_hw_breakpoints(); }
    /// Arm one hardware write-watchpoint per (va, len) range (one DBGWVR slot each) so a watched
    /// guest store surfaces from `advance()` as `Advance::Watch`. The 4-slot hardware limit is
    /// enforced upstream by the debugger's `watch` command. Cleared by `clear_watchpoints` or drop.
    pub fn arm_watchpoints(&mut self, ranges: &[(u64, u64)]) {
        assert!(ranges.len() <= 4, "watch command enforces the limit");
        for (slot, &(va, len)) in ranges.iter().enumerate() {
            self.b.arm_hw_watchpoint(slot, va, len);
        }
    }
    /// Disarm every hardware watchpoint (single-step-safe again).
    pub fn clear_watchpoints(&mut self) { self.b.clear_hw_watchpoints(); }
    /// The fault/watch address of the last `Stop::Other` (for a watchpoint hit: the accessed VA).
    pub fn far(&self) -> u64 { self.b.fault_ipa() }
    /// Bring-up register dump (x0..x30, SP, PC, ELR, FAR).
    pub fn dbg_regs(&self) -> String { self.b.dbg_regs() }
    /// Read `len` bytes of guest memory at `va`, or None if the full `[va, va+len)` span is not
    /// mapped inside one backing (all-or-nothing — never a partial or clamped read).
    pub fn read_mem(&self, va: u64, len: usize) -> Option<Vec<u8>> {
        self.b.read_guest_checked(va, len)
    }
    /// Capture the current registers + full guest memory (for the determinism oracle / debugger).
    pub fn snapshot(&mut self) -> (retrace_trace::Regs, Vec<retrace_trace::Region>) {
        match self.b.snapshot() {
            Event::Snapshot { regs, mem } => (regs, mem),
            _ => unreachable!("Box_::snapshot always returns Event::Snapshot"),
        }
    }
    /// Byte-compare current guest memory against `expect`; Some(detail) on the first divergence.
    pub fn diff_memory(&self, expect: &[retrace_trace::Region]) -> Option<String> {
        self.b.diff_memory(expect)
    }

    /// Single-step exactly `k` instructions into the current landmark's window. Deterministic replay
    /// faults inside the window (a cache-window page-in or a reservation commit) are handled and the
    /// instruction re-stepped, counting zero steps — identical to `advance`'s fault handling. Errs,
    /// NAMING the window length, if the window-ending trap arrives before `k` retire (no silent
    /// clamp; the length substring is a UX contract the reverse-stepi relies on). The session is
    /// spent on Err — the guest is parked mid-window with `k` unsatisfied.
    pub fn step_insns(&mut self, k: u64) -> Result<(), String> {
        for done in 0..k {
            loop {
                match self.b.step() {
                    Stop::Step => break,
                    Stop::Other { esr } => {
                        if self.b.page_in_cache(self.b.fault_ipa()) { continue; }
                        if self.b.commit_reserved_page(self.b.fault_ipa()) { continue; }
                        return Err(format!("fault during step {done}/{k}: {}", self.b.describe_stop(esr)));
                    }
                    // The window ends after exactly `done` instructions — name that length; the
                    // window-ending trap is left unconsumed (the guest stays parked at it).
                    Stop::Syscall { .. } => return Err(format!(
                        "window {} ends after {done} instruction(s); cannot step {k}", self.idx)),
                    // step_insns: stepping INTO the crash — the instruction never retires; the
                    // session stays parked immediately before it.
                    Stop::Fault { pc, esr: _, far } => return Err(format!(
                        "guest crashed at step {done}/{k}: pc={pc:#x} far={far:#x}")),
                }
            }
        }
        Ok(())
    }

    /// Single-step to the window-ending trap, returning the window length (instructions retired
    /// before the trap). Faults inside the window are paged in / committed and re-stepped, exactly
    /// as `step_insns` does. Deterministic per (trace, landmark). The session is spent (parked at
    /// the trap).
    pub fn window_len_here(&mut self) -> Result<u64, String> {
        let mut n = 0u64;
        loop {
            match self.b.step() {
                Stop::Step => n += 1,
                Stop::Other { esr } => {
                    if self.b.page_in_cache(self.b.fault_ipa()) { continue; }
                    if self.b.commit_reserved_page(self.b.fault_ipa()) { continue; }
                    return Err(format!("fault at step {n}: {}", self.b.describe_stop(esr)));
                }
                Stop::Syscall { .. } => return Ok(n),
                // window_len_here: the crash ENDS the final window — its length is the count of
                // retired instructions before the fault (the fault itself never retires).
                Stop::Fault { .. } => return Ok(n),
            }
        }
    }

    /// Re-open a session's trace-level constants (`events`, `truncated`) and restore a `Box_` +
    /// position from a previously captured checkpoint, skipping the landmark-0 replay a cold `open`
    /// would pay. `stdout` starts empty — no checkpoint consumer reads it.
    pub fn from_checkpoint(trace_path: &Path, checkpoint: &SessionCheckpoint) -> Result<Self, String> {
        let (events, truncated) = retrace_trace::Reader::open_checked(trace_path)
            .map_err(|e| format!("cannot open trace: {e}"))?;
        let b = Box_::from_checkpoint(&checkpoint.box_state);
        Ok(ReplaySession { b, events, idx: checkpoint.idx, stdout: Vec::new(),
                            guest_task_port: checkpoint.guest_task_port, truncated })
    }

    /// Capture this session's current position as a `SessionCheckpoint`.
    pub fn checkpoint(&self) -> SessionCheckpoint {
        SessionCheckpoint { box_state: self.b.checkpoint(), idx: self.idx,
                            guest_task_port: self.guest_task_port }
    }

    /// FP/SIMD register dump — the checkpoint determinism tests' FP half of `dbg_regs()`.
    pub fn dbg_fp_regs(&self) -> String { self.b.dbg_fp_regs() }
}

/// An in-memory-only capture of a `ReplaySession`'s complete position-varying state: `Box_`'s full
/// internal state (`BoxState`) plus the two `ReplaySession` fields that vary by position (`idx`,
/// `guest_task_port`). `stdout` is deliberately not captured — nothing that reads a
/// checkpoint-restored session inspects it.
#[derive(Clone)]
pub struct SessionCheckpoint {
    box_state: retrace_box::BoxState,
    idx: usize,
    guest_task_port: Option<u64>,
}

impl SessionCheckpoint {
    fn approx_bytes(&self) -> usize { self.box_state.mem.iter().map(|r| r.bytes.len()).sum() }
}

/// A session-scoped, single-trace cache of `SessionCheckpoint`s keyed by trace-execution-order
/// position `(landmark N, step K)`. Purely a performance layer for `checkpointed_seek` — never
/// consulted for correctness. Only positions expensive to REACH (single-step count >=
/// `cost_gate_steps`) get stored, evicting the least-recently-used entry first once `byte_budget`
/// would be exceeded. No invalidation: a checkpoint's validity depends only on (trace file,
/// position), both fixed for this cache's lifetime — entries are only ever evicted for space.
pub struct CheckpointCache {
    entries: std::collections::BTreeMap<(usize, u64), Rc<SessionCheckpoint>>,
    recency: Vec<(usize, u64)>, // oldest-used first; touched entries move to the back
    byte_budget: usize,
    used_bytes: usize,
    cost_gate_steps: u64,
    total_single_steps: u64,
    window_lens: std::collections::BTreeMap<usize, u64>, // landmark N -> window length (fixed per trace)
    window_probe_steps: u64,
}

impl CheckpointCache {
    pub fn new(byte_budget: usize, cost_gate_steps: u64) -> Self {
        CheckpointCache { entries: std::collections::BTreeMap::new(), recency: Vec::new(),
                          byte_budget, used_bytes: 0, cost_gate_steps, total_single_steps: 0,
                          window_lens: std::collections::BTreeMap::new(), window_probe_steps: 0 }
    }

    /// Total single-steps ever paid across every `checkpointed_seek` call against this cache — the
    /// cost-gating input, and the deterministic proxy the test suite uses to prove acceleration.
    pub fn total_single_steps(&self) -> u64 { self.total_single_steps }
    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn used_bytes(&self) -> usize { self.used_bytes }

    /// Window length of landmark `n`, memoized: a window's length is a fixed, deterministic
    /// property of (trace, landmark), so it is measured at most once per cache lifetime. The
    /// measuring probe seeks via `checkpointed_seek` (benefiting from, and feeding, the position
    /// cache) and then single-steps the full window once. The caller must hold NO live
    /// `ReplaySession` (one VM per process). `window_probe_steps` counts the steps paid by these
    /// probes — the deterministic proxy the tests use; deliberately separate from
    /// `total_single_steps`, which counts only position-seek steps.
    pub fn window_len(&mut self, trace_path: &Path, n: usize) -> Result<u64, String> {
        if let Some(&len) = self.window_lens.get(&n) { return Ok(len); }
        let mut probe = checkpointed_seek(trace_path, self, n, 0)?;
        let len = probe.window_len_here()?;
        drop(probe); // free the VM before returning control to a caller that will open a session
        self.window_probe_steps += len;
        self.window_lens.insert(n, len);
        Ok(len)
    }

    /// Total single-steps ever paid by `window_len`'s discovery probes against this cache.
    pub fn window_probe_steps(&self) -> u64 { self.window_probe_steps }

    fn touch(&mut self, key: (usize, u64)) {
        self.recency.retain(|&k| k != key);
        self.recency.push(key);
    }

    /// The best cached position at or before `(n, k)` in execution order, if any — shared out via
    /// `Rc` (no memory copy) and marked most-recently-used.
    fn best_at_or_before(&mut self, n: usize, k: u64) -> Option<((usize, u64), Rc<SessionCheckpoint>)> {
        let key = *self.entries.range(..=(n, k)).next_back()?.0;
        self.touch(key);
        Some((key, Rc::clone(&self.entries[&key])))
    }

    /// Record `steps_paid` toward the running total, and — only if it clears the cost gate — store
    /// `checkpoint` at `(n, k)`, evicting least-recently-used entries first while over budget.
    fn record_and_maybe_insert(&mut self, n: usize, k: u64, steps_paid: u64, checkpoint: Rc<SessionCheckpoint>) {
        self.total_single_steps += steps_paid;
        if steps_paid < self.cost_gate_steps { return; }
        let bytes = checkpoint.approx_bytes();
        while self.used_bytes + bytes > self.byte_budget && !self.recency.is_empty() {
            let oldest = self.recency.remove(0);
            if let Some(evicted) = self.entries.remove(&oldest) { self.used_bytes -= evicted.approx_bytes(); }
        }
        if bytes > self.byte_budget { return; } // a single entry over budget is never cached
        if let Some(old) = self.entries.insert((n, k), checkpoint) {
            self.used_bytes -= old.approx_bytes(); // same-key overwrite: retire the old entry's bytes
        }
        self.used_bytes += bytes;
        self.touch((n, k));
    }
}

/// Same contract as `seek` — the cache is purely an accelerator; a miss falls back to the cold path,
/// so no new failure mode reaches callers. On a same-window hit, resumes with only the remaining
/// `step_insns`; on an earlier-window hit, resumes with `advance_to_landmark` then `step_insns`; on
/// a miss, seeks cold. After landing, the single-step count actually paid this call (landmark
/// replay is native-speed and deliberately excluded from the cost gate) is recorded, and the
/// position stored as a fresh checkpoint if that count clears `cache`'s cost gate.
pub fn checkpointed_seek(trace_path: &Path, cache: &mut CheckpointCache, n: usize, k: u64)
    -> Result<ReplaySession, String> {
    let hit = cache.best_at_or_before(n, k);
    let (s, steps_paid) = match hit {
        Some(((n0, k0), checkpoint)) if n0 == n => {
            let mut s = ReplaySession::from_checkpoint(trace_path, &checkpoint)?;
            s.step_insns(k - k0)?;
            (s, k - k0)
        }
        Some((_, checkpoint)) => {
            let mut s = ReplaySession::from_checkpoint(trace_path, &checkpoint)?;
            s.advance_to_landmark(n).map_err(|d| format!("seek to landmark {n}: {}", d.detail))?;
            s.step_insns(k)?;
            (s, k)
        }
        None => {
            let mut s = ReplaySession::open(trace_path)?;
            s.advance_to_landmark(n).map_err(|d| format!("seek to landmark {n}: {}", d.detail))?;
            s.step_insns(k)?;
            (s, k)
        }
    };
    cache.record_and_maybe_insert(n, k, steps_paid, Rc::new(s.checkpoint()));
    Ok(s)
}

/// A fresh session positioned at the M3 coordinate P = (landmark `n`, step `k`): restore from the
/// snapshot, advance to landmark `n` (the divergence oracle verifies every trap on the way), then
/// single-step `k` instructions into its window. One VM per process, so the caller must drop this
/// session before opening another. Errs (no partial session) if the seek can't be satisfied.
pub fn seek(trace_path: &Path, n: usize, k: u64) -> Result<ReplaySession, String> {
    let mut s = ReplaySession::open(trace_path)?;
    s.advance_to_landmark(n).map_err(|d| format!("seek to landmark {n}: {}", d.detail))?;
    s.step_insns(k)?;
    Ok(s)
}

pub fn replay(trace_path: &Path) -> Result<ReplayReport, Divergence> {
    let mut s = ReplaySession::open(trace_path)
        .map_err(|e| Divergence { landmark: 0, pc: 0, detail: e })?;
    loop {
        if let Advance::Exited(report) = s.advance()? { return Ok(report); }
    }
}
