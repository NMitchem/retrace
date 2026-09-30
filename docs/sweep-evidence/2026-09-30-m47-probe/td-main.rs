// Throwaway probe dumper: print every event whose syscall number is in argv[2..] (decimal, may be
// negative), plus the last 4 events.
use retrace_trace::{Event, Reader};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let nums: Vec<i64> = a[2..].iter().map(|s| s.parse().unwrap()).collect();
    let (ev, torn) = Reader::open_checked(&a[1]).unwrap();
    println!("events={} torn={}", ev.len(), torn);
    let n = ev.len();
    for (i, e) in ev.iter().enumerate() {
        match e {
            Event::Syscall { num, args, ret, ret1, err, writes, thread }
                if nums.contains(&(*num as i64)) || i + 4 >= n =>
            {
                println!("#{i} num={} args={:x?} ret={ret:#x} ret1={ret1:#x} err={err} thread={thread}", *num as i64, &args[..4]);
                for w in writes { println!("    write {w:x?}"); }
            }
            Event::Exit { code, thread } => println!("#{i} Exit code={code} thread={thread}"),
            Event::Crash { pc, esr, far, thread } => println!("#{i} Crash pc={pc:#x} esr={esr:#x} far={far:#x} thread={thread}"),
            _ => {}
        }
    }
}
