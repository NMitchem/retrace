// Throwaway: which events of a trace touch a given ipa, and what each snapshot holds there.
// Usage: ipawho <trace> <ipa-hex> [radius]
use retrace_trace::{Event, Reader, Region};

fn cover(r: &Region, ipa: u64) -> Option<u8> {
    if ipa >= r.ipa && ipa < r.ipa + r.bytes.len() as u64 { Some(r.bytes[(ipa - r.ipa) as usize]) } else { None }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let ipa = u64::from_str_radix(a[2].trim_start_matches("0x"), 16).unwrap();
    let (ev, torn) = Reader::open_checked(&a[1]).unwrap();
    println!("events={} torn={}", ev.len(), torn);
    let mut nums: std::collections::BTreeMap<u64, usize> = Default::default();
    for (i, e) in ev.iter().enumerate() {
        match e {
            Event::Snapshot { mem, regs } => {
                let hit: Vec<_> = mem.iter().filter_map(|r| cover(r, ipa).map(|b| (r.ipa, r.bytes.len(), b))).collect();
                println!("#{i} Snapshot pc={:#x} regions={} covering={:x?}", regs.pc, mem.len(), hit);
                for r in mem { if cover(r, ipa).is_some() {
                    let off = (ipa - r.ipa) as usize;
                    let lo = off.saturating_sub(16); let hi = (off + 16).min(r.bytes.len());
                    println!("    bytes[{:#x}..{:#x}] = {:02x?}", r.ipa + lo as u64, r.ipa + hi as u64, &r.bytes[lo..hi]);
                }}
            }
            Event::Syscall { num, args, ret, err, writes, thread, .. } => {
                *nums.entry(*num).or_default() += 1;
                let lo = ipa & !0x3fffff; let hi = lo + 0x400000;
                if args.iter().any(|&a| a >= lo && a < hi) && writes.iter().all(|w| cover(w, ipa).is_none()) {
                    println!("#{i} Syscall(args in block) num={} args={:x?} ret={ret:#x} err={err} thread={thread} writes={}", *num as i64, args, writes.len());
                }
                for w in writes { if let Some(b) = cover(w, ipa) {
                    println!("#{i} Syscall num={} args={:x?} ret={ret:#x} err={err} thread={thread} write ipa={:#x} len={:#x} byte={b:#04x}",
                        *num as i64, args, w.ipa, w.bytes.len());
                }}
            }
            Event::SignalDelivery { writes, sig, thread, .. } => {
                for w in writes { if let Some(b) = cover(w, ipa) {
                    println!("#{i} SignalDelivery sig={sig} thread={thread} write ipa={:#x} len={:#x} byte={b:#04x}", w.ipa, w.bytes.len());
                }}
            }
            Event::Exit { code, thread } => println!("#{i} Exit code={code} thread={thread}"),
            Event::Crash { pc, esr, far, thread } => println!("#{i} Crash pc={pc:#x} esr={esr:#x} far={far:#x} thread={thread}"),
            Event::Signal { sig, pc, thread } => println!("#{i} Signal sig={sig} pc={pc:#x} thread={thread}"),
        }
    }
    // Page map of a range: per 16 KiB page, the final snapshot's non-zero byte count and the last
    // recorded syscall write's non-zero byte count there (the kernel's content, before guest edits).
    if let (Some(lo), Some(hi)) = (a.get(3), a.get(4)) {
        let lo = u64::from_str_radix(lo.trim_start_matches("0x"), 16).unwrap();
        let hi = u64::from_str_radix(hi.trim_start_matches("0x"), 16).unwrap();
        let fin = ev.iter().rev().find_map(|e| if let Event::Snapshot { mem, .. } = e { Some(mem) } else { None }).unwrap();
        let mut page = lo;
        while page < hi {
            let fin_nz = (page..page + 0x4000).filter(|&x| fin.iter().find_map(|r| cover(r, x)).unwrap_or(0) != 0).count();
            let mut last: Option<(usize, usize)> = None;
            for (i, e) in ev.iter().enumerate() {
                if let Event::Syscall { writes, .. } = e {
                    if !writes.iter().any(|w| w.ipa < page + 0x4000 && w.ipa + w.bytes.len() as u64 > page) { continue; }
                    let n = (page..page + 0x4000).filter(|&x| writes.iter().any(|w| cover(w, x).is_some())).count();
                    if n > 0 {
                        let nz = (page..page + 0x4000).filter(|&x| writes.iter().find_map(|w| cover(w, x)).unwrap_or(0) != 0).count();
                        last = Some((i, nz));
                    }
                }
            }
            println!("page {page:#x}: final snapshot non-zero bytes={fin_nz:5}  last recorded write (landmark, non-zero bytes)={last:?}");
            page += 0x4000;
        }
    }
    let mut v: Vec<_> = nums.into_iter().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1));
    println!("kevent_qos(374) events: {}", v.iter().find(|(n, _)| *n == 374).map(|x| x.1).unwrap_or(0));
    println!("syscall counts (top 25): {:?}", v.iter().take(25).map(|(n, c)| (*n as i64, *c)).collect::<Vec<_>>());
}
