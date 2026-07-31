// M8-stack Task 1. The trace-reproducibility oracle, calibrated against the simplest freestanding
// guest there is: `hello` does write(1, "hello\n") then exit(0) under -nostdlib -static. It has no
// clock, no entropy, no allocator and no ports, so two recordings of it MUST be byte-identical.
// If this test fails, the oracle is wrong — not the guest.
mod util;

#[test]
fn hello_records_reproducibly() {
    util::assert_trace_reproducible(retrace_guest::HELLO);
}
