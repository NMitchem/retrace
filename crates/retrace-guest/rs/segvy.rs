// M12 headline guest. A stock full-std Rust binary that faults on a wild pointer.
//
// Deliberately NOT built with -C panic=abort: a hardware fault is not a panic, so unlike M11's
// panicky.rs this needs no flag to reach a signal. What it exercises is libstd's OWN SIGSEGV
// handler, which libstd installs at startup (M11 measured the install: flags 0x41 =
// SA_SIGINFO|SA_ONSTACK). That handler compares si_addr against its guard-page range, concludes
// this is not a stack overflow, resets the disposition to SIG_DFL, and RETURNS — so the faulting
// store re-executes and the second fault terminates the guest.
//
// The fault VA has bit 46 set, as crashy.c's GARBAGE_VA does: only a STAGE-1 translation fault
// reaches Stop::Fault, the stop the delivery arm consults. A VA below 2^36 takes a stage-2 abort
// instead — fatal, and it would never reach a handler at all.
fn main() {
    println!("about to fault");
    unsafe { std::ptr::write_volatile(0x4000_dead_0000usize as *mut u64, 1) };
    println!("survived");
}
