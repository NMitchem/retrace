// crates/retrace-guest/rs/sigthread.rs
//
// M16's headline guest: the first that is both THREADED and SIGNALLING. The oracle's caught-raise
// and sigreturn mirrors have never been reached by a guest with two live threads (M15's standing
// fidelity caveat), and `pthread_kill`'s target port has never been decoded at all.
//
// The ordering is the proof, not a convenience:
//   * the child is spawned BEFORE main masks anything, so it inherits an empty mask (Task 9)
//   * main signals the child while the child is Runnable-but-NOT-current — the cooperative
//     scheduler switches only on block or exit, so main still holds the vCPU
//   * the child therefore takes the signal in its NEVER-RUN entry context: the handler runs, then
//     sigreturn lands on `thread_start_pc` and the body starts.
//
// THE STDOUT LINE ORDER IS THIS GUEST'S REAL OBSERVABLE, and it is worth stating exactly, because
// all of it is MEASURED (record-dyn'd through the CLI before Task 5 was written):
//
//   native, 20/20 identical runs:  installed | child pthread | kill rc 0 | handler | child body | joined
//   retrace TODAY (pre-Task 7):    installed | child pthread | handler | kill rc 0 | child body | joined
//
// The inversion of `handler` and `kill rc 0` IS the defect M16 closes, made visible. Today retrace
// ignores __pthread_kill's target port and delivers to whoever is running — main — synchronously
// inside the pthread_kill syscall, so the handler prints BEFORE the syscall returns. Natively the
// CHILD takes it, so main's pthread_kill returns first. After Task 7 the child takes it here too:
// main prints "kill rc 0", blocks in join, and only then does the child run its handler and body —
// i.e. retrace's order becomes native's order, exactly.
//
// So Task 5's test asserts the WRONG order on purpose, documenting the bug; Task 7 flips it. Both
// assert against retrace's own recorded behaviour rather than against a native execution, because
// POSIX guarantees no such ordering — a native run is not a specification even when it reproduces
// 20/20 on one host. That retrace's post-M16 order happens to equal native's is a fidelity result
// worth reporting, not the thing being tested.
//
// Task 5 shipped steps 1-4; Task 9 appended the mask/pending half at the end of `main`, whose own
// claim is stated at that code rather than repeated here.
//
// Same rustc recipe as watchthread: no -C panic=abort.

// PLAIN `extern "C"`, not `unsafe extern "C"`. build.rs invokes rustc with no `--edition`, so
// every Rust guest compiles as edition 2015, where `unsafe extern` is a syntax error. The only
// other Rust guest with an extern block, `protrust.rs:17`, uses this form. MEASURED: this file
// compiles and runs with the exact build.rs recipe; the `unsafe extern` form does not.
extern "C" {
    fn pthread_kill(thread: u64, sig: i32) -> i32;
    fn sigaction(sig: i32, act: *const SigAction, old: *mut SigAction) -> i32;
    #[link_name = "write"]
    fn libc_write(fd: i32, buf: *const u8, n: usize) -> isize;
    // The Task 9 half. `sigset_t` on macOS is 32 bits — VERIFIED against this host's SDK, not
    // assumed: `sys/_types/_sigset_t.h` has `typedef __darwin_sigset_t sigset_t;` and
    // `sys/_types.h:85` has `typedef __uint32_t __darwin_sigset_t;`. So `u32` is the exact
    // width; a wider type would hand the kernel bytes past the end of the set.
    fn pthread_sigmask(how: i32, set: *const u32, old: *mut u32) -> i32;
    fn sigpending(set: *mut u32) -> i32;
    fn pthread_self() -> u64;
}

const SIG_BLOCK: i32 = 1;
const SIG_UNBLOCK: i32 = 2;

// SA_SIGINFO, installed via `sigaction` — NOT `signal(3)`. MEASURED: a `signal()`-installed
// handler is non-SA_SIGINFO, and `build_frame` (sig.rs:262) asserts fail-loud on exactly that:
// "a non-SA_SIGINFO handler is not modelled. Its infostyle is 0x1 (measured, vs 0x1e for
// SA_SIGINFO) and the frame layout is identical, so supporting it is small — but no gate guest
// exercises it." That wall is real and is NOT M16's to clear: infostyle is unrelated to thread
// attribution, and clearing it here would be scope creep into a different modelling gap. The
// assert stays honest and untouched; this guest simply installs the shape the box models.
//
// macOS `struct sigaction`: 8-byte handler union, 4-byte sigset_t mask, 4-byte flags. libc's
// wrapper fills in sa_tramp itself, so the guest never declares it.
#[repr(C)]
struct SigAction { handler: usize, mask: u32, flags: i32 }

const SA_SIGINFO: i32 = 0x0040;

const SIGUSR1: i32 = 30;

extern "C" fn on_usr1(_sig: i32, _info: *mut u8, _uap: *mut u8) {
    // A raw write(2) rather than println!: a handler must not take libstd's stdout lock, which the
    // interrupted thread may already hold. The child is the only thread that runs this in Task 5,
    // but the Task 9 half runs it on main too.
    let msg = b"handler\n";
    unsafe { libc_write(1, msg.as_ptr(), msg.len()) };
}

fn main() {
    // `as *const () as usize`, not `as usize`: rustc 1.95 warns `direct cast of function item
    // into an integer` (function_casts_as_integer, on by default) on the direct form, and a guest
    // that warns on every build is not pristine output.
    let act = SigAction { handler: on_usr1 as *const () as usize, mask: 0, flags: SA_SIGINFO };
    assert_eq!(unsafe { sigaction(SIGUSR1, &act, core::ptr::null_mut()) }, 0);
    println!("installed");

    let h = std::thread::spawn(|| {
        println!("child body");
    });

    // The child's pthread_t, which is what `pthread_kill` names. std exposes it; no libc crate.
    use std::os::unix::thread::JoinHandleExt;
    let child = h.as_pthread_t() as u64;
    println!("child pthread {child:#x}");

    let rc = unsafe { pthread_kill(child, SIGUSR1) };
    println!("kill rc {rc}");

    h.join().unwrap();
    println!("joined");

    // The mask/pending half (Task 9). Main blocks SIGUSR1 for ITSELF, raises it on itself so it
    // must pend, observes sigpending reporting it, then unblocks — which is the landmark the
    // delivery is anchored to. Every step is main's own: the child has already been joined, so
    // nothing here depends on a second live thread, and the pending set is exercised on the one
    // thread whose Runnable-ness at its own sigprocmask is true by construction.
    let set: u32 = 1u32 << (SIGUSR1 - 1);
    let mut old: u32 = 0;
    unsafe { pthread_sigmask(SIG_BLOCK, &set, &mut old) };
    let rc2 = unsafe { pthread_kill(pthread_self(), SIGUSR1) };
    println!("self kill rc {rc2}");

    let mut pend: u32 = 0;
    unsafe { sigpending(&mut pend) };
    println!("pending {}", (pend >> (SIGUSR1 - 1)) & 1);

    // The anchor. `handler` prints from INSIDE this call — the delivery is materialised at the
    // unmask landmark, before it returns — so `handler` precedes `unblocked` in stdout, and that
    // adjacency is what the e2e asserts on the trace side as SignalDelivery-immediately-after-
    // the-mask-Syscall.
    unsafe { pthread_sigmask(SIG_UNBLOCK, &set, &mut old) };
    println!("unblocked");
}
