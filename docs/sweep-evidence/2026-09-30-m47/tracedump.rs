// Throwaway (M47 Task 6): print a trace's length and torn flag, every Syscall landmark whose number is
// in argv[3..] (decimal, may be negative) with its full args/ret/ret1/err/write count/thread, the last
// argv[2] events, every terminal event, and the gettimeofday (116) and fork (2) counts. TD_BYTES=n adds
// each shown write's first n bytes as printable ASCII; TD_HEX=n (fix round 1) as hex.
use retrace_trace::{Event, Reader};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path = &a[1];
    let last: usize = a.get(2).map(|s| s.parse().unwrap()).unwrap_or(6);
    let nums: Vec<i64> = a.iter().skip(3).map(|s| s.parse().unwrap()).collect();
    let (ev, torn) = Reader::open_checked(path).unwrap();
    println!("events={} torn={}", ev.len(), torn);
    let n = ev.len();
    let mut gtod = 0usize;
    let mut forks = 0usize;
    for (i, e) in ev.iter().enumerate() {
        let show = i + last >= n;
        match e {
            Event::Syscall { num, args, ret, ret1, err, writes, thread } => {
                let s = *num as i64;
                if s == 116 { gtod += 1; }
                if s == 2 { forks += 1; }
                if nums.contains(&s) || show {
                    println!("#{i} Syscall num={s} args={args:x?} ret={ret:#x} ret1={ret1:#x} err={err} writes={} thread={thread}",
                        writes.len());
                    // TD_BYTES=n: each write's ipa, length and first n bytes, printable ASCII kept.
                    if let Some(k) = std::env::var("TD_BYTES").ok().and_then(|v| v.parse::<usize>().ok()) {
                        for w in writes {
                            let t: String = w.bytes.iter().take(k)
                                .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else if b == b'\n' { '|' } else { '.' })
                                .collect();
                            println!("    write ipa={:#x} len={} {t}", w.ipa, w.bytes.len());
                        }
                    }
                    // TD_HEX=n: each write's ipa, length and first n bytes in hex.
                    if let Some(k) = std::env::var("TD_HEX").ok().and_then(|v| v.parse::<usize>().ok()) {
                        for w in writes {
                            let h: Vec<String> = w.bytes.iter().take(k).map(|b| format!("{b:02x}")).collect();
                            println!("    write ipa={:#x} len={} {}", w.ipa, w.bytes.len(), h.join(" "));
                        }
                    }
                }
            }
            Event::Snapshot { .. } if show || i == 0 => println!("#{i} Snapshot"),
            Event::Exit { code, thread } => println!("#{i} Exit code={code} thread={thread}"),
            Event::Crash { pc, esr, far, thread } => println!("#{i} Crash pc={pc:#x} esr={esr:#x} far={far:#x} thread={thread}"),
            Event::Signal { sig, pc, thread } => println!("#{i} Signal sig={sig} pc={pc:#x} thread={thread}"),
            Event::SignalDelivery { sig, thread, .. } => println!("#{i} SignalDelivery sig={sig} thread={thread}"),
            _ => {}
        }
    }
    println!("gettimeofday(116) events={gtod} fork(2) events={forks}");
}
