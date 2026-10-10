// M49 first-contact corpus (spec §3b), row S: std::thread::sleep(50 ms), which libstd implements
// with nanosleep(&ts, &rem): clock_get_time (mach_msg2 1000), then __semwait_signal (334)
// (spec §2e). Prints whether Instant (mach_absolute_time) advanced by at least 50 ms.
use std::time::{Duration, Instant};

fn main() {
    println!("sleeping 50 ms");
    let t0 = Instant::now();
    std::thread::sleep(Duration::from_millis(50));
    println!("ok={}", (t0.elapsed() >= Duration::from_millis(50)) as u8);
}
