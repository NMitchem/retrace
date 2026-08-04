// M10 t3. Translation and EBADF, stated against a bare FdTable rather than a Box_.
//
// Deliberately not built on a real Box_: constructing one creates a VM, and HVF allows only one per
// process, so a table-only test that booted a guest would collide with every other VM test in this
// crate under --test-threads=1. The translation logic under test is pure, so the box's thin wrappers
// (Box_::translate_fds / bind_returned_fd) are exercised end-to-end by fdtable_e2e instead.
use retrace_arch::{AT_FDCWD, MWL_MAX_REGION_COUNT, MWL_REGION_STRIDE, fd_operands, allocates_fd,
                   SYS_CLOSE, SYS_MMAP, SYS_OPENAT, SYS_PREAD, SYS_READ_NOCANCEL, SYS_FCNTL_NOCANCEL};
use retrace_box::{EBADF, FdTable};

/// The same walk `Box_::translate_fds` performs, over a bare table.
fn translate(fds: &FdTable, num: u64, args: &mut [u64; 8]) -> Result<(), u64> {
    for &i in fd_operands(num) {
        let v = args[i];
        if (v as i64) < 0 { continue; }
        match fds.host(v) {
            Some(h) => args[i] = h as u64,
            None => return Err(EBADF),
        }
    }
    Ok(())
}

#[test]
fn translate_rewrites_the_guest_fd_to_its_host_fd() {
    let mut t = FdTable::new();
    let g = t.alloc();
    t.bind(g, 17);
    let mut args = [0u64; 8];
    args[0] = g;
    assert!(translate(&t, SYS_PREAD, &mut args).is_ok());
    assert_eq!(args[0], 17, "the host kernel must see the HOST fd, never the guest's");
}

#[test]
fn translate_rejects_a_closed_or_never_opened_fd_with_ebadf() {
    let mut t = FdTable::new();
    let g = t.alloc();
    t.bind(g, 17);
    t.close(g);
    let mut args = [0u64; 8];
    args[0] = g;
    assert_eq!(translate(&t, SYS_CLOSE, &mut args), Err(EBADF), "a closed fd is EBADF");
    args[0] = 42;
    assert_eq!(translate(&t, SYS_CLOSE, &mut args), Err(EBADF), "a never-opened fd is EBADF");
}

#[test]
fn translate_uses_x4_for_mmap() {
    let mut t = FdTable::new();
    let g = t.alloc();
    t.bind(g, 17);
    let mut args = [0u64; 8];
    args[4] = g;
    assert!(translate(&t, SYS_MMAP, &mut args).is_ok());
    assert_eq!(args[4], 17);
    assert_eq!(args[0], 0, "mmap's x0 is an address hint, not an fd — it must be untouched");
}

#[test]
fn at_fdcwd_passes_through_untranslated() {
    let t = FdTable::new();
    let mut args = [0u64; 8];
    args[0] = AT_FDCWD as u64;
    assert!(translate(&t, SYS_OPENAT, &mut args).is_ok(),
        "AT_FDCWD is a sentinel, not a descriptor — it must not be rejected as EBADF");
    assert_eq!(args[0], AT_FDCWD as u64);
}

#[test]
fn nocancel_variants_translate_identically_to_their_plain_forms() {
    // The rows jq actually exercises. Before M10 these were the silent hole.
    let mut t = FdTable::new();
    let g = t.alloc();
    t.bind(g, 17);
    for num in [SYS_READ_NOCANCEL, SYS_FCNTL_NOCANCEL] {
        let mut args = [0u64; 8];
        args[0] = g;
        assert!(translate(&t, num, &mut args).is_ok(), "syscall {num} must translate");
        assert_eq!(args[0], 17, "syscall {num} must reach the kernel with the HOST fd");
    }
}

#[test]
fn console_fds_translate_to_themselves() {
    // M9 intercepts console writes and closes upstream — but ONLY those. stdio's fstat(1)/ioctl(1)
    // still forward, so translating fd 1 must yield retrace's fd 1, not EBADF. Answering EBADF here
    // crashed watch_dyn's dynamic guest, which is how this was found.
    let t = FdTable::new();
    for fd in 0..=2u64 {
        let mut args = [0u64; 8];
        args[0] = fd;
        assert!(translate(&t, SYS_PREAD, &mut args).is_ok(), "console fd {fd} must translate");
        assert_eq!(args[0], fd, "console fds map identically onto retrace's own");
    }
}

#[test]
fn mwl_region_layout_matches_the_sdk_header() {
    // struct mwl_region { int mwlr_fd; vm_prot_t; uint64_t; mach_vm_address_t; mach_vm_size_t; }
    // — 4+4+8+8+8 = 32, with mwlr_fd at offset 0. If this ever drifts, translate_mwl_regions walks
    // the array wrong and hands the kernel a garbage descriptor.
    assert_eq!(MWL_REGION_STRIDE, 32);
    assert_eq!(MWL_MAX_REGION_COUNT, 5);
    // 550 must NOT be in fd_operands: its fd is in guest memory, not a register.
    assert_eq!(fd_operands(retrace_arch::SYS_MAP_WITH_LINKING_NP), &[] as &[usize]);
    assert!(!allocates_fd(retrace_arch::SYS_MAP_WITH_LINKING_NP));
}
