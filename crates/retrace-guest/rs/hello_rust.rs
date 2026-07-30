// M7 rung 1 of the breadth ladder: the smallest REAL Rust program, built by the real toolchain.
//
// Full std and println! are the point — they pull in std::rt init, the stdout lock, and the stack
// guard, none of which a hand-written C fixture exercises. Deliberately no opt-level tuning and no
// panic=abort: the goal is what the toolchain emits by default. A Rust panic() would go
// panic -> abort() -> SIGABRT, which lands on M6's deferred signal-delivery boundary rather than the
// Stop::Fault crash path, so this guest must not panic.
fn main() {
    println!("hi from rust");
}
