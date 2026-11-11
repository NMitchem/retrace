//! M58 B4: a frame-pointer walk. arm64 macOS keeps `[fp] = previous fp` and `[fp + 8] = return address` in every
//! frame record, so the call chain is a linked list in guest memory. The walk is pure: the caller supplies the
//! registers and a reader, so it is tested on a synthetic memory map and used on the guest's own.

/// One frame: its pc (frame 0's is the stop pc, every other frame's a return address) and the frame pointer that
/// frame's record was read from.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Frame { pub pc: u64, pub fp: u64 }

/// The most frames a walk returns.
pub const MAX_FRAMES: usize = 64;

/// retrace's guest VA is 47 bits (`TCR_EL1.T0SZ = 17`); an arm64e return address carries its PAC in the bits above.
const VA_MASK: u64 = (1 << 47) - 1;

/// Walk the call chain from `pc`, `fp` and `lr`. `read(fp)` returns the record `(previous fp, return address)` at
/// `fp`, or `None` when it is unreadable.
///
/// Frame 0 is `pc`. When `at_entry` (the function has not yet run its `stp x29, x30, [sp, #-N]!`, so `fp` is still
/// the caller's), frame 1 is `lr`, and the first record read is the caller's own. Otherwise frame 1 is the first
/// record's return address. It stops at `fp == 0`, an `fp` that does not rise above the previous one, an unreadable
/// record, a return address of 0, or `max` frames.
pub fn walk(pc: u64, fp: u64, lr: u64, at_entry: bool, read: impl Fn(u64) -> Option<(u64, u64)>, max: usize) -> Vec<Frame> {
    let mut out = vec![Frame { pc, fp }];
    if at_entry && out.len() < max {
        let lr = lr & VA_MASK;
        if lr == 0 { return out; }
        out.push(Frame { pc: lr, fp });
    }
    let mut cur = fp;
    while out.len() < max && cur != 0 {
        let Some((prev, ret)) = read(cur) else { break };
        let ret = ret & VA_MASK;
        if ret == 0 { break; }
        out.push(Frame { pc: ret, fp: prev });
        if prev <= cur { break; }
        cur = prev;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn mem(recs: &[(u64, u64, u64)]) -> impl Fn(u64) -> Option<(u64, u64)> {
        let m: HashMap<u64, (u64, u64)> = recs.iter().map(|&(fp, p, r)| (fp, (p, r))).collect();
        move |fp| m.get(&fp).copied()
    }

    #[test]
    fn a_three_record_chain() {
        let read = mem(&[(0x100, 0x200, 0xa1), (0x200, 0x300, 0xa2), (0x300, 0, 0xa3)]);
        let f = walk(0x10, 0x100, 0x99, false, read, 64);
        assert_eq!(f.iter().map(|f| f.pc).collect::<Vec<_>>(), vec![0x10, 0xa1, 0xa2, 0xa3]);
    }

    #[test]
    fn at_entry_puts_lr_first() {
        let read = mem(&[(0x100, 0x200, 0xa1), (0x200, 0, 0xa2)]);
        let f = walk(0x10, 0x100, 0x99, true, read, 64);
        assert_eq!(f.iter().map(|f| f.pc).collect::<Vec<_>>(), vec![0x10, 0x99, 0xa1, 0xa2]);
    }

    #[test]
    fn a_signed_lr_is_stripped() {
        let signed = 0xb4a5_0000_0000_0000u64 | 0x1_8000_1234;
        let read = mem(&[(0x100, 0, signed)]);
        let f = walk(0x10, 0x100, 0x99, false, read, 64);
        assert_eq!(f[1].pc, 0x1_8000_1234);
        let f = walk(0x10, 0, signed, true, mem(&[]), 64);
        assert_eq!(f[1].pc, 0x1_8000_1234);
    }

    #[test]
    fn a_cycle_stops() {
        let read = mem(&[(0x100, 0x100, 0xa1)]);
        assert_eq!(walk(0x10, 0x100, 0, false, read, 64).len(), 2);
        let read = mem(&[(0x100, 0x200, 0xa1), (0x200, 0x100, 0xa2)]);
        assert_eq!(walk(0x10, 0x100, 0, false, read, 64).len(), 3);
    }

    #[test]
    fn max_and_unreadable_stop() {
        let read = mem(&[(0x100, 0x200, 0xa1), (0x200, 0x300, 0xa2), (0x300, 0x400, 0xa3)]);
        assert_eq!(walk(0x10, 0x100, 0, false, read, 3).len(), 3);
        assert_eq!(walk(0x10, 0x100, 0, false, mem(&[]), 64).len(), 1);
    }
}
