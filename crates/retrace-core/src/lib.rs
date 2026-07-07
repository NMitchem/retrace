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
                w.append(&Event::Syscall { num, args, ret, writes: vec![] }).map_err(|e| format!("append write: {e}"))?; count += 1;
                b.set_x0_and_return(ret);
            }
            // Every other syscall goes through the general memory-diff engine (forwarded once).
            Stop::Syscall { num, args } => {
                let (ret, writes) = b.forward_and_diff(num, args);
                w.append(&Event::Syscall { num, args, ret, writes }).map_err(|e| format!("append syscall: {e}"))?; count += 1;
                b.set_x0_and_return(ret);
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
    let mut idx = 1usize; // events[0] is the snapshot
    loop {
        match b.run() {
            Stop::Syscall { num, args } => {
                let pc = b.position();
                match events.get(idx) {
                    Some(Event::Syscall { num: rn, args: ra, ret, writes: _ }) => {
                        if num != *rn || args != *ra {
                            return Err(Divergence { landmark: idx, pc,
                                detail: format!("syscall mismatch: live (num={num}, args={args:?}) != recorded (num={rn}, args={ra:?})") });
                        }
                        // ASSERT NEGATIVE SPACE: we feed the recorded result; we do NOT execute the syscall.
                        if num == SYS_WRITE && (args[0]==1 || args[0]==2) {
                            stdout.extend_from_slice(&b.read_guest(args[1], args[2] as usize));
                        }
                        b.set_x0_and_return(*ret);
                        idx += 1;
                    }
                    Some(Event::Exit { .. }) if num == SYS_EXIT => { /* fallthrough handled below */ }
                    other => return Err(Divergence { landmark: idx, pc,
                        detail: format!("expected recorded syscall, got {other:?} (trace truncated={truncated})") }),
                }
                if num == SYS_EXIT {
                    match events.get(idx) {
                        Some(Event::Exit { code }) => return Ok(ReplayReport { stdout, exit_code: *code }),
                        other => return Err(Divergence { landmark: idx, pc,
                            detail: format!("expected recorded Exit, got {other:?} (trace truncated={truncated})") }),
                    }
                }
            }
            Stop::Other { esr } => {
                let pc = b.pc();
                return Err(Divergence { landmark: idx, pc, detail: format!("unexpected non-syscall exit esr=0x{esr:x}") });
            }
        }
    }
}
