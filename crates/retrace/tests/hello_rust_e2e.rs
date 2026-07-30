// THE M7 HEADLINE GATE. Rung 1 of the breadth ladder: a real Rust binary, built by the real
// toolchain with full std, records and replays bit-for-bit through real /usr/lib/dyld AND actually
// reaches main. The rung assertion (util::assert_rung_records_and_replays, proven both ways in
// tests/rung.rs) is what makes "reaches main" load-bearing rather than decorative: without it this
// gate would pass on a guest that crashed inside dyld, because M6 records such a crash as a
// successful recording that replays bit-for-bit.
mod util;

#[test]
#[ignore = "M7 rung 1 is parked at a PAC-garbled branch in dyld: a rustc-built hello_rust dies \
            after ~245 traps (the count drifts 245-247 run-to-run; the crash site does not) \
            without reaching main. EC 0x20 (instruction abort, lower EL), IFSC \
            level-0 translation fault, branch target 0x67c0001800fc388 = live PAC signature bits \
            over the valid shared-cache address 0x1800fc388 — the guest branched through a signed \
            pointer as if it were raw. Un-ignore only on a genuine double pass. See \
            docs/superpowers/specs/2026-07-26-retrace-m7-rust-design.md."]
fn hello_rust_records_and_replays_reaching_main() {
    util::assert_rung_records_and_replays(retrace_guest::HELLO_RUST, b"hi from rust\n");
}
