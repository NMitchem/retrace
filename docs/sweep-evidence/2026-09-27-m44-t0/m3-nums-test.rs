// M44 t0 M3 halt rows + M4 corpus reach: the throwaway test that produced t0-m3-nums.log. It ran as
// crates/retrace-trace/tests/t0_m3_nums.rs (deleted after the run, never committed there) with
// `export T0_TRACES=<$L/t0-m3-traces.txt>` then
// `cargo test -p retrace-trace --test t0_m3_nums -- --test-threads=1 --nocapture`.
// Reads every trace path listed (one per line) and prints, per trace, the count of each syscall
// number of interest among its Event::Syscall landmarks, with the first occurrence's landmark
// index, thread, ret and err.
use retrace_trace::{Event, Reader};

#[test]
fn t0_m3_nums() {
    let list = std::fs::read_to_string(std::env::var("T0_TRACES").expect("T0_TRACES")).unwrap();
    const NUMS: &[i64] = &[461, 463, 464, 345, 346, 10, 128, 244, 59, 374, 362, 409, 410, 422, 542, 543, 98, 111, 330, 540, 541];
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
        println!("T0M3 TRACE {path} events={} syscalls={syscalls} truncated={truncated} last={last}", ev.len());
        for &n in NUMS {
            let hits: Vec<usize> = ev.iter().enumerate().filter(|(_, e)| matches!(e, Event::Syscall { num, .. } if *num as i64 == n)).map(|(i, _)| i).collect();
            if let Some(&i) = hits.first() {
                if let Event::Syscall { args, ret, err, thread, .. } = &ev[i] {
                    println!("T0M3   num={n} count={} first=landmark {i} thread={thread} ret={ret:#x} err={err} x0={:#x} x1={:#x} x2={:#x} x3={:#x}",
                        hits.len(), args[0], args[1], args[2], args[3]);
                }
            } else {
                println!("T0M3   num={n} count=0");
            }
        }
    }
}
