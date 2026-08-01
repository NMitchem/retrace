// M8-stack Task 1. The trace-reproducibility oracle, calibrated against the simplest freestanding
// guest there is: `hello` does write(1, "hello\n") then exit(0) under -nostdlib -static. It has no
// clock, no entropy, no allocator and no ports, so two recordings of it MUST be byte-identical.
// If this test fails, the oracle is wrong — not the guest.
mod util;

#[test]
fn hello_records_reproducibly() {
    util::assert_trace_reproducible(retrace_guest::HELLO);
}

#[test]
fn usrstack_records_deterministically() {
    // `usrstack` is freestanding, so it is in the reproducibility oracle's scope: two recordings
    // must be byte-identical. Today they are not — the recorded sysctl reply carries the host's
    // ASLR'd stack address, which differs every run.
    util::assert_trace_reproducible(retrace_guest::USRSTACK);
}
