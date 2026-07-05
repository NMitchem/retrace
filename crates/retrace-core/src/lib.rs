use std::path::Path;
use retrace_box::{Box_, Stop};
use retrace_trace::{Writer, Event, Regs, Region};
use retrace_arch::{SYS_WRITE, SYS_EXIT};
use hv_sys::{reg, sysreg};

pub struct RecordSummary { pub stdout: Vec<u8>, pub exit_code: u64, pub events: usize }

pub fn snapshot_of(b: &Box_) -> Event {
    let mut mem = Vec::new();
    for bk in &b.backings {
        let bytes = unsafe { std::slice::from_raw_parts(bk.host, bk.len) }.to_vec();
        mem.push(Region { ipa: bk.ipa, bytes });
    }
    let mut x = [0u64;31];
    for i in 0..31 { x[i] = b.vcpu.get_reg(reg::x(i as u32)).unwrap(); }
    let regs = Regs {
        x, pc: b.vcpu.get_reg(reg::PC).unwrap(),
        sp_el0: b.vcpu.get_sys(sysreg::SP_EL0).unwrap(),
        cpsr: b.vcpu.get_reg(reg::CPSR).unwrap(),
    };
    Event::Snapshot { regs, mem }
}

pub fn record(loaded: &retrace_guest::Loaded, trace_path: &Path) -> RecordSummary {
    let mut b = Box_::load(loaded);
    let mut w = Writer::create(trace_path).expect("create trace");
    let mut count = 0usize;
    w.append(&snapshot_of(&b)).unwrap(); count += 1;

    let mut stdout = Vec::new();
    let mut exit_code = 0u64;
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == SYS_WRITE => {
                // Forward write() against the 1:1 buffer, capture bytes + return value.
                let bytes = b.read_guest(args[1], args[2] as usize);
                if args[0] == 1 || args[0] == 2 { stdout.extend_from_slice(&bytes); }
                let ret = args[2]; // wrote all bytes into our capture
                w.append(&Event::Syscall { num, args, ret }).unwrap(); count += 1;
                b.set_x0_and_return(ret);
            }
            Stop::Syscall { num, args } if num == SYS_EXIT => {
                exit_code = args[0];
                w.append(&Event::Exit { code: exit_code }).unwrap(); count += 1;
                break;
            }
            Stop::Syscall { num, .. } => panic!("M0 unhandled syscall {num} (expected write/exit only)"),
            Stop::Other { esr } => panic!("M0 unexpected non-syscall exit esr=0x{esr:x}"),
        }
    }
    RecordSummary { stdout, exit_code, events: count }
}

#[derive(Debug)]
pub struct ReplayReport { pub stdout: Vec<u8>, pub exit_code: u64 }
#[derive(Debug)]
pub struct Divergence { pub landmark: usize, pub pc: u64, pub detail: String }

pub fn replay(trace_path: &Path) -> Result<ReplayReport, Divergence> {
    let events = retrace_trace::Reader::open(trace_path).expect("open trace");
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
                let pc = b.vcpu.get_sys(sysreg::ELR_EL1).unwrap();
                match events.get(idx) {
                    Some(Event::Syscall { num: rn, args: ra, ret }) => {
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
                        detail: format!("expected recorded syscall, got {other:?}") }),
                }
                if num == SYS_EXIT {
                    match events.get(idx) {
                        Some(Event::Exit { code }) => return Ok(ReplayReport { stdout, exit_code: *code }),
                        other => return Err(Divergence { landmark: idx, pc,
                            detail: format!("expected recorded Exit, got {other:?}") }),
                    }
                }
            }
            Stop::Other { esr } => {
                let pc = b.vcpu.get_reg(reg::PC).unwrap();
                return Err(Divergence { landmark: idx, pc, detail: format!("unexpected non-syscall exit esr=0x{esr:x}") });
            }
        }
    }
}
