pub mod machmsg;

use std::path::Path;
use retrace_box::{Box_, Stop};
use retrace_trace::{Writer, Event, Region};
use retrace_arch::{SYS_WRITE, SYS_EXIT};

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

pub struct RecordSummary { pub stdout: Vec<u8>, pub exit_code: u64, pub events: usize }

pub fn record(loaded: &retrace_guest::Loaded, trace_path: &Path) -> Result<RecordSummary, String> {
    record_box(Box_::load(loaded), trace_path)
}

/// Dynamic record: run the exe through real dyld (mapped via `load_dynamic`) and record. Same
/// record loop as the static path — dyld's syscalls/mach-traps flow through the shared engine.
pub fn record_dynamic(exe: &retrace_guest::Loaded, dyld: &retrace_guest::Loaded, argv0: &str,
                      trace_path: &Path) -> Result<RecordSummary, String> {
    record_box(Box_::load_dynamic(exe, dyld, argv0), trace_path)
}

fn record_box(mut b: Box_, trace_path: &Path) -> Result<RecordSummary, String> {
    let mut w = Writer::create(trace_path).map_err(|e| format!("create trace: {e}"))?;
    let mut count = 0usize;
    w.append(&b.snapshot()).map_err(|e| format!("append snapshot: {e}"))?; count += 1;

    let mut stdout = Vec::new();
    let exit_code;
    // Bring-up diagnostic (RETRACE_TRACE=1): log every trap the record loop dispatches, so a
    // forwarded syscall/mach-trap that misbehaves is identifiable from the last line printed.
    let trace_log = std::env::var_os("RETRACE_TRACE").is_some();
    // The guest's task-port NAME (the result of task_self_trap −28, still host-forwarded this
    // milestone): machmsg routing needs it to recognize task-destined kernel RPCs. Learned
    // identically on record (forwarded result) and replay (recorded result).
    let mut guest_task_port: Option<u64> = None;
    loop {
        let stop = b.run();
        if trace_log {
            if let Stop::Syscall { num, args } = &stop {
                eprintln!("[trap] num={} (0x{:x}) pc={:#x} args=[{:#x},{:#x},{:#x},{:#x},{:#x},{:#x}]",
                    *num as i64, num, b.position(), args[0], args[1], args[2], args[3], args[4], args[5]);
                // Echo dyld's fd-1/2 diagnostics so a fatal error message is visible.
                if *num == SYS_WRITE && (args[0] == 1 || args[0] == 2) {
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
        }
        match stop {
            Stop::Syscall { num, args } if num == SYS_EXIT => {
                let final_snap = b.snapshot();          // final-memory landmark
                w.append(&Event::Exit { code: args[0] }).map_err(|e| format!("append exit: {e}"))?; count += 1;
                w.append(&final_snap).map_err(|e| format!("append final snapshot: {e}"))?; count += 1;
                exit_code = args[0];
                break;
            }
            // Console writes are mirrored + faked (NOT forwarded) so record doesn't emit to the
            // record process's real stdout AND double the mirror; replay reproduces from the mirror.
            Stop::Syscall { num, args } if num == SYS_WRITE && (args[0] == 1 || args[0] == 2) => {
                stdout.extend_from_slice(&b.read_guest(args[1], args[2] as usize));
                let ret = args[2];
                w.append(&Event::Syscall { num, args, ret, err: false, writes: vec![] }).map_err(|e| format!("append write: {e}"))?; count += 1;
                b.set_x0_err_and_return(ret, false);
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
                let ipa = b.guest_mmap(args[1]);       // args[1] = length
                w.append(&Event::Syscall { num, args, ret: ipa, err: false, writes: vec![] }).map_err(|e| format!("append mmap: {e}"))?; count += 1;
                b.set_x0_err_and_return(ipa, false);
            }
            Stop::Syscall { num, args } if num == retrace_arch::SYS_MMAP => {
                let (ipa, writes) = b.guest_mmap_file(args[0], args[1], args[2], args[3], args[4] as i32, args[5]);
                // PROT_EXEC (0x4): promote the freshly-mapped region to RO+exec (ATTR_CODE) stage-1
                // pages so the guest can execute from it under W^X (e.g. dyld mapping the shared
                // cache's __TEXT). Done BEFORE resuming the guest, on record AND replay.
                if args[2] & 0x4 != 0 { b.set_region_exec(ipa, args[1]); }
                w.append(&Event::Syscall { num, args, ret: ipa, err: false, writes }).map_err(|e| format!("append mmap_file: {e}"))?; count += 1;
                b.set_x0_err_and_return(ipa, false);
            }
            // munmap/mprotect (debt #2): honor them for real — drop + hv_vm_unmap the backing on
            // munmap so a later mmap can reuse the address; best-effort hv_vm_protect on
            // mprotect. Neither writes guest memory itself (the guest's own subsequent stores
            // do), so they're recorded like mmap: ret=0, no writes, reproduced by re-execution.
            Stop::Syscall { num, args } if num == retrace_arch::SYS_MUNMAP => {
                b.guest_munmap(args[0], args[1]);
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![] }).map_err(|e| format!("append munmap: {e}"))?; count += 1;
                b.set_x0_err_and_return(0, false);
            }
            Stop::Syscall { num, args } if num == retrace_arch::SYS_MPROTECT => {
                b.guest_mprotect(args[0], args[1], args[2]);
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![] }).map_err(|e| format!("append mprotect: {e}"))?; count += 1;
                b.set_x0_err_and_return(0, false);
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
                    w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: writes.clone() }).map_err(|e| format!("append shared_region_check: {e}"))?; count += 1;
                    b.apply_and_return(0, false, &writes);
                } else {
                    // dyld's deliberate error path (e.g. `shared_region_check_np((void*)-1)` to
                    // return a failure code): the kernel's copyout to the bad pointer yields EFAULT.
                    // Reproduce it deterministically — carry set, x0 = EFAULT, no writes.
                    const EFAULT: u64 = 14;
                    w.append(&Event::Syscall { num, args, ret: EFAULT, err: true, writes: vec![] }).map_err(|e| format!("append shared_region_check(bad ptr): {e}"))?; count += 1;
                    b.set_x0_err_and_return(EFAULT, true);
                }
            }
            // shared_region_map_and_slide_2_np (#536): the kernel cache-mapping syscall. We do NOT
            // map here — the cache is lazily demand-paged (page_in_cache) on stage-2 faults. Install
            // the pager and return success. Installed on BOTH record and replay so both page
            // identical bytes; no cache bytes are ever written to the trace.
            Stop::Syscall { num, args } if num == retrace_arch::SYS_SHARED_REGION_MAP_AND_SLIDE_2_NP => {
                b.install_cache_pager();
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![] }).map_err(|e| format!("append shared_region_map: {e}"))?; count += 1;
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
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: writes.clone() }).map_err(|e| format!("append mac_syscall: {e}"))?; count += 1;
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
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: writes.clone() }).map_err(|e| format!("append mach_vm_map: {e}"))?; count += 1;
                b.apply_and_return(0, false, &writes);
            }
            // mach_vm_deallocate: free guest memory (drop the backing + stage-2 unmap).
            Stop::Syscall { num, args } if num == MACH_VM_DEALLOCATE => {
                b.guest_munmap(args[1], args[2]);
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![] }).map_err(|e| format!("append mach_vm_dealloc: {e}"))?; count += 1;
                b.set_x0_err_and_return(0, false);
            }
            // mach_vm_protect: no-op success. Stage-2 stays RWX; stage-1 W^X is already correct, so
            // a guest protect changes nothing we model — returning KERN_SUCCESS keeps dyld happy.
            Stop::Syscall { num, args } if num == MACH_VM_PROTECT => {
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![] }).map_err(|e| format!("append mach_vm_protect: {e}"))?; count += 1;
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
                            err: false, writes: writes.clone() })
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
                            err: false, writes: writes.clone() })
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
                            err: false, writes: writes.clone() })
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
                            err: false, writes: writes.clone() })
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
                        w.append(&Event::Syscall { num, args, ret, err, writes })
                            .map_err(|e| format!("append mach_msg2 fwd: {e}"))?; count += 1;
                        b.set_x0_err_and_return(ret, err);
                    }
                    machmsg::Route::Unsupported(why) => {
                        if trace_log { eprintln!("[regs]\n{}\n[bt]\n{}", b.dbg_regs(), b.dbg_backtrace(24)); }
                        return Err(format!("unsupported mach_msg2 at pc {:#x}: {why}", b.position()));
                    }
                }
            }
            // Mach traps arrive as `svc #0x80` with a NEGATIVE trap number in x16. They forward +
            // memory-diff exactly like a BSD syscall (a negative x16 is a valid mach-trap selector
            // to the kernel; the reply is either in x0 — captured as `ret` — or written into a
            // guest message buffer — captured as `writes`). Special cases that hand back fresh
            // kernel state the diff can't reproduce (ports mapped into the guest, allocations that
            // must land in guest IPA space) are added here as they are discovered.
            Stop::Syscall { num, args } if (num as i64) < 0 => {
                let (ret, err, writes) = b.forward_and_diff(num, args);
                // Learn the guest's task-port name from task_self_trap (−28) so machmsg routing can
                // recognize task-destined kernel RPCs. Mirrored on replay from the recorded result.
                if num == MACH_TASK_SELF && !err { guest_task_port = Some(ret); }
                w.append(&Event::Syscall { num, args, ret, err, writes }).map_err(|e| format!("append mach-trap: {e}"))?; count += 1;
                b.set_x0_err_and_return(ret, err);
            }
            // Every other syscall goes through the general memory-diff engine (forwarded once).
            Stop::Syscall { num, args } => {
                let (ret, err, writes) = b.forward_and_diff(num, args);
                w.append(&Event::Syscall { num, args, ret, err, writes }).map_err(|e| format!("append syscall: {e}"))?; count += 1;
                b.set_x0_err_and_return(ret, err);
            }
            // A cache-window stage-2 fault: stage/fixup/re-sign/map the page (page_in_cache) and
            // re-run. Regenerated deterministically here on record AND replay, so nothing about the
            // cache page goes into the trace. A non-cache fault (page_in_cache returns false) is a
            // real bring-up failure — decode the ESR class + faulting IPA so it names itself.
            Stop::Other { esr } => {
                if b.page_in_cache(b.fault_ipa()) { continue; }
                if b.commit_reserved_page(b.fault_ipa()) { continue; }
                if trace_log { eprintln!("[regs]\n{}\n[bt]\n{}", b.dbg_regs(), b.dbg_backtrace(24)); }
                return Err(b.describe_stop(esr));
            }
            // Stop::Step is only produced by Box_::step(); record_box drives run(), never step().
            Stop::Step => unreachable!("record_box drives run(), which never single-steps"),
        }
    }
    Ok(RecordSummary { stdout, exit_code, events: count })
}

#[derive(Debug)]
pub struct ReplayReport { pub stdout: Vec<u8>, pub exit_code: u64 }
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
/// `exit` and the final-memory landmark verified clean (`Exited`, run done); or a hardware
/// breakpoint fired mid-window (`Break`, M3 debugger only — carries nothing: the caller reads
/// `landmark()`/`cur_pc()`). `Break` is unreachable under the plain `replay()` oracle, which never
/// arms breakpoints.
pub enum Advance { Event, Exited(ReplayReport), Break }

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

    /// Consume exactly ONE trace event (returning `Advance::Event`), or drive the guest to `exit`
    /// (returning `Advance::Exited`). Non-event stops — a cache-window page-in or a reservation
    /// commit — are handled internally and the guest re-run, so `advance` returns only on event
    /// consumption or exit. Once it has returned `Advance::Exited` the run is complete; calling
    /// `advance` again is unspecified (the guest is past its final landmark) — callers must not.
    pub fn advance(&mut self) -> Result<Advance, Divergence> {
        loop {
            match self.b.run() {
                Stop::Syscall { num, args } => {
                    let pc = self.b.position();
                    if num == SYS_EXIT {
                        // Verify Exit, then the final-memory landmark.
                        match self.events.get(self.idx) {
                            Some(Event::Exit { code }) => {
                                if args[0] != *code {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!("exit code mismatch: live {} != recorded {}", args[0], code) });
                                }
                                match self.events.get(self.idx + 1) {
                                    Some(Event::Snapshot { mem: final_mem, .. }) => {
                                        if let Some(d) = self.b.diff_memory(final_mem) {
                                            return Err(Divergence { landmark: self.idx + 1, pc, detail: d });
                                        }
                                        return Ok(Advance::Exited(ReplayReport {
                                            stdout: std::mem::take(&mut self.stdout), exit_code: *code }));
                                    }
                                    other => return Err(Divergence { landmark: self.idx + 1, pc,
                                        detail: format!("expected final memory Snapshot, got {other:?}") }),
                                }
                            }
                            other => return Err(Divergence { landmark: self.idx, pc,
                                detail: format!("expected recorded Exit, got {other:?}") }),
                        }
                    }
                    match self.events.get(self.idx) {
                        Some(Event::Syscall { num: rn, args: ra, ret, err, writes }) => {
                            if num != *rn || args != *ra {
                                return Err(Divergence { landmark: self.idx, pc,
                                    detail: format!("syscall mismatch: live (num={num}, args={args:?}) != recorded (num={rn}, args={ra:?})") });
                            }
                            // Learn the guest's task-port name (mirror of record) from the recorded −28 result.
                            if num == MACH_TASK_SELF && !*err { self.guest_task_port = Some(*ret); }
                            // Mirror fd-1/2 write output (the buffer is already filled by prior applied reads).
                            if num == SYS_WRITE && (args[0] == 1 || args[0] == 2) {
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
                                self.idx += 1;
                                return Ok(Advance::Event);
                            }
                            // mmap: recreate the mapping deterministically (the guest reproduces its own
                            // stores by re-execution). The IPA must match the recording exactly.
                            if num == retrace_arch::SYS_MMAP && args[3] & MAP_ANON != 0 {
                                let ipa = self.b.guest_mmap(args[1]);
                                if ipa != *ret {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!("mmap ipa mismatch: replay {ipa:#x} != recorded {ret:#x}") });
                                }
                                self.b.set_x0_err_and_return(*ret, false);
                                self.idx += 1;
                                return Ok(Advance::Event);
                            }
                            // file-backed mmap (Task 8): anon-alloc + address identically (no file
                            // access), verify the recreated IPA equals the recorded ret (this is what
                            // makes MAP_FIXED correct on replay), then stage the recorded bytes.
                            if num == retrace_arch::SYS_MMAP {
                                let ipa = self.b.guest_mmap_replay(args[0], args[1], args[2], args[3]);
                                if ipa != *ret {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!("mmap_file ipa mismatch: replay {ipa:#x} != recorded {ret:#x}") });
                                }
                                // Same exec promotion as record: the guest executes the mmap'd code on
                                // replay too (replay runs the guest, only faking syscall results), so the
                                // exec pages must exist here as well — before the recorded bytes are staged.
                                if args[2] & 0x4 != 0 { self.b.set_region_exec(ipa, args[1]); }
                                self.b.apply_and_return(*ret, *err, writes);
                                self.idx += 1;
                                return Ok(Advance::Event);
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
                                self.idx += 1;
                                return Ok(Advance::Event);
                            }
                            if num == MACH_VM_DEALLOCATE {
                                self.b.guest_munmap(args[1], args[2]);
                                self.b.set_x0_err_and_return(*ret, *err);
                                self.idx += 1;
                                return Ok(Advance::Event);
                            }
                            if num == MACH_VM_PROTECT {
                                self.b.set_x0_err_and_return(*ret, *err);
                                self.idx += 1;
                                return Ok(Advance::Event);
                            }
                            // shared_region_check_np (#294): install the demand-pager on replay too
                            // (record installed it here), so cache faults regenerate identical pages, then
                            // apply the recorded base write via the generic path.
                            if num == retrace_arch::SYS_SHARED_REGION_CHECK_NP {
                                self.b.install_cache_pager();
                                self.b.apply_and_return(*ret, *err, writes);
                                self.idx += 1;
                                return Ok(Advance::Event);
                            }
                            // shared_region_map_and_slide_2_np (#536): install the demand-pager on
                            // replay too (record installed it here), so cache faults regenerate identical
                            // pages.
                            if num == retrace_arch::SYS_SHARED_REGION_MAP_AND_SLIDE_2_NP {
                                self.b.install_cache_pager();
                                self.b.set_x0_err_and_return(*ret, *err);
                                self.idx += 1;
                                return Ok(Advance::Event);
                            }
                            // munmap/mprotect (debt #2): honor them for real on replay too, so a later
                            // mmap in the trace can reuse the address exactly like it did on record.
                            if num == retrace_arch::SYS_MUNMAP {
                                self.b.guest_munmap(args[0], args[1]);
                                self.b.set_x0_err_and_return(0, false);
                                self.idx += 1;
                                return Ok(Advance::Event);
                            }
                            if num == retrace_arch::SYS_MPROTECT {
                                self.b.guest_mprotect(args[0], args[1], args[2]);
                                self.b.set_x0_err_and_return(0, false);
                                self.idx += 1;
                                return Ok(Advance::Event);
                            }
                            // Apply recorded kernel writes + feed ret; NO real syscall executes.
                            self.b.apply_and_return(*ret, *err, writes);
                            self.idx += 1;
                            return Ok(Advance::Event);
                        }
                        other => return Err(Divergence { landmark: self.idx, pc,
                            detail: format!("expected recorded syscall, got {other:?} (truncated={})", self.truncated) }),
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
            return Err(Divergence { landmark: self.idx, pc: self.pc(),
                detail: format!("cannot seek backward to landmark {n} (already at {})", self.idx) });
        }
        while self.idx < n {
            if let Advance::Exited(_) = self.advance()? {
                return Err(Divergence { landmark: self.idx, pc: self.pc(),
                    detail: format!("run exited before landmark {n}") });
            }
        }
        Ok(())
    }

    /// The current landmark index (how many trace events have been consumed).
    pub fn landmark(&self) -> usize { self.idx }
    /// The guest's execution position (ELR_EL1 at a syscall trap).
    pub fn pc(&self) -> u64 { self.b.position() }
    /// The live instruction pointer (reg PC) — the true position at an arbitrary (N, K) coordinate.
    /// This differs from `pc()`/`position()` (ELR_EL1, a syscall's return address): they coincide
    /// only at a landmark boundary (K=0); mid-window, at the initial snapshot, and at a hardware
    /// breakpoint hit, only reg PC names where the guest actually is. The M3 debugger reports this.
    pub fn cur_pc(&self) -> u64 { self.b.pc() }
    /// Arm up to 6 hardware instruction breakpoints (extra addresses beyond slot 5 are ignored — the
    /// caller's landmark-granular check covers them) so a mid-window PC match surfaces from
    /// `advance()` as `Advance::Break`. Cleared by `clear_breakpoints` or by dropping the session.
    pub fn arm_breakpoints(&mut self, addrs: &[u64]) {
        for (slot, &va) in addrs.iter().take(6).enumerate() {
            self.b.arm_hw_breakpoint(slot, va);
        }
    }
    /// Disarm every hardware breakpoint (return the vcpu to a clean, single-step-safe state).
    pub fn clear_breakpoints(&mut self) { self.b.clear_hw_breakpoints(); }
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
            }
        }
    }
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
