use std::path::Path;
use retrace_box::{Box_, Stop};
use retrace_trace::{Writer, Event};
use retrace_arch::{SYS_WRITE, SYS_EXIT};

pub struct RecordSummary { pub stdout: Vec<u8>, pub exit_code: u64, pub events: usize }

pub fn record(loaded: &retrace_guest::Loaded, trace_path: &Path) -> Result<RecordSummary, String> {
    let mut b = Box_::load(loaded);
    let mut w = Writer::create(trace_path).map_err(|e| format!("create trace: {e}"))?;
    let mut count = 0usize;
    w.append(&b.snapshot()).map_err(|e| format!("append snapshot: {e}"))?; count += 1;

    let mut stdout = Vec::new();
    let exit_code;
    loop {
        match b.run() {
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
            // deterministically-addressed tracked backing and returns its IPA.
            Stop::Syscall { num, args } if num == retrace_arch::SYS_MMAP => {
                let ipa = b.guest_mmap(args[1]);       // args[1] = length
                w.append(&Event::Syscall { num, args, ret: ipa, err: false, writes: vec![] }).map_err(|e| format!("append mmap: {e}"))?; count += 1;
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
            // Every other syscall goes through the general memory-diff engine (forwarded once).
            Stop::Syscall { num, args } => {
                let (ret, err, writes) = b.forward_and_diff(num, args);
                w.append(&Event::Syscall { num, args, ret, err, writes }).map_err(|e| format!("append syscall: {e}"))?; count += 1;
                b.set_x0_err_and_return(ret, err);
            }
            Stop::Other { esr } => return Err(format!("M1 unexpected non-syscall exit esr=0x{esr:x}")),
        }
    }
    Ok(RecordSummary { stdout, exit_code, events: count })
}

#[derive(Debug)]
pub struct ReplayReport { pub stdout: Vec<u8>, pub exit_code: u64 }
#[derive(Debug)]
pub struct Divergence { pub landmark: usize, pub pc: u64, pub detail: String }

pub fn replay(trace_path: &Path) -> Result<ReplayReport, Divergence> {
    // open_checked keeps every whole, CRC-valid record and drops a torn/corrupt tail; a
    // missing/unreadable file, an empty/torn trace, or a lost leading Snapshot each become
    // a named Divergence (exit 3) rather than a panic.
    let (events, truncated) = retrace_trace::Reader::open_checked(trace_path)
        .map_err(|e| Divergence { landmark: 0, pc: 0, detail: format!("cannot open trace: {e}") })?;
    if events.is_empty() {
        return Err(Divergence { landmark: 0, pc: 0, detail: "empty/torn trace: no readable records".into() });
    }
    let (regs, mem) = match events.first() {
        Some(Event::Snapshot { regs, mem }) => (regs.clone(), mem.clone()),
        _ => return Err(Divergence { landmark: 0, pc: 0, detail: "trace missing leading Snapshot".into() }),
    };
    // Rebuild the guest from the snapshot's exact regions (includes stack + trampoline);
    // restore maps only those regions and re-establishes fixed sysregs + captured registers.
    let mut b = Box_::restore(&mem, &regs);

    let mut stdout = Vec::new();
    let mut idx = 1usize; // events[0] is the initial snapshot
    loop {
        match b.run() {
            Stop::Syscall { num, args } => {
                let pc = b.position();
                if num == SYS_EXIT {
                    // Verify Exit, then the final-memory landmark.
                    match events.get(idx) {
                        Some(Event::Exit { code }) => {
                            if args[0] != *code {
                                return Err(Divergence { landmark: idx, pc,
                                    detail: format!("exit code mismatch: live {} != recorded {}", args[0], code) });
                            }
                            match events.get(idx + 1) {
                                Some(Event::Snapshot { mem: final_mem, .. }) => {
                                    if let Some(d) = b.diff_memory(final_mem) {
                                        return Err(Divergence { landmark: idx + 1, pc, detail: d });
                                    }
                                    return Ok(ReplayReport { stdout, exit_code: *code });
                                }
                                other => return Err(Divergence { landmark: idx + 1, pc,
                                    detail: format!("expected final memory Snapshot, got {other:?}") }),
                            }
                        }
                        other => return Err(Divergence { landmark: idx, pc,
                            detail: format!("expected recorded Exit, got {other:?}") }),
                    }
                }
                match events.get(idx) {
                    Some(Event::Syscall { num: rn, args: ra, ret, err, writes }) => {
                        if num != *rn || args != *ra {
                            return Err(Divergence { landmark: idx, pc,
                                detail: format!("syscall mismatch: live (num={num}, args={args:?}) != recorded (num={rn}, args={ra:?})") });
                        }
                        // Mirror fd-1/2 write output (the buffer is already filled by prior applied reads).
                        if num == SYS_WRITE && (args[0] == 1 || args[0] == 2) {
                            stdout.extend_from_slice(&b.read_guest(args[1], args[2] as usize));
                        }
                        // mmap: recreate the mapping deterministically (the guest reproduces its own
                        // stores by re-execution). The IPA must match the recording exactly.
                        if num == retrace_arch::SYS_MMAP {
                            let ipa = b.guest_mmap(args[1]);
                            if ipa != *ret {
                                return Err(Divergence { landmark: idx, pc,
                                    detail: format!("mmap ipa mismatch: replay {ipa:#x} != recorded {ret:#x}") });
                            }
                            b.set_x0_err_and_return(*ret, false);
                            idx += 1;
                            continue;
                        }
                        // munmap/mprotect (debt #2): honor them for real on replay too, so a later
                        // mmap in the trace can reuse the address exactly like it did on record.
                        if num == retrace_arch::SYS_MUNMAP {
                            b.guest_munmap(args[0], args[1]);
                            b.set_x0_err_and_return(0, false);
                            idx += 1;
                            continue;
                        }
                        if num == retrace_arch::SYS_MPROTECT {
                            b.guest_mprotect(args[0], args[1], args[2]);
                            b.set_x0_err_and_return(0, false);
                            idx += 1;
                            continue;
                        }
                        // Apply recorded kernel writes + feed ret; NO real syscall executes.
                        b.apply_and_return(*ret, *err, writes);
                        idx += 1;
                    }
                    other => return Err(Divergence { landmark: idx, pc,
                        detail: format!("expected recorded syscall, got {other:?} (truncated={truncated})") }),
                }
            }
            Stop::Other { esr } => {
                return Err(Divergence { landmark: idx, pc: b.pc(), detail: format!("unexpected non-syscall exit esr=0x{esr:x}") });
            }
        }
    }
}
