// THE M7 HEADLINE GATE. Rung 1 of the breadth ladder: a real Rust binary, built by the real
// toolchain with full std, records and replays bit-for-bit through real /usr/lib/dyld AND actually
// reaches main. The rung assertion (util::assert_rung_records_and_replays, proven both ways in
// tests/rung.rs) is what makes "reaches main" load-bearing rather than decorative: without it this
// gate would pass on a guest that crashed inside dyld, because M6 records such a crash as a
// successful recording that replays bit-for-bit.
mod util;

#[test]
#[ignore = "M7 rung 1 is re-parked, past the PAC wall Task 6 fixed, at a new and different-class \
            wall: no HVF fault at all (no pc/esr/far — the PAC-garbled-branch signature is gone). \
            The guest's own Rust runtime panics during std init while installing the main thread's \
            stack-overflow guard page: 'failed to allocate a guard page: Undefined error: 0 (os \
            error 0)' at library/std/src/sys/pal/unix/stack_overflow.rs:526, immediately preceded \
            by an mmap trap (num=197, args addr=0x16f4ec000 len=0x4000 prot=0x3(RW) \
            flags=0x41012(PRIVATE|ANON|FIXED|...) fd=-1 off=0) whose result the guest's libstd \
            treats as failure. The panic drives Rust's abort path, which raises a real SIGABRT that \
            reaches the host record-dyn process itself (exit 134 / Command::status().code()==None) \
            — this is a syscall-surface gap (mmap/guard-page semantics), not a pointer-signing \
            defect, and it lands in the Rust panic/abort -> SIGABRT signal-delivery path that has \
            been out of scope since M6. Un-ignore only on a genuine double pass. See \
            docs/superpowers/specs/2026-07-26-retrace-m7-rust-design.md."]
fn hello_rust_records_and_replays_reaching_main() {
    util::assert_rung_records_and_replays(retrace_guest::HELLO_RUST, b"hi from rust\n");
}
