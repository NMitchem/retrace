//! Pure mach_msg2 / MIG codec: register unpacking, request decode, reply encode, routing.
//! No VM access, no I/O — every function is a deterministic bytes-in/bytes-out transform,
//! unit-tested against wire bytes captured from a live run (tests/fixtures/mach_msg2_capture.txt).

// mach_msg2_trap option bits — SPI, from xnu osfmk/mach/message.h (not in the public SDK).
// route() exact-matches the observed KOBJECT send+rcv shape, so a wrong constant cannot
// mis-route silently: any other shape is Unsupported (fail-loud).
const MACH64_SEND_MSG: u64 = 0x1;
const MACH64_RCV_MSG: u64 = 0x2;
const MACH64_SEND_KOBJECT_CALL: u64 = 0x2_0000_0000;

/// The eight mach_msg2_trap registers, unpacked (see the spec's ABI table).
pub struct Msg2 {
    pub data: u64, pub options: u64,
    pub bits: u32, pub send_size: u32,
    pub dest: u32, pub reply_port: u32,
    pub voucher: u32, pub msgh_id: u32,
    pub desc_count: u32, pub rcv_name: u32,
    pub rcv_size: u32, pub priority: u32,
    pub timeout: u64,
}
impl Msg2 {
    pub fn unpack(args: &[u64; 8]) -> Msg2 {
        let lo = |v: u64| v as u32;
        let hi = |v: u64| (v >> 32) as u32;
        Msg2 {
            data: args[0], options: args[1],
            bits: lo(args[2]), send_size: hi(args[2]),
            dest: lo(args[3]), reply_port: hi(args[3]),
            voucher: lo(args[4]), msgh_id: hi(args[4]),
            desc_count: lo(args[5]), rcv_name: hi(args[5]),
            rcv_size: lo(args[6]), priority: hi(args[6]),
            timeout: args[7],
        }
    }
}

/// Where a mach_msg2 goes. ServiceVmMap/StubReclamation are emulated against the guest;
/// Forward is the decided read-only/create-once allowlist (memory-diff'd like any mach trap);
/// Unsupported carries a decoded description for the fail-loud error.
pub enum Route { ServiceVmMap, StubReclamation, Forward(&'static str), Unsupported(String) }

/// Read-only kernel queries + create-once calls that stay forwarded (spec §Scope). Keyed by
/// msgh_id alone: these are kernel-subsystem ids, unambiguous under the KOBJECT options shape.
const FORWARD_ALLOWLIST: &[(u32, &str)] =
    &[(200, "host_info"), (206, "host_get_clock_service"), (3418, "semaphore_create")];

pub fn route(m: &Msg2, guest_task_port: Option<u64>) -> Route {
    if m.options != MACH64_SEND_MSG | MACH64_RCV_MSG | MACH64_SEND_KOBJECT_CALL {
        return Route::Unsupported(format!(
            "options {:#x} (not the kernel-object send+rcv shape)", m.options));
    }
    if let Some((_, name)) = FORWARD_ALLOWLIST.iter().find(|(id, _)| *id == m.msgh_id) {
        return Route::Forward(name);
    }
    if guest_task_port == Some(m.dest as u64) {
        match m.msgh_id {
            4811 => return Route::ServiceVmMap,
            4822 => return Route::StubReclamation,
            _ => {}
        }
    }
    Route::Unsupported(format!(
        "msgh_id {} dest {:#x} (guest task port {:?}) send_size {}",
        m.msgh_id, m.dest, guest_task_port, m.send_size))
}

#[cfg(test)]
mod tests {
    use super::*;
    // Live-captured _kernelrpc_mach_vm_map (4811) mach_msg2 register file (libmalloc nano wall).
    const VM_MAP_ARGS: [u64; 8] = [0x1fb638, 0x2_0000_0003, 0x64_8000_1513,
                                   0x1603_0000_0203, 0x12cb_0000_0000, 0x1603_0000_0001, 0x34, 0];
    #[test]
    fn unpacks_the_packed_register_abi() {
        let m = Msg2::unpack(&VM_MAP_ARGS);
        assert_eq!(m.data, 0x1fb638);
        assert_eq!(m.options, 0x2_0000_0003);
        assert_eq!(m.bits, 0x8000_1513);          // COMPLEX | remote COPY_SEND | local MAKE_SEND_ONCE
        assert_eq!(m.send_size, 0x64);            // 100 bytes
        assert_eq!(m.dest, 0x203);
        assert_eq!(m.reply_port, 0x1603);
        assert_eq!(m.msgh_id, 4811);
        assert_eq!(m.desc_count, 1);
        assert_eq!(m.rcv_name, 0x1603);
        assert_eq!(m.rcv_size, 0x34);             // 52 bytes
        assert_eq!(m.timeout, 0);
    }
    fn msg(msgh_id: u32, dest: u32, options: u64) -> Msg2 {
        let mut m = Msg2::unpack(&VM_MAP_ARGS);
        m.msgh_id = msgh_id; m.dest = dest; m.options = options; m
    }
    const KOBJ: u64 = 0x2_0000_0003;
    #[test]
    fn routes_vm_map_to_service_and_reclamation_to_stub() {
        assert!(matches!(route(&msg(4811, 0x203, KOBJ), Some(0x203)), Route::ServiceVmMap));
        assert!(matches!(route(&msg(4822, 0x203, KOBJ), Some(0x203)), Route::StubReclamation));
    }
    #[test]
    fn routes_the_decided_allowlist_to_forward() {
        assert!(matches!(route(&msg(200,  0x1f03, KOBJ), Some(0x203)), Route::Forward("host_info")));
        assert!(matches!(route(&msg(206,  0x1f03, KOBJ), Some(0x203)), Route::Forward("host_get_clock_service")));
        assert!(matches!(route(&msg(3418, 0x203,  KOBJ), Some(0x203)), Route::Forward("semaphore_create")));
    }
    #[test]
    fn everything_else_fails_loudly() {
        // Unknown msgh_id to the task port.
        assert!(matches!(route(&msg(4816, 0x203, KOBJ), Some(0x203)), Route::Unsupported(_)));
        // Serviceable id but to a NON-task port.
        assert!(matches!(route(&msg(4811, 0x999, KOBJ), Some(0x203)), Route::Unsupported(_)));
        // Task port not learned yet.
        assert!(matches!(route(&msg(4811, 0x203, KOBJ), None), Route::Unsupported(_)));
        // Non-KOBJECT options (daemon IPC shape) and vector-form both refuse.
        assert!(matches!(route(&msg(4811, 0x203, 0x3), Some(0x203)), Route::Unsupported(_)));
        assert!(matches!(route(&msg(4811, 0x203, 0x3_0000_0003), Some(0x203)), Route::Unsupported(_)));
    }
}
