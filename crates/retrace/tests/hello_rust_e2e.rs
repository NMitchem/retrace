// THE M7 HEADLINE GATE — GREEN as of M8-stack. Rung 1 of the breadth ladder: a real Rust binary,
// built by the real toolchain with full std, records and replays bit-for-bit through real
// /usr/lib/dyld AND actually reaches main. The rung assertion
// (util::assert_rung_records_and_replays, proven both ways in tests/rung.rs) is what makes "reaches
// main" load-bearing rather than decorative: without it this gate would pass on a guest that crashed
// inside dyld, because M6 records such a crash as a successful recording that replays bit-for-bit.
//
// It was `#[ignore]`d through M7 and most of M8 at libstd's stack-overflow guard page. The whole
// chain, for anyone reading this after a regression: `install_main_guard` mmaps MAP_FIXED at
// `pthread_get_stackaddr_np() - pthread_get_stacksize_np()`. M7 parked because BOTH operands were
// wrong. M8 fixed the first (`kern.usrstack64` was forwarded, handing the guest retrace's own host
// ASLR stack address) and proved by probe that the second is untouchable: macOS 26's libpthread
// reports a CONSTANT 0x7fc000 for the main thread and ignores the `getrlimit(RLIMIT_STACK)` reply
// entirely. With a 2 MiB stack top that subtraction underflowed to 0xffffffffffa04000, which first
// aborted the recorder inside hv_vm_map (now EINVAL — see wildfixed_e2e.rs) and then aborted the
// guest. The close was to stop fighting the constant and give it room: DYN_STACK_TOP moved to 40
// MiB, so the guard page lands at 0x2004000 in free, mappable space. `stack_geometry_tests::
// the_guard_page_libstd_computes_is_a_mappable_guest_address` pins that arithmetic instantly on
// every gate; this test is the end-to-end proof.
mod util;

#[test]
fn hello_rust_records_and_replays_reaching_main() {
    util::assert_rung_records_and_replays(retrace_guest::HELLO_RUST, b"hi from rust\n");
}
