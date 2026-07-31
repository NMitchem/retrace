// THE M7 HEADLINE GATE. Rung 1 of the breadth ladder: a real Rust binary, built by the real
// toolchain with full std, records and replays bit-for-bit through real /usr/lib/dyld AND actually
// reaches main. The rung assertion (util::assert_rung_records_and_replays, proven both ways in
// tests/rung.rs) is what makes "reaches main" load-bearing rather than decorative: without it this
// gate would pass on a guest that crashed inside dyld, because M6 records such a crash as a
// successful recording that replays bit-for-bit.
mod util;

#[test]
#[ignore = "M8-stack ADVANCED rung 1's guard-page wall but did not clear it. libstd's \
            install_main_guard (inlined into std::rt::lang_start_internal) computes align_up(\
            pthread_get_stackaddr_np(self) - pthread_get_stacksize_np(self), pagesize) and mmaps \
            it MAP_FIXED. M8 fixed the FIRST operand: pthread_get_stackaddr_np now returns the \
            GUEST's stack top (0x200000) instead of retrace's host ASLR address — proven by probe, \
            answering kern.usrstack64 with 0x1f0000 moved the mmap by exactly -0x10000. The SECOND \
            operand is NOT RLIMIT_STACK: macOS 26's libpthread reports a constant 0x7fc000 (8 MiB \
            minus one 16 KiB page) as the main thread's size and IGNORES the getrlimit(RLIMIT_STACK) \
            reply — proven by probe, answering 0x10000000 instead of 0x40000 left the mmap address \
            BIT-IDENTICAL. So 0x200000 - 0x7fc000 underflows to 0xffffffffffa04000. Failing trap: \
            mmap (num=197) pc=0x1804aea18 args=[addr=0xffffffffffa04000 len=0x4000 prot=0x3(RW) \
            flags=0x41012(PRIVATE|FIXED|ANON|UNIX03) fd=-1 off=0]. That wild MAP_FIXED request is \
            REFUSED with EINVAL exactly as the real kernel refuses it (it briefly took the RECORDER \
            down instead — an HV_BAD_ARGUMENT panic at the 'hv_vm_map (mmap region)' expect, exit \
            101 — which is what wildfixed_e2e.rs and tests/fixedwild.rs now pin), so the failure is \
            back inside the GUEST, and with a truthful errno: libstd panics 'failed to allocate a \
            guard page: Invalid argument (os error 22)' -> 'fatal runtime error: initialization or \
            cleanup bug, aborting' -> abort. TWO things must land to clear this. (1) The real lever \
            for the guard-page ADDRESS: libpthread's own main-thread stack-size bookkeeping, which \
            the probes prove is not getrlimit. (2) Guest-raised SIGNAL DELIVERY, deferred since M6: \
            the guest's abort forwards __pthread_kill(sig=6) (trap num=328 args=[0x103,0x6]) to the \
            HOST, which kills the record-dyn process itself (exit 134), so the trace ends with no \
            terminal event and replay diverges at the last landmark with 'expected recorded \
            syscall, got None (truncated=false)'. M6's crash recording covers HVF FAULTS, not a \
            signal the guest raises on itself. Un-ignore only on a genuine double pass. See \
            docs/superpowers/specs/2026-07-31-retrace-m8-stack-design.md."]
fn hello_rust_records_and_replays_reaching_main() {
    util::assert_rung_records_and_replays(retrace_guest::HELLO_RUST, b"hi from rust\n");
}
