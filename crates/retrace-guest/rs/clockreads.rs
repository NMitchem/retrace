// M52 Task 6 (spec §3h, C9): a program that reads the clock as often as a long test suite does.
// memchr's suite reads it about 4 million times per native second; this reads it 4 000 000 times
// and says whether its own first and last readings are less than one second apart. Natively that
// takes about 0.1 s; a synthetic clock that moves too far per read puts the answer at `no`.
use std::time::{Duration, Instant};

fn main() {
    let first = Instant::now();
    let mut last = first;
    for _ in 0..4_000_000u32 {
        last = Instant::now();
    }
    let under = last.duration_since(first) < Duration::from_secs(1);
    println!("under_one_guest_second={}", if under { "yes" } else { "no" });
}
