// M8-stack. The guest stack identity contract: retrace must tell the guest the truth about its
// OWN address space, and must honor addr/MAP_FIXED for anonymous mmap.
//
// Static (`record`) load path geometry, from crates/retrace-box/src/lib.rs:
//   STACK_TOP_IPA = 0x20000, and load() maps ONE granule at STACK_TOP_IPA - GRANULE,
//   so the guest stack is [0x1C000, 0x20000) -- top 0x20000, size 0x4000.
mod util;

const STATIC_STACK_TOP:  u64 = 0x0002_0000;
const STATIC_STACK_SIZE: u64 = 0x0000_4000;
const FIXED_TARGET:      u64 = 0x000B_0000_0000;

fn fields(stdout: &[u8]) -> (u64, u64, u64, u64) {
    assert_eq!(stdout.len(), 32, "guest must publish exactly four u64s, got {} bytes", stdout.len());
    let g = |i: usize| u64::from_le_bytes(stdout[i..i + 8].try_into().unwrap());
    (g(0), g(8), g(16), g(24))
}

#[test]
fn usrstack64_reports_the_guests_own_stack_top() {
    let (rec, _t) = util::record(retrace_guest::USRSTACK);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let (usrstack, _, _, _) = fields(&rec.stdout);
    assert_eq!(usrstack, STATIC_STACK_TOP,
        "kern.usrstack64 must report the GUEST's stack top, not the host's ({usrstack:#x})");
}

#[test]
fn rlimit_stack_reports_the_guests_own_stack_size() {
    let (rec, _t) = util::record(retrace_guest::USRSTACK);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let (_, cur, max, _) = fields(&rec.stdout);
    assert_eq!((cur, max), (STATIC_STACK_SIZE, STATIC_STACK_SIZE),
        "RLIMIT_STACK must report the GUEST's stack size, not the host's");
}

#[test]
fn anonymous_map_fixed_lands_at_the_requested_address() {
    let (rec, _t) = util::record(retrace_guest::USRSTACK);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let (_, _, _, mapret) = fields(&rec.stdout);
    assert_eq!(mapret, FIXED_TARGET,
        "an anonymous MAP_FIXED mmap must land at the requested address, not at the bump \
         allocator's next slot ({mapret:#x})");
}

#[test]
fn usrstack_replays_bit_for_bit() {
    let (rec, trace) = util::record(retrace_guest::USRSTACK);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, rec.stdout, "replay stdout must match record stdout byte-for-byte");
}
