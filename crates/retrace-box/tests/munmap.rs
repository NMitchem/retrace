use retrace_box::{Box_, Stop};
use retrace_arch::SYS_EXIT;
use retrace_guest::{parse_macho, MMAPGUEST};

// After the guest's own mmap then munmap, the mapped set must shrink back — proving munmap is
// honored (drops + hv_vm_unmap the backing), not a no-op. RED: guest_munmap/mapped_len absent.
#[test]
fn munmap_removes_the_backing() {
    let loaded = parse_macho(&std::fs::read(MMAPGUEST).unwrap());
    let mut b = Box_::load(&loaded);
    let before = b.mapped_len();
    let mut mmapped_ipa = 0u64;
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_MMAP => {
                mmapped_ipa = b.guest_mmap(args[1]);
                assert!(b.mapped_len() > before, "mmap must grow the map set");
                b.set_x0_err_and_return(mmapped_ipa, false);
            }
            Stop::Syscall { num, args } if num == retrace_arch::SYS_MUNMAP => {
                let grown = b.mapped_len();
                b.guest_munmap(mmapped_ipa, args[1]);
                assert!(b.mapped_len() < grown, "munmap must shrink the map set (backing removed)");
                b.set_x0_err_and_return(0, false);
            }
            Stop::Syscall { num, .. } if num == SYS_EXIT => break,
            // MMAPGUEST also does a SYS_WRITE (mirroring its stores to stdout) between the
            // mmap and munmap; forward it (like the general dispatch path) so the vcpu actually
            // resumes instead of re-trapping the same SVC forever.
            Stop::Syscall { num, args } => {
                let (ret, err, _writes) = b.forward_and_diff(num, args);
                b.set_x0_err_and_return(ret, err);
            }
            Stop::Other { esr } => panic!("faulted esr=0x{esr:x}"),
            Stop::Step => unreachable!("run() does not single-step"),
        }
    }
}
