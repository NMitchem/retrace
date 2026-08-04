// M2-xpcport: mint_bootstrap_port hands back a REAL kernel-valid send right (accepts a SEND
// mach_port_mod_refs +1) and is idempotent. This is the premise of the XPC-pipe fix — libxpc's
// __xpc_mach_port_retain_send = mach_port_mod_refs(SEND,+1) must SUCCEED on the handed-back name; it
// returned KERN_INVALID_NAME on the old synthetic constant 0x0BAD0B03 -> NULL pipe -> brk (Task 1).
use retrace_box::Box_;
use retrace_guest::{parse_macho, slice_arm64e, HELLO_DYN, DYLD_PATH};

extern "C" {
    static mach_task_self_: u32;
    fn mach_port_mod_refs(task: u32, name: u32, right: u32, delta: i32) -> i32;
}
const MACH_PORT_RIGHT_SEND: u32 = 0;
const KERN_SUCCESS: i32 = 0;
const KERN_INVALID_NAME: i32 = 0xf; // 15

#[test]
fn minted_bootstrap_port_accepts_a_send_mod_refs_and_is_idempotent() {
    let exe = parse_macho(&std::fs::read(HELLO_DYN).unwrap());
    let dyld = parse_macho(slice_arm64e(&std::fs::read(DYLD_PATH).unwrap()));
    let mut b = Box_::load_dynamic(&exe, &dyld, &["hello_dyn".to_string()]);

    let name = b.mint_bootstrap_port();
    assert_ne!(name, 0, "minted name must be nonzero");
    assert_eq!(b.mint_bootstrap_port(), name, "mint is idempotent (cached)");

    // The fix's premise: a SEND mod_refs(+1) SUCCEEDS on the minted name (it holds a send right)...
    let kr = unsafe { mach_port_mod_refs(mach_task_self_, name, MACH_PORT_RIGHT_SEND, 1) };
    assert_eq!(kr, KERN_SUCCESS, "mod_refs(SEND,+1) on minted port must succeed; kr={kr:#x}");
    // ...whereas the old synthetic constant is not a real name — exactly why the pipe came back NULL.
    let kr_bad = unsafe { mach_port_mod_refs(mach_task_self_, 0x0BAD_0B03, MACH_PORT_RIGHT_SEND, 1) };
    assert_eq!(kr_bad, KERN_INVALID_NAME, "synthetic 0x0BAD0B03 must be INVALID_NAME; kr={kr_bad:#x}");

    // Balance the +1 we added (hygiene; not load-bearing — the process is single-shot).
    let _ = unsafe { mach_port_mod_refs(mach_task_self_, name, MACH_PORT_RIGHT_SEND, -1) };
}
