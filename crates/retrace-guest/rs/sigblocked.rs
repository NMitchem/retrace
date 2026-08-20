// crates/retrace-guest/rs/sigblocked.rs
//
// The guest for the sigblocked_e2e gate, which M17 un-parked: a signal whose target is BLOCKED in
// __ulock_wait, not merely not-current.
//
// Three threads, not two, and that is forced rather than incidental. The cooperative scheduler
// switches only on block or exit, so main can never be running while a peer is blocked — for the
// peer to have blocked, main must have blocked first. A blocked JOINER, though, leaves its joinee
// running: main joins a, a joins b, so b runs while a sits in __ulock_wait. b is the only thread
// that can express this signal.
//
// Written while the gate was still parked (M16 t13), deliberately as real code that compiled and
// could be un-ignored the day the wall fell rather than as prose describing a guest that did not
// exist. M17 Tasks 3-6 cleared that wall; Task 7 then only had to delete the `#[ignore]`, and the
// gate's four assertions passed unchanged.
use std::sync::atomic::{AtomicU64, Ordering};

extern "C" {
    fn pthread_kill(thread: u64, sig: i32) -> i32;
    fn sigaction(sig: i32, act: *const SigAction, old: *mut SigAction) -> i32;
    fn pthread_self() -> u64;
}
#[repr(C)]
struct SigAction { handler: usize, mask: u32, flags: i32 }
const SIGUSR1: i32 = 30;
// SA_SIGINFO is MANDATORY, not stylistic. `signal(3)` installs a handler WITHOUT it, and
// sig.rs's build_frame asserts fail-loud that a non-SA_SIGINFO handler is unmodelled. A guest
// that trips that assert would park this gate at the wrong wall — documenting an SA_SIGINFO
// limitation instead of the blocked-target one this gate exists to record. Same shape as
// sigthread.rs, deliberately.
const SA_SIGINFO: i32 = 0x0040;
static A_PT: AtomicU64 = AtomicU64::new(0);
extern "C" fn on_usr1(_sig: i32, _info: *mut u8, _uap: *mut u8) {}

fn main() {
    let act = SigAction { handler: on_usr1 as *const () as usize, mask: 0, flags: SA_SIGINFO };
    assert_eq!(unsafe { sigaction(SIGUSR1, &act, core::ptr::null_mut()) }, 0);
    let a = std::thread::spawn(|| {
        // Publish a's own pthread_t BEFORE blocking, so b has something to name.
        A_PT.store(unsafe { pthread_self() }, Ordering::SeqCst);
        let b = std::thread::spawn(|| {
            // a is Blocked(Wait { addr }) right now: b was scheduled precisely because a
            // blocked. MEASURED (Task 13, forced with --ignored): the record-side panic names
            // `Blocked(Wait { addr: 809578548 })`, NOT `Blocked(Join)` — `pthread_join` blocks in
            // `__ulock_wait`, and `guest_ulock_wait` is the only site in the box that ever calls
            // `threads.block`, so `BlockReason::Join` is a variant nothing produces today.
            let at = A_PT.load(Ordering::SeqCst);
            unsafe { pthread_kill(at, SIGUSR1) };
            println!("b signalled a");
        });
        b.join().unwrap();
        println!("a resumed");
    });
    a.join().unwrap();
    println!("done");
}
