// M52 Task 4 (spec §3e, R11): the back-off spinner. Main waits on a channel for 60 seconds while a
// child yields ten times (`sched_yield`, so swtch_pri) and then parks for 20 ms. A clock that jumped
// on a sole yielder would carry main's 60 s deadline past it and time the wait out; one stride per
// yield does not, and the park's own deadline is what wakes the child.
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

fn main() {
    let (tx, rx) = mpsc::channel();
    let h = thread::spawn(move || {
        for _ in 0..10 {
            thread::yield_now();
        }
        let t0 = Instant::now();
        thread::park_timeout(Duration::from_millis(20));
        tx.send(t0.elapsed() >= Duration::from_millis(20)).unwrap();
    });
    match rx.recv_timeout(Duration::from_secs(60)) {
        Ok(by_timeout) => {
            println!("received");
            println!("parked_by_timeout={}", if by_timeout { "yes" } else { "no" });
        }
        Err(e) => println!("main timed out: {e:?}"),
    }
    h.join().unwrap();
}
