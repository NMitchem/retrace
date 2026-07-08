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

/// Where a mach_msg2 goes. ServiceVmMap is emulated against the guest; StubMigReply(retcode)
/// answers an optional/no-op kernel routine (no out-params) with a mig_reply_error carrying
/// `retcode`; Forward is the decided read-only/create-once allowlist (memory-diff'd like any
/// mach trap); Unsupported carries a decoded description for the fail-loud error.
pub enum Route { ServiceVmMap, StubMigReply(i32), Forward(&'static str), Unsupported(String) }

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
            // vm_reclaim (deferred reclamation): optional. Report unavailable so libmalloc takes
            // its no-reclaim fallback.
            4822 => return Route::StubMigReply(KERN_NOT_SUPPORTED),
            // Private task_restartable subsystem (base 8000): _register(8000) records
            // libplatform's os_unfair_lock restartable critical sections; _synchronize(8001) is a
            // barrier over them. On a single vCPU with no preemption those ranges can never fire,
            // so a KERN_SUCCESS no-op is a faithful, deterministic answer (both routines have no
            // out-params → mig_reply_error).
            8000 | 8001 => return Route::StubMigReply(KERN_SUCCESS),
            _ => {}
        }
    }
    Route::Unsupported(format!(
        "msgh_id {} dest {:#x} (guest task port {:?}) send_size {}",
        m.msgh_id, m.dest, guest_task_port, m.send_size))
}

pub const MACH_MSG_SUCCESS: u64 = 0;
pub const KERN_SUCCESS: i32 = 0;
pub const KERN_NOT_SUPPORTED: i32 = 46;
pub const KERN_NO_SPACE: i32 = 3;
const MACH_MSGH_BITS_COMPLEX: u32 = 0x8000_0000;

/// _kernelrpc_mach_vm_map (4811) request body (mig __Request__, pack(4); offsets in the plan).
pub struct VmMapReq {
    pub address: u64, pub size: u64, pub mask: u64, pub flags: u32,
    pub offset: u64, pub copy: u32,
    pub cur_protection: u32, pub max_protection: u32, pub inheritance: u32,
}

fn u32_at(b: &[u8], o: usize) -> u32 { u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) }
fn u64_at(b: &[u8], o: usize) -> u64 { u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) }

pub fn decode_vm_map(buf: &[u8]) -> Result<VmMapReq, String> {
    if buf.len() < 100 { return Err(format!("vm_map request short: {} < 100", buf.len())); }
    let (bits, id, descs) = (u32_at(buf, 0), u32_at(buf, 20), u32_at(buf, 24));
    if id != 4811 { return Err(format!("msgh_id {id} != 4811")); }
    if bits & MACH_MSGH_BITS_COMPLEX == 0 { return Err("complex bit clear".into()); }
    if descs != 1 { return Err(format!("descriptor count {descs} != 1")); }
    Ok(VmMapReq {
        address: u64_at(buf, 48), size: u64_at(buf, 56), mask: u64_at(buf, 64),
        flags: u32_at(buf, 72), offset: u64_at(buf, 76), copy: u32_at(buf, 84),
        cur_protection: u32_at(buf, 88), max_protection: u32_at(buf, 92),
        inheritance: u32_at(buf, 96),
    })
}

// Received-reply header constants, golden-copied from the captured kernel reply (fixture is
// authoritative; if the byte-equality test disagrees with these, correct THESE to the fixture).
// NOTE: corrected against the real capture — the brief's starting guess for REPLY_BITS was
// 0x12, but the captured reply header bytes are `00 12 00 00`, i.e. u32::from_le_bytes gives
// 0x00001200, not 0x12. NDR and TRAILER matched the brief's guess exactly (no change needed).
const REPLY_BITS: u32 = 0x1200; // captured bytes 00 12 00 00 (local=MOVE_SEND_ONCE=0x12 << 8)
const NDR: [u8; 8] = [0, 0, 0, 0, 1, 0, 0, 0];
const TRAILER: [u8; 8] = [0, 0, 0, 0, 8, 0, 0, 0]; // mach_msg_trailer_t { type 0, size 8 }

fn reply_header(out: &mut Vec<u8>, msgh_size: u32, reply_port: u32, reply_id: u32) {
    out.extend_from_slice(&REPLY_BITS.to_le_bytes());
    out.extend_from_slice(&msgh_size.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());            // remote: send-once right consumed
    out.extend_from_slice(&reply_port.to_le_bytes());      // local: the port it "arrived" on
    out.extend_from_slice(&0u32.to_le_bytes());            // voucher
    out.extend_from_slice(&reply_id.to_le_bytes());
}

/// KERN_SUCCESS reply for 4811: header(24) + NDR(8) + RetCode(4) + address(8) + trailer(8).
pub fn encode_vm_map_reply(reply_port: u32, address: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(52);
    reply_header(&mut out, 44, reply_port, 4911);
    out.extend_from_slice(&NDR);
    out.extend_from_slice(&0i32.to_le_bytes());            // KERN_SUCCESS
    out.extend_from_slice(&address.to_le_bytes());
    out.extend_from_slice(&TRAILER);
    out
}

/// mig_reply_error_t for any request id: header(24) + NDR(8) + RetCode(4) + trailer(8).
pub fn encode_mig_error(request_msgh_id: u32, reply_port: u32, retcode: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(44);
    reply_header(&mut out, 36, reply_port, request_msgh_id + 100);
    out.extend_from_slice(&NDR);
    out.extend_from_slice(&retcode.to_le_bytes());
    out.extend_from_slice(&TRAILER);
    out
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
    fn routes_vm_map_to_service_and_stubs_to_mig_reply() {
        assert!(matches!(route(&msg(4811, 0x203, KOBJ), Some(0x203)), Route::ServiceVmMap));
        // 4822 vm_reclaim => KERN_NOT_SUPPORTED; 8000/8001 task_restartable => KERN_SUCCESS.
        assert!(matches!(route(&msg(4822, 0x203, KOBJ), Some(0x203)),
                         Route::StubMigReply(KERN_NOT_SUPPORTED)));
        assert!(matches!(route(&msg(8000, 0x203, KOBJ), Some(0x203)),
                         Route::StubMigReply(KERN_SUCCESS)));
        assert!(matches!(route(&msg(8001, 0x203, KOBJ), Some(0x203)),
                         Route::StubMigReply(KERN_SUCCESS)));
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

    // msg 6's 100-byte send buffer (the FIXED 24-GiB PROT_NONE reservation; matches VM_MAP_ARGS).
    const FIXTURE_VM_MAP_REQ: [u8; 100] = [
        0x13, 0x15, 0x00, 0x80, 0x64, 0x00, 0x00, 0x00, 0x03, 0x02, 0x00, 0x00, 0x03, 0x16, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0xcb, 0x12, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x13, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x01, 0x00, 0x00, 0x00,
    ];
    // msg 4's reply — a 44-byte SUCCESS reply + 8-byte trailer = first 52 bytes of its reply dump.
    const FIXTURE_VM_MAP_SUCCESS_REPLY: [u8; 52] = [
        0x00, 0x12, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x16, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x2f, 0x13, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x67, 0x09, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x08, 0x00, 0x00, 0x00,
    ];
    // msg 6's reply — a 36-byte mig_reply_error (KERN_NO_SPACE) + 8-byte trailer = first 44 bytes.
    const FIXTURE_MIG_ERROR_REPLY: [u8; 44] = [
        0x00, 0x12, 0x00, 0x00, 0x24, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x16, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x2f, 0x13, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
        0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00,
    ];
    #[test]
    fn decodes_the_captured_vm_map_request() {
        let r = decode_vm_map(&FIXTURE_VM_MAP_REQ).unwrap();       // msg 6: FIXED 24-GiB reservation
        assert_eq!(r.address, 0x4_0000_0000);
        assert_eq!(r.size, 0x6_0000_0000);
        assert_eq!(r.flags & 0x1, 0);              // NOT ANYWHERE (FIXED)
        assert_eq!(r.cur_protection, 0);           // PROT_NONE => a reservation, not a commit
    }
    #[test]
    fn decode_rejects_malformed() {
        assert!(decode_vm_map(&FIXTURE_VM_MAP_REQ[..96]).is_err());          // short
        let mut bad = FIXTURE_VM_MAP_REQ; bad[20] = 0xcc;                    // msgh_id byte
        assert!(decode_vm_map(&bad).is_err());
        let mut bad = FIXTURE_VM_MAP_REQ; bad[24] = 2;                       // desc_count
        assert!(decode_vm_map(&bad).is_err());
    }
    #[test]
    fn encodes_a_byte_identical_success_reply() {
        // msg 4's real success reply: reply-local port @12, host-returned address @36.
        let port = u32::from_le_bytes(FIXTURE_VM_MAP_SUCCESS_REPLY[12..16].try_into().unwrap());
        let addr = u64::from_le_bytes(FIXTURE_VM_MAP_SUCCESS_REPLY[36..44].try_into().unwrap());
        assert_eq!(encode_vm_map_reply(port, addr), FIXTURE_VM_MAP_SUCCESS_REPLY.to_vec());
    }
    #[test]
    fn encodes_a_byte_identical_mig_error_reply() {
        // msg 6's real error reply (KERN_NO_SPACE=3); request id 4811 => reply id 4911.
        let port = u32::from_le_bytes(FIXTURE_MIG_ERROR_REPLY[12..16].try_into().unwrap());
        assert_eq!(encode_mig_error(4811, port, KERN_NO_SPACE), FIXTURE_MIG_ERROR_REPLY.to_vec());
    }
    #[test]
    fn mig_error_reply_has_the_documented_shape() {
        let e = encode_mig_error(4822, 0x1603, KERN_NOT_SUPPORTED);          // the 4822 stub case
        assert_eq!(e.len(), 44);
        assert_eq!(u32::from_le_bytes(e[4..8].try_into().unwrap()), 36);     // msgh_size
        assert_eq!(i32::from_le_bytes(e[20..24].try_into().unwrap()), 4922); // reply id
        assert_eq!(i32::from_le_bytes(e[32..36].try_into().unwrap()), KERN_NOT_SUPPORTED);
    }
    #[test]
    fn restartable_register_stub_replies_success() {
        // task_restartable_ranges_register (8000) has no out-params: its reply is a mig_reply_error
        // with RetCode = KERN_SUCCESS. Reply id = 8000 + 100 = 8100; rcv_size 44 = 36 + 8 trailer.
        let e = encode_mig_error(8000, 0x1703, KERN_SUCCESS);
        assert_eq!(e.len(), 44);
        assert_eq!(u32::from_le_bytes(e[4..8].try_into().unwrap()), 36);     // msgh_size
        assert_eq!(u32::from_le_bytes(e[12..16].try_into().unwrap()), 0x1703); // reply-local port
        assert_eq!(i32::from_le_bytes(e[20..24].try_into().unwrap()), 8100); // reply id
        assert_eq!(i32::from_le_bytes(e[32..36].try_into().unwrap()), 0);    // KERN_SUCCESS
    }
}
