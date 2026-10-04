//! M48 Task 1, box level: every SIMD restore installs the value it was handed. Before the fix each
//! of these read back whatever the host had in v0 (walls.md §4 item 1). Static box: the restores
//! under test touch only the vCPU and the thread table.
use retrace_box::thread::ThreadCtx;
use retrace_box::Box_;

fn tb() -> Box_ {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::SPINLOOP).unwrap());
    Box_::load(&loaded)
}

/// 32 distinct values that no register holds by accident.
fn values(seed: u128) -> [u128; 32] {
    std::array::from_fn(|i| std::hint::black_box(seed ^ ((i as u128 + 1) * 0x0101_0101_0101_0101_0101_0101_0101_0101)))
}

#[test]
fn the_debuggers_register_write_installs_its_value_in_every_q_register() {
    let mut b = tb();
    let v = values(0x5eed_0000_0000_0000_0000_0000_0000_0000);
    for (n, &x) in v.iter().enumerate() { b.vcpu_set_q(n as u32, x); }
    for (n, &x) in v.iter().enumerate() { assert_eq!(b.vcpu_get_q(n as u32), x, "q{n}"); }
}

#[test]
fn a_thread_switch_installs_the_incoming_threads_simd_registers() {
    let mut b = tb();
    let mut ctx = ThreadCtx::zeroed();
    ctx.fp = values(0x7417_0000_0000_0000_0000_0000_0000_0000);
    let want = ctx.fp;
    let t = b.threads_mut().spawn(ctx, (0, 0));
    b.switch_to_thread(t);
    for (n, &x) in want.iter().enumerate() { assert_eq!(b.vcpu_get_q(n as u32), x, "thread {t} q{n} after the switch"); }
    let main_saved = b.threads().ctx_of(0).fp;
    b.switch_to_thread(0);
    for (n, &x) in main_saved.iter().enumerate() { assert_eq!(b.vcpu_get_q(n as u32), x, "main q{n} after the switch back"); }
}

#[test]
fn a_checkpoint_restore_installs_the_captured_simd_registers() {
    let b = tb();
    let mut st = b.checkpoint();
    st.fp = values(0xc4ec_0000_0000_0000_0000_0000_0000_0000);
    let want = st.fp;
    drop(b); // one VM per process: the restored box builds its own
    let r = Box_::from_checkpoint(&st);
    for (n, &x) in want.iter().enumerate() { assert_eq!(r.vcpu_get_q(n as u32), x, "q{n} after from_checkpoint"); }
}
