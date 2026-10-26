// M50 Review Focus 1: a Rust program reaching a wall. libstd installs its SIGSEGV and SIGBUS
// handlers and an alternate signal stack before main, so the run up to the wall is the Rust
// runtime's, not C's. chroot(2) is `std::os::unix::fs::chroot`, a call with no row (M53 C1: this
// fixture called fsync(2) until fsync got one).
use std::io::Write;

fn main() {
    println!("cf: rs_chroot");
    std::io::stdout().flush().unwrap();
    let r = std::os::unix::fs::chroot("/nonexistent-cf");
    println!("cf: rs_chroot returned {r:?}");
}
