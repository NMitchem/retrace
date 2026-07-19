use retrace_box::*;
// Drive the file-I/O guest to its first read() and confirm forward_and_diff captures the
// file bytes as writes and returns the byte count.
#[test]
fn forward_and_diff_captures_read_bytes() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::FILEIO).unwrap());
    let mut b = Box_::load(&loaded);
    // Advance to the read() syscall (open, then fstat, then read).
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_READ => {
                let (ret, _err, writes) = b.forward_and_diff(num, args);
                assert_eq!(ret, 19, "read should return the 19 fixture bytes");
                // the write must land at the read buffer (args[1]) and contain the fixture
                let w = writes.iter().find(|w| w.ipa == args[1]).expect("no write at read buf");
                assert!(w.bytes.starts_with(b"retrace-m1-fixture\n"));
                return;
            }
            Stop::Syscall { num, args } => { let (ret, _err, _) = b.forward_and_diff(num, args); b.set_x0_and_return(ret); }
            Stop::Other { esr } => panic!("unexpected exit esr=0x{esr:x}"),
            Stop::Fault { pc, esr, far } => panic!("guest crashed pc=0x{pc:x} esr=0x{esr:x} far=0x{far:x}"),
            Stop::Step => unreachable!("run() does not single-step"),
        }
    }
}
