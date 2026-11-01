//! M54 t0: print a trace's opening snapshot, one `ipa len crc32` line per region and then the
//! registers. The CRC is the trace crate's own (reflected 0xedb88320, init and final xor all ones),
//! restated here because the crate keeps `crc32` private.
use retrace_trace::{Event, Reader};

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xffff_ffff;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (!(crc & 1)).wrapping_add(1));
        }
    }
    !crc
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: snapdump <trace>");
    let events = Reader::open(&path).expect("readable trace");
    let Some(Event::Snapshot { regs, mem }) = events.into_iter().next() else {
        panic!("first event is not a Snapshot");
    };
    for r in &mem {
        println!("{:#x} {} {:08x}", r.ipa, r.bytes.len(), crc32(&r.bytes));
    }
    for (i, x) in regs.x.iter().enumerate() {
        println!("x{i} {x:#x}");
    }
    println!("pc {:#x}\nsp_el0 {:#x}\ncpsr {:#x}", regs.pc, regs.sp_el0, regs.cpsr);
}
