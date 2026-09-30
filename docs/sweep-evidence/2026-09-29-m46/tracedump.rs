// Throwaway (M46 Task 6): print a trace's length, torn flag, every kevent_qos (374) / kevent_id (375)
// / workq_kernreturn (368) landmark, the gettimeofday (116) count, and the last six events.
use retrace_trace::{Event, Reader};

fn main() {
    let path = std::env::args().nth(1).expect("trace path");
    let (ev, torn) = Reader::open_checked(&path).unwrap();
    println!("events={} torn={}", ev.len(), torn);
    let n = ev.len();
    let mut gtod = 0usize;
    for (i, e) in ev.iter().enumerate() {
        let show = i + 6 >= n;
        match e {
            Event::Syscall { num, .. } if *num == 116 && !show => gtod += 1,
            Event::Syscall { num, args, ret, ret1, err, writes, thread }
                if matches!(*num, 116 | 368 | 374 | 375) || show =>
            {
                if *num == 116 { gtod += 1; }
                println!("#{i} Syscall num={} args={args:x?} ret={ret:#x} ret1={ret1:#x} err={err} writes={} thread={thread}",
                    *num as i64, writes.len());
            }
            Event::Snapshot { .. } if show || i == 0 => println!("#{i} Snapshot"),
            Event::Exit { code, thread } => println!("#{i} Exit code={code} thread={thread}"),
            Event::Crash { pc, esr, far, thread } => println!("#{i} Crash pc={pc:#x} esr={esr:#x} far={far:#x} thread={thread}"),
            Event::Signal { sig, pc, thread } => println!("#{i} Signal sig={sig} pc={pc:#x} thread={thread}"),
            Event::SignalDelivery { sig, thread, .. } => println!("#{i} SignalDelivery sig={sig} thread={thread}"),
            _ => {}
        }
    }
    let r7 = ev.iter().filter(|e| matches!(e, Event::Syscall { num: 116, args, err: false, .. } if args[2] != 0)).count();
    println!("gettimeofday(116) events={gtod} with-mach-time-out-param(x2!=0, R7 rewrites)={r7}");
}
