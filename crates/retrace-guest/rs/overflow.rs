// The guest for stackoverflow_rust_e2e, which is PARKED #[ignore]d at M8 spec risk R3.
//
// It cannot pass today and the reason is measured, not suspected: libstd computes its guard page at
// pthread_get_stackaddr_np() - pthread_get_stacksize_np(), and macOS 26's libpthread reports a
// CONSTANT 0x7fc000 that retrace cannot influence (M8 measured that answering a different
// getrlimit(RLIMIT_STACK) leaves the address bit-identical). With DYN_STACK_SIZE at 256 KiB the
// guard lands 7.73 MiB BELOW the real stack bottom, so this recursion runs off the stack into
// unbacked IPA and takes a stage-2 fault instead of striking the guard.
//
// black_box on the recursive call is load-bearing: without it the optimizer turns this into a loop.
use std::hint::black_box;

fn recurse(depth: u64) -> u64 {
    let pad = [depth; 64];
    black_box(&pad);
    black_box(recurse(black_box(depth) + 1)) + pad[0]
}

fn main() {
    println!("about to overflow");
    let d = recurse(0);
    println!("survived at depth {d}");
}
