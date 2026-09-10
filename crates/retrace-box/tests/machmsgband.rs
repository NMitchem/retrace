// M32 Task 1: a MEASUREMENT, not a guard. It exists to answer one question — does the M30 guard
// band for mach_msg2's message buffer land at or past `send_size`, where the kernel never reads?
// If it does, the buffer's argument may be canary-filled without re-creating the M30 corruption.
//
// This test is expected to survive the milestone as a regression pin on the measured fact.
use retrace_arch::SYS_EXIT;
use retrace_box::{Box_, Stop};

const MACH_MSG2: u64 = (-47i64) as u64;

#[test]
fn the_band_for_mach_msg2s_buffer_lands_at_or_past_send_size() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::MACHMSG).unwrap());
    let mut b = Box_::load(&loaded);
    let mut seen = 0usize;
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == MACH_MSG2 => {
                let send_size = (args[2] >> 32) as usize;
                let rcv_size = (args[6] & 0xffff_ffff) as usize;
                let len = b.dbg_window_len_for(args[0]);
                // Printed so the triples land in the task report verbatim, per Step 3.
                eprintln!("[M32 t1] buf={:#x} len={len} send_size={send_size} rcv_size={rcv_size}",
                    args[0]);
                assert!(len >= send_size,
                    "band would land INSIDE the kernel-read region: len {len} < send_size \
                     {send_size} for buffer {:#x}. The hypothesis is REFUTED — mach_msg2 must \
                     stay withheld (spec §7, last bullet).", args[0]);
                seen += 1;
                // MEASURED (not in the brief): resuming past this trap here would forward the
                // raw svc to the REAL host kernel, bypassing retrace-core's Route::ServiceVmMap —
                // this bare `Box_` harness has no such routing. The real `mach_vm_map` RPC then
                // succeeds against the *test process's own* task port (itself forwarded above)
                // and hands back a genuine HOST virtual address in the reply, not a guest IPA.
                // The guest's very next instruction stores through that address as if it were a
                // guest pointer, and the guest's stage-1 tables (built by `Box_::load` and
                // knowing nothing about the host's address space) have no mapping for it — a
                // Data Abort (EC 0x24) unrelated to anything this task measures. `machmsg.s`
                // dispatches exactly one `mach_msg2` call, so the measurement this test exists
                // for is already complete the moment it is captured above; stop here rather than
                // resume into a fault that belongs to Route::ServiceVmMap's absence, not to the
                // window-length question this task answers.
                break;
            }
            Stop::Syscall { num, args: _ } if num == SYS_EXIT => break,
            Stop::Syscall { num, args } => {
                let (ret, err, _w) = b.forward_and_diff(num, args);
                b.set_x0_err_and_return(ret, err);
            }
            Stop::Other { esr } => panic!("unexpected exit esr=0x{esr:x}"),
            Stop::Fault { pc, esr, far } => panic!("guest crashed pc=0x{pc:x} esr=0x{esr:x} far=0x{far:x}"),
            Stop::Step => unreachable!("run() does not single-step"),
        }
    }
    assert!(seen > 0,
        "machmsg guest dispatched ZERO mach_msg2 calls — this measurement measured nothing, \
         which is the dead-channel trap the spec's §4b exists to catch");
}
