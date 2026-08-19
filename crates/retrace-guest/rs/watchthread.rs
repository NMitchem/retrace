// M15's headline guest. Two threads that write DIFFERENT addresses, so a watch hit's thread
// attribution is a claim that can be wrong — which is what makes the gate meaningful.
//
// The child writes the watched cell; main never touches it. Exit 0 proves nothing on its own, and
// neither does "the watch fired" — the watch already fires correctly today without any of M15. The
// assertion is WHICH THREAD is named.
//
// The two cells' addresses cross no syscall boundary on their own (a `static mut`'s address is
// never an argument to anything the kernel sees), so there is nothing in the trace to learn them
// from the way M13's `protnone_rust_e2e` learns its page from a recorded `mprotect`. Instead the
// guest PRINTS both addresses — that is its own recorded behaviour (stdout), just not a syscall
// argument. Printing BOTH, not just the watched one, is what lets the gate check they are actually
// distinct rather than assuming it.
//
// Same rustc recipe as threadrust: no -C panic=abort, since nothing here is expected to panic.
static mut CHILD_CELL: u64 = 0;
static mut MAIN_CELL: u64 = 0;

fn main() {
    println!("child cell {:#x}", &raw const CHILD_CELL as usize);
    println!("main cell {:#x}", &raw const MAIN_CELL as usize);
    println!("main before spawn");
    let h = std::thread::spawn(|| {
        unsafe { std::ptr::write_volatile(&raw mut CHILD_CELL, 0xC417_D000_0000_0001) };
        println!("child wrote");
    });
    unsafe { std::ptr::write_volatile(&raw mut MAIN_CELL, 0x9A1B_0000_0000_0002) };
    h.join().unwrap();
    println!("joined");
}
