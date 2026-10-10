// M49 first-contact corpus (spec §3b), row F: a Rust read_volatile at 0x10. libstd's SIGSEGV
// handler runs first (it checks si_addr against the main thread's guard page), finds it is not a
// stack overflow, resets the disposition and returns, and the re-executed load kills the process:
// natively 139. println! flushes at the newline, before the fault.
fn main() {
    println!("about to fault");
    let p = 0x10 as *const u64;
    let v = unsafe { std::ptr::read_volatile(p) };
    println!("survived {v}");
}
