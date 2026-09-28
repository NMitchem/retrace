// M44 Task 4 — throwaway landmark reader, adapted from t0's t0-m3-nums-test.rs (see
// .superpowers/sdd/2026-09-27-retrace-m44-owed/t0-m3-nums-test.rs). Reads the t4-controller's kept
// traces (docs/sweep-evidence, scratchpad copies before commit) and prints, per trace, the first
// landmark and count of each M44-relevant syscall number, plus the trace's terminal event. Run as
// `crates/retrace-trace/tests/t4_m44_nums.rs` and DELETED before committing — nothing of this file
// runs in the repo; this copy is evidence only.
use retrace_trace::{Event, Reader};

#[test]
fn t4_m44_nums() {
    let list = std::fs::read_to_string(std::env::var("T4_TRACES").expect("T4_TRACES")).unwrap();
    const NUMS: &[i64] = &[461, 464, 345, 10, 128, 244, 374];
    for path in list.lines().filter(|l| !l.trim().is_empty()) {
        let (ev, truncated) = Reader::open_checked(path).unwrap();
        let syscalls = ev.iter().filter(|e| matches!(e, Event::Syscall { .. })).count();
        let last = match ev.last() {
            Some(Event::Snapshot { .. }) => "Snapshot".to_string(),
            Some(Event::Syscall { num, .. }) => format!("Syscall({})", *num as i64),
            Some(Event::Exit { code, .. }) => format!("Exit({code})"),
            Some(e) => format!("{:?}", std::mem::discriminant(e)),
            None => "none".to_string(),
        };
        println!("T4M44 TRACE {path} events={} syscalls={syscalls} truncated={truncated} last={last}", ev.len());
        for &n in NUMS {
            let hits: Vec<usize> = ev.iter().enumerate().filter(|(_, e)| matches!(e, Event::Syscall { num, .. } if *num as i64 == n)).map(|(i, _)| i).collect();
            if let Some(&i) = hits.first() {
                if let Event::Syscall { args, ret, err, thread, .. } = &ev[i] {
                    println!("T4M44   num={n} count={} first=landmark {i} thread={thread} ret={ret:#x} err={err} x0={:#x} x1={:#x} x2={:#x} x3={:#x}",
                        hits.len(), args[0], args[1], args[2], args[3]);
                }
            } else {
                println!("T4M44   num={n} count=0");
            }
        }
    }
}
