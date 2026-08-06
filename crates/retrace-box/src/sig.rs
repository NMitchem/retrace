//! The guest's signal dispositions (M11-signals).
//!
//! **Pure guest state.** Every field is a function of the guest's own `sigaction`/`sigprocmask`/
//! `sigaltstack` calls, so record and replay compute an identical table from an identical syscall
//! sequence and nothing here ever enters the trace. That is `FdTable::slots`' posture, and it is why
//! this module has no `Box_` and no VM in it — the whole thing is unit-testable at full speed.
//!
//! **Disposition, not delivery.** M11 models what the guest ASKED for. It never runs a handler:
//! `Handler` exists so the raise path can fail loud instead of silently applying the default action.

use retrace_arch::{NSIG, SIG_BLOCK, SIG_DFL, SIG_IGN, SIG_SETMASK, SIG_UNBLOCK};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Dfl,
    Ign,
    Handler(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SigAction {
    pub disp: Disposition,
    pub mask: u32,
    pub flags: u32,
}

impl Default for SigAction {
    fn default() -> Self {
        SigAction { disp: Disposition::Dfl, mask: 0, flags: 0 }
    }
}

/// Per-signal disposition, the blocked mask, and the alternate stack.
///
/// `altstack` is STORED but never honoured: no handler runs this milestone, so there is nothing to
/// run on an alternate stack. Keeping it makes `sigaltstack` a real syscall with a real writeback
/// rather than a lie, and costs one field.
#[derive(Debug, Clone)]
pub struct SigTable {
    /// Indexed by signal number; `[0]` is unused so indexing mirrors signal numbering (1..=31).
    disp: [SigAction; NSIG],
    /// Bit `(sig - 1)`, matching `sigset_t`'s encoding for signals 1..=32.
    blocked: u32,
    altstack: Option<(u64, u64, u64)>,
}

impl Default for SigTable {
    /// All-default, nothing blocked, no alt stack — which is genuinely correct for a fresh process,
    /// so there is no seeding step that could be got wrong.
    fn default() -> Self {
        SigTable { disp: [SigAction::default(); NSIG], blocked: 0, altstack: None }
    }
}

impl SigTable {
    fn idx(sig: u64) -> usize {
        assert!(
            sig >= 1 && (sig as usize) < NSIG,
            "signal {sig} out of range 1..{NSIG} — the guest passed a signal number the table \
             cannot represent; widen NSIG or reject it at the syscall arm"
        );
        sig as usize
    }

    pub fn action(&self, sig: u64) -> SigAction {
        self.disp[Self::idx(sig)]
    }

    pub fn set_action(&mut self, sig: u64, a: SigAction) -> SigAction {
        std::mem::replace(&mut self.disp[Self::idx(sig)], a)
    }

    pub fn is_blocked(&self, sig: u64) -> bool {
        self.blocked & (1u32 << (Self::idx(sig) - 1)) != 0
    }

    pub fn mask(&self) -> u32 {
        self.blocked
    }

    pub fn set_mask(&mut self, how: u64, set: u32) -> u32 {
        let old = self.blocked;
        self.blocked = match how {
            SIG_BLOCK => old | set,
            SIG_UNBLOCK => old & !set,
            SIG_SETMASK => set,
            _ => panic!(
                "sigprocmask how={how} is not BLOCK(1)/UNBLOCK(2)/SETMASK(3) — an unmodelled \
                 value, not a guest error to swallow"
            ),
        };
        old
    }

    pub fn altstack(&self) -> Option<(u64, u64, u64)> {
        self.altstack
    }

    pub fn set_altstack(&mut self, ss: Option<(u64, u64, u64)>) -> Option<(u64, u64, u64)> {
        std::mem::replace(&mut self.altstack, ss)
    }
}

/// Decode the ACT argument: `struct __sigaction`, 24 bytes (`sys/signal.h:277`).
///
/// `sa_tramp` (offset 8) is read past and DISCARDED — it addresses libc's signal trampoline, which
/// only matters once a handler is delivered, and M11 delivers nothing.
pub fn decode_act(bytes: &[u8]) -> SigAction {
    assert!(
        bytes.len() >= 24,
        "struct __sigaction is 24 bytes, got {} — the caller read too few guest bytes",
        bytes.len()
    );
    let h = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
    SigAction {
        disp: match h {
            SIG_DFL => Disposition::Dfl,
            SIG_IGN => Disposition::Ign,
            va => Disposition::Handler(va),
        },
        mask: u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
        flags: u32::from_le_bytes(bytes[20..24].try_into().unwrap()),
    }
}

/// Encode the OLDACT writeback: `struct sigaction`, **16 bytes** (`sys/signal.h:287`) — the input
/// struct's `sa_tramp` is absent, so `sa_mask` moves from offset 16 to offset 8.
///
/// The return type is a fixed `[u8; 16]` on purpose: emitting 24 bytes here would corrupt the guest
/// 8 bytes past the struct, and the fixed width makes that impossible rather than merely tested.
pub fn encode_oldact(a: SigAction) -> [u8; 16] {
    let h = match a.disp {
        Disposition::Dfl => SIG_DFL,
        Disposition::Ign => SIG_IGN,
        Disposition::Handler(va) => va,
    };
    let mut o = [0u8; 16];
    o[0..8].copy_from_slice(&h.to_le_bytes());
    o[8..12].copy_from_slice(&a.mask.to_le_bytes());
    o[12..16].copy_from_slice(&a.flags.to_le_bytes());
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use retrace_arch::{SIG_BLOCK, SIG_SETMASK, SIG_UNBLOCK};

    #[test]
    fn fresh_table_is_all_default_unblocked_no_altstack() {
        let t = SigTable::default();
        for sig in 1..32u64 {
            assert_eq!(t.action(sig).disp, Disposition::Dfl, "sig {sig}");
            assert!(!t.is_blocked(sig), "sig {sig}");
        }
        assert_eq!(t.mask(), 0);
        assert_eq!(t.altstack(), None);
    }

    #[test]
    fn set_action_returns_the_previous_action() {
        let mut t = SigTable::default();
        let old = t.set_action(6, SigAction { disp: Disposition::Ign, mask: 0, flags: 0 });
        assert_eq!(old.disp, Disposition::Dfl, "first install returns the default");
        let old = t.set_action(6, SigAction { disp: Disposition::Handler(0x1_0000), mask: 3, flags: 4 });
        assert_eq!(old.disp, Disposition::Ign, "second install returns what the first set");
        assert_eq!(t.action(6), SigAction { disp: Disposition::Handler(0x1_0000), mask: 3, flags: 4 });
    }

    #[test]
    fn mask_honours_block_unblock_setmask() {
        let mut t = SigTable::default();
        assert_eq!(t.set_mask(SIG_BLOCK, 0b0110), 0, "returns the OLD mask");
        assert_eq!(t.mask(), 0b0110);
        assert_eq!(t.set_mask(SIG_BLOCK, 0b1000), 0b0110);
        assert_eq!(t.mask(), 0b1110, "BLOCK is a union");
        assert_eq!(t.set_mask(SIG_UNBLOCK, 0b0100), 0b1110);
        assert_eq!(t.mask(), 0b1010, "UNBLOCK clears");
        assert_eq!(t.set_mask(SIG_SETMASK, 0b0001), 0b1010);
        assert_eq!(t.mask(), 0b0001, "SETMASK replaces");
    }

    #[test]
    fn is_blocked_indexes_by_sig_minus_one() {
        let mut t = SigTable::default();
        t.set_mask(SIG_SETMASK, 1 << 5); // bit 5 == signal 6
        assert!(t.is_blocked(6), "bit (sig-1) is the encoding");
        assert!(!t.is_blocked(5));
        assert!(!t.is_blocked(7));
    }

    // THE golden test. sigaction(2)'s in-param and out-param are different C structs:
    // 24 bytes in (struct __sigaction, carries sa_tramp), 16 bytes out (struct sigaction).
    #[test]
    fn decode_act_reads_24_bytes_and_ignores_sa_tramp() {
        let mut b = [0u8; 24];
        b[0..8].copy_from_slice(&0xdead_0000u64.to_le_bytes()); // handler VA
        b[8..16].copy_from_slice(&0xbeef_0000u64.to_le_bytes()); // sa_tramp — MUST be ignored
        b[16..20].copy_from_slice(&0x0000_00ffu32.to_le_bytes()); // sa_mask
        b[20..24].copy_from_slice(&0x0000_0042u32.to_le_bytes()); // sa_flags
        let a = decode_act(&b);
        assert_eq!(a.disp, Disposition::Handler(0xdead_0000));
        assert_eq!(a.mask, 0xff);
        assert_eq!(a.flags, 0x42);
    }

    #[test]
    fn decode_act_maps_sig_dfl_and_sig_ign() {
        let mut b = [0u8; 24];
        b[0..8].copy_from_slice(&0u64.to_le_bytes());
        assert_eq!(decode_act(&b).disp, Disposition::Dfl);
        b[0..8].copy_from_slice(&1u64.to_le_bytes());
        assert_eq!(decode_act(&b).disp, Disposition::Ign);
    }

    // The one that stops the 8-byte guest corruption. Every offset is pinned.
    #[test]
    fn encode_oldact_is_exactly_16_bytes_with_no_sa_tramp() {
        let out = encode_oldact(SigAction {
            disp: Disposition::Handler(0xdead_0000),
            mask: 0xff,
            flags: 0x42,
        });
        assert_eq!(out.len(), 16, "struct sigaction is 16 bytes — NOT struct __sigaction's 24");
        assert_eq!(u64::from_le_bytes(out[0..8].try_into().unwrap()), 0xdead_0000);
        assert_eq!(
            u32::from_le_bytes(out[8..12].try_into().unwrap()),
            0xff,
            "sa_mask sits at offset 8, where sa_tramp would be in the INPUT struct"
        );
        assert_eq!(u32::from_le_bytes(out[12..16].try_into().unwrap()), 0x42);
    }

    #[test]
    fn encode_oldact_round_trips_dfl_and_ign_as_0_and_1() {
        let d = encode_oldact(SigAction { disp: Disposition::Dfl, mask: 0, flags: 0 });
        assert_eq!(u64::from_le_bytes(d[0..8].try_into().unwrap()), 0);
        let i = encode_oldact(SigAction { disp: Disposition::Ign, mask: 0, flags: 0 });
        assert_eq!(u64::from_le_bytes(i[0..8].try_into().unwrap()), 1);
    }

    #[test]
    fn altstack_is_stored_and_returns_the_previous_value() {
        let mut t = SigTable::default();
        assert_eq!(t.set_altstack(Some((0x9000, 0x4000, 0))), None);
        assert_eq!(t.altstack(), Some((0x9000, 0x4000, 0)));
        assert_eq!(t.set_altstack(None), Some((0x9000, 0x4000, 0)));
    }
}
