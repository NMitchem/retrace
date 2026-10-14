// M50 Review Focus 1: a Rust program reaching a wall. libstd installs its SIGSEGV and SIGBUS
// handlers and an alternate signal stack before main, so the run up to the wall is the Rust
// runtime's, not C's. fsync(2) is declared by hand: a rustc-built fixture has no libc crate, and
// `File::sync_all` is fcntl(F_FULLFSYNC) on Apple, which has a row.
use std::io::Write;

extern "C" {
    fn fsync(fd: i32) -> i32;
}

fn main() {
    println!("cf: rs_fsync");
    std::io::stdout().flush().unwrap();
    let r = unsafe { fsync(1) };
    println!("cf: rs_fsync returned {r}");
}
