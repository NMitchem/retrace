// M13 headline guest. A stock full-std Rust binary that protects one of its own pages PROT_NONE and
// then stores through it. Proves enforcement end-to-end through real libc, real dyld, and libstd's
// own fault handlers — without depending on stack geometry.
//
// NOT a stack-overflow guest: libstd's guard page lands 7.73 MiB below retrace's real stack bottom
// (M8 spec risk R3, measured in M13 Task 2), so an overflow never strikes it. That capability is
// gated by stackoverflow_rust_e2e, parked #[ignore]d at that wall.
//
// The pre-protect touch is load-bearing: it puts a WRITABLE translation in the TLB, so protect_none
// must invalidate it. Without the flush the second store hits the stale entry and this guest prints
// "survived" and exits 0.
//
// Built by plain rustc with no Cargo, so there is no libc crate available; libSystem is linked
// automatically and the two syscalls are declared directly.
use std::ffi::c_void;

extern "C" {
    fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut c_void;
    fn mprotect(addr: *mut c_void, len: usize, prot: i32) -> i32;
}

const PROT_NONE: i32 = 0;
const PROT_READ: i32 = 1;
const PROT_WRITE: i32 = 2;
const MAP_PRIVATE: i32 = 0x0002;
const MAP_ANON: i32 = 0x1000;

fn main() {
    let len = 0x4000usize;
    let p = unsafe {
        mmap(std::ptr::null_mut(), len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0)
    };
    assert!(!p.is_null() && p as isize != -1, "mmap failed");
    unsafe { std::ptr::write_volatile(p as *mut u64, 0xAAAA) };
    println!("mapped and touched");

    assert_eq!(unsafe { mprotect(p, len, PROT_NONE) }, 0, "mprotect(PROT_NONE) failed");
    println!("protected");

    unsafe { std::ptr::write_volatile(p as *mut u64, 0xBBBB) }; // must fault
    println!("survived");                                       // must never print
}
