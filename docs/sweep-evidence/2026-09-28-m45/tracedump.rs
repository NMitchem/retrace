// Throwaway: print a trace's length, torn flag, every kevent_qos (374) landmark, and the last events.
use retrace_trace::{Event, Reader};

fn main() {
    let path = std::env::args().nth(1).expect("trace path");
    let (ev, torn) = Reader::open_checked(&path).unwrap();
    println!("events={} torn={}", ev.len(), torn);
    let n = ev.len();
    for (i, e) in ev.iter().enumerate() {
        let show = i + 4 >= n;
        match e {
            Event::Syscall { num, args, ret, ret1, err, writes, thread } if *num == 374 || show => {
                println!("#{i} Syscall num={num} args={args:x?} ret={ret:#x} ret1={ret1:#x} err={err} writes={} thread={thread}", writes.len());
            }
            Event::Snapshot { .. } if show || i == 0 => println!("#{i} Snapshot"),
            Event::Exit { code, thread } => println!("#{i} Exit code={code} thread={thread}"),
            Event::Crash { pc, esr, far, thread } => println!("#{i} Crash pc={pc:#x} esr={esr:#x} far={far:#x} thread={thread}"),
            Event::Signal { sig, pc, thread } => println!("#{i} Signal sig={sig} pc={pc:#x} thread={thread}"),
            Event::SignalDelivery { sig, thread, .. } => println!("#{i} SignalDelivery sig={sig} thread={thread}"),
            _ => {}
        }
    }
}
