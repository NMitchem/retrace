//! The guest's signal dispositions (M11-signals).
//!
//! **Pure guest state.** `SigTable`'s one field is a function of the guest's own `sigaction` calls,
//! so record and replay compute an identical table from an identical syscall sequence and nothing
//! here ever enters the trace. That is `FdTable::slots`' posture, and it is why this module has no
//! `Box_` and no VM in it — the whole thing is unit-testable at full speed. The blocked mask, the
//! pending set and the alternate stack used to live here too, driven by `sigprocmask`/`sigaltstack`;
//! M16 moved them to `Thread` (`thread.rs`), since POSIX makes those three per-thread while
//! dispositions stay process-wide.
//!
//! **Disposition, then delivery.** M11 modelled what the guest ASKED for and never ran a handler.
//! M12 adds the other half — `build_frame`/`choose_frame_base`, the pure layout of the frame a real
//! kernel pushes before entering `sa_tramp`. Still no `Box_` and no VM: the frame is a function of
//! its inputs alone, which is both why record and replay produce identical bytes and why the whole
//! layout is testable in microseconds.

use retrace_arch::{NSIG, SA_ONSTACK, SA_SIGINFO, SIG_DFL, SIG_IGN, SS_ONSTACK, UC_FLAVOR};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Dfl,
    Ign,
    Handler(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SigAction {
    pub disp: Disposition,
    /// `sa_tramp`, offset 8 of the 24-byte input struct. The kernel enters the TRAMPOLINE, not the
    /// handler, so delivery needs this and M11's discard of it was the missing half.
    pub tramp: u64,
    pub mask: u32,
    pub flags: u32,
}

impl Default for SigAction {
    fn default() -> Self {
        SigAction { disp: Disposition::Dfl, tramp: 0, mask: 0, flags: 0 }
    }
}

/// Per-signal disposition — process-wide, per POSIX. The blocked mask, the pending set and the
/// alternate stack used to live here too; M16 moved them to `Thread` (`thread.rs`), since POSIX
/// makes those three per-thread while dispositions stay shared across the whole process.
#[derive(Debug, Clone)]
pub struct SigTable {
    /// Indexed by signal number; `[0]` is unused so indexing mirrors signal numbering (1..=31).
    disp: [SigAction; NSIG],
}

impl Default for SigTable {
    /// All-default — which is genuinely correct for a fresh process, so there is no seeding step
    /// that could be got wrong.
    fn default() -> Self {
        SigTable { disp: [SigAction::default(); NSIG] }
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
}

/// Decode the ACT argument: `struct __sigaction`, 24 bytes (`sys/signal.h:277`).
///
/// `sa_tramp` (offset 8) is CAPTURED. M11 read past it because nothing was ever delivered; M12
/// enters it, so discarding it would leave delivery with no entry point.
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
        tramp: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
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

/// Decode the sigaltstack NEW argument: `struct sigaltstack { void *ss_sp; size_t ss_size; int
/// ss_flags; }`, 24 bytes with padding (`sys/signal.h`) — `sigaction`'s sibling syscall, and this is
/// `decode_act`'s counterpart for it. Returned as `(ss_sp, ss_size, ss_flags)`, matching
/// `Threads::altstack_of`'s `Option<(u64, u64, u64)>`; `ss_flags` widens to u64 here even though the
/// wire field is a u32, for that same reason.
pub fn decode_stack(bytes: &[u8]) -> (u64, u64, u64) {
    assert!(
        bytes.len() >= 24,
        "struct sigaltstack is 24 bytes, got {} — the caller read too few guest bytes",
        bytes.len()
    );
    (
        u64::from_le_bytes(bytes[0..8].try_into().unwrap()),
        u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
        u32::from_le_bytes(bytes[16..20].try_into().unwrap()) as u64,
    )
}

/// Encode the OLDSTACK writeback: the same 24-byte `struct sigaltstack` — unlike `sigaction`'s
/// asymmetric in/out structs, sigaltstack's NEW and OLD shapes are IDENTICAL, so this is symmetric
/// with `decode_stack` rather than narrower the way `encode_oldact` is narrower than `decode_act`.
/// `ss_flags` narrows back to its wire width; bytes 20..24 are zero padding.
///
/// One encoder called by both record and replay, exactly as `encode_oldact` already is for
/// `sigaction`: if replay re-spelled this layout by hand, a byte-compare against record's hand-rolled
/// copy would only be comparing a duplicate against itself, not proving the two sides agree.
pub fn encode_oldstack(ss: (u64, u64, u64)) -> [u8; 24] {
    let (sp, size, flags) = ss;
    let mut o = [0u8; 24];
    o[0..8].copy_from_slice(&sp.to_le_bytes());
    o[8..16].copy_from_slice(&size.to_le_bytes());
    o[16..20].copy_from_slice(&(flags as u32).to_le_bytes());
    o
}

// ---- The signal frame -------------------------------------------------------------------------

/// Frame geometry, measured by `spikes/sigabi.c` against the live SDK. The frame is ONE block at
/// the new `sp`; `spikes/sigtramp.c` measured `sp == x3`, i.e. siginfo sits at offset 0.
pub const FRAME_SIGINFO_OFF: usize = 0;
pub const FRAME_UCONTEXT_OFF: usize = 104; // == sizeof(siginfo_t)
pub const FRAME_MCONTEXT_OFF: usize = 160; // == 104 + sizeof(ucontext_t), and uc_mcontext points here
pub const FRAME_LEN: usize = 976; // 104 + 56 + 816
/// The kernel left 128 bytes between the frame top and the pre-signal `sp` (measured: old sp
/// 0x16b9c6730, frame base 0x16b9c62e0, frame 976). Reproduced rather than explained.
pub const FRAME_SLACK: u64 = 128;

const MCONTEXT_LEN: u64 = 816;
const TS_OFF: usize = 16; // thread_state64 within mcontext64
const NS_OFF: usize = 288; // neon_state64 within mcontext64

/// Fixed key for the `sigreturn` token. The host randomizes its equivalent per process — measured,
/// two runs of `spikes/sigtramp.c` returned different values — so retrace, which synthesizes the
/// whole frame, must own it as a CONSTANT. Same posture as the fixed PAC keys.
const SIGRETURN_TOKEN_KEY: u64 = 0x5265_7472_6163_6512;

pub fn sigreturn_token(uctx_ipa: u64) -> u64 {
    SIGRETURN_TOKEN_KEY ^ uctx_ipa.rotate_left(17)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadState {
    pub x: [u64; 29],
    pub fp: u64,
    pub lr: u64,
    pub sp: u64,
    pub pc: u64,
    pub cpsr: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NeonState {
    pub v: [u128; 32],
    pub fpsr: u32,
    pub fpcr: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct FrameInput {
    pub sig: u64,
    pub si_code: u64,
    pub si_addr: u64,
    pub esr: u64,
    pub far: u64,
    pub ts: ThreadState,
    pub ns: NeonState,
    pub mask: u32,
    pub act: SigAction,
    pub frame_base: u64,
    /// `choose_frame_base`'s second return, fed back in. It is what `uc_onstack` reports, and the
    /// only reason the frame needs it: a handler that queries `sigaltstack(NULL, &old)` — or a libc
    /// that consults `uc_onstack` on the way out — reads this field, so hardcoding it to 0 would be
    /// a lie on exactly the alt-stack path `choose_frame_base` exists to support.
    pub on_alt: bool,
}

/// What the vCPU must be set to on entry. `pc` is the TRAMPOLINE, never the handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryRegs {
    pub x: [u64; 6],
    pub sp: u64,
    pub pc: u64,
}

/// Where the frame goes, and whether the handler runs on the alternate stack.
///
/// `on_alt` in means "the guest is ALREADY running on its alt stack" — in that case the frame grows
/// down from the current `sp` instead of resetting to the alt stack's top, which would clobber the
/// frame an outer handler is still using.
pub fn choose_frame_base(
    sp: u64,
    act: SigAction,
    altstack: Option<(u64, u64, u64)>,
    on_alt: bool,
) -> (u64, bool) {
    let wants_alt = act.flags & SA_ONSTACK != 0;
    match (wants_alt, altstack, on_alt) {
        (true, Some((ss_sp, ss_size, _)), false) => {
            (((ss_sp + ss_size) - FRAME_LEN as u64) & !15, true)
        }
        (true, Some(_), true) => ((sp - FRAME_SLACK - FRAME_LEN as u64) & !15, true),
        _ => ((sp - FRAME_SLACK - FRAME_LEN as u64) & !15, false),
    }
}

/// Lay out the signal frame. Pure: no VM, no `Box_`, no I/O — every byte is a function of the
/// inputs, which is what makes record and replay produce identical frames and what lets the whole
/// layout be tested in microseconds.
pub fn build_frame(inp: &FrameInput) -> (Vec<u8>, EntryRegs) {
    let mut f = vec![0u8; FRAME_LEN];
    let base = inp.frame_base;
    let uc = base + FRAME_UCONTEXT_OFF as u64;
    let mc = base + FRAME_MCONTEXT_OFF as u64;

    // --- siginfo_t at +0 ---
    let si = FRAME_SIGINFO_OFF;
    f[si..si + 4].copy_from_slice(&(inp.sig as u32).to_le_bytes()); // si_signo
    f[si + 8..si + 12].copy_from_slice(&(inp.si_code as u32).to_le_bytes()); // si_code
    f[si + 24..si + 32].copy_from_slice(&inp.si_addr.to_le_bytes()); // si_addr

    // --- ucontext_t at +104. uc_mcontext is a POINTER; the mcontext is the block at +160. ---
    let u = FRAME_UCONTEXT_OFF;
    let onstack = if inp.on_alt { SS_ONSTACK as u32 } else { 0 };
    f[u..u + 4].copy_from_slice(&onstack.to_le_bytes()); // uc_onstack
    f[u + 4..u + 8].copy_from_slice(&inp.mask.to_le_bytes()); // uc_sigmask
    f[u + 40..u + 48].copy_from_slice(&MCONTEXT_LEN.to_le_bytes()); // uc_mcsize
    f[u + 48..u + 56].copy_from_slice(&mc.to_le_bytes()); // uc_mcontext

    // --- mcontext64 at +160: exception(16) | thread(272) | neon(528) ---
    let m = FRAME_MCONTEXT_OFF;
    f[m..m + 8].copy_from_slice(&inp.far.to_le_bytes()); // __es.__far
    f[m + 8..m + 12].copy_from_slice(&(inp.esr as u32).to_le_bytes()); // __es.__esr

    let t = m + TS_OFF;
    for (i, xi) in inp.ts.x.iter().enumerate() {
        f[t + i * 8..t + i * 8 + 8].copy_from_slice(&xi.to_le_bytes());
    }
    f[t + 232..t + 240].copy_from_slice(&inp.ts.fp.to_le_bytes());
    f[t + 240..t + 248].copy_from_slice(&inp.ts.lr.to_le_bytes());
    f[t + 248..t + 256].copy_from_slice(&inp.ts.sp.to_le_bytes());
    f[t + 256..t + 264].copy_from_slice(&inp.ts.pc.to_le_bytes());
    f[t + 264..t + 268].copy_from_slice(&(inp.ts.cpsr as u32).to_le_bytes());

    let n = m + NS_OFF;
    for (i, vi) in inp.ns.v.iter().enumerate() {
        f[n + i * 16..n + i * 16 + 16].copy_from_slice(&vi.to_le_bytes());
    }
    f[n + 512..n + 516].copy_from_slice(&inp.ns.fpsr.to_le_bytes());
    f[n + 516..n + 520].copy_from_slice(&inp.ns.fpcr.to_le_bytes());

    let catcher = match inp.act.disp {
        Disposition::Handler(va) => va,
        other => panic!(
            "build_frame called for disposition {other:?} — only Handler has anything to deliver \
             to. The caller's disposition check is wrong."
        ),
    };
    // infostyle: measured 0x1e (UC_FLAVOR) for an SA_SIGINFO handler and 0x1 for one without
    // (Task 1 Step 0, spikes/sigtramp.c). Only the SA_SIGINFO shape is MODELLED — the frame layout
    // is identical either way, but no guest in the gate set installs a non-SA_SIGINFO handler, so
    // delivering to one would ship an untested path. Assert rather than guess.
    assert!(
        inp.act.flags & SA_SIGINFO != 0,
        "a non-SA_SIGINFO handler is not modelled. Its infostyle is 0x1 (measured, vs 0x1e for \
         SA_SIGINFO) and the frame layout is identical, so supporting it is small — but no gate \
         guest exercises it, so it is asserted rather than shipped untested."
    );
    let regs = EntryRegs {
        x: [catcher, UC_FLAVOR, inp.sig, base, uc, sigreturn_token(uc)],
        sp: base,
        pc: inp.act.tramp,
    };
    (f, regs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_table_is_all_default() {
        let t = SigTable::default();
        for sig in 1..32u64 {
            assert_eq!(t.action(sig).disp, Disposition::Dfl, "sig {sig}");
        }
    }

    #[test]
    fn set_action_returns_the_previous_action() {
        let mut t = SigTable::default();
        let old = t.set_action(6, SigAction { disp: Disposition::Ign, tramp: 0, mask: 0, flags: 0 });
        assert_eq!(old.disp, Disposition::Dfl, "first install returns the default");
        let old = t.set_action(6, SigAction { disp: Disposition::Handler(0x1_0000), tramp: 0, mask: 3, flags: 4 });
        assert_eq!(old.disp, Disposition::Ign, "second install returns what the first set");
        assert_eq!(
            t.action(6),
            SigAction { disp: Disposition::Handler(0x1_0000), tramp: 0, mask: 3, flags: 4 }
        );
    }

    // THE golden test. sigaction(2)'s in-param and out-param are different C structs:
    // 24 bytes in (struct __sigaction, carries sa_tramp), 16 bytes out (struct sigaction).
    // It pins where mask/flags LIVE — at 16 and 20, i.e. past sa_tramp. M12 also reads sa_tramp
    // itself; that is `decode_act_now_captures_sa_tramp`.
    #[test]
    fn decode_act_reads_mask_and_flags_from_past_sa_tramp() {
        let mut b = [0u8; 24];
        b[0..8].copy_from_slice(&0xdead_0000u64.to_le_bytes()); // handler VA
        b[8..16].copy_from_slice(&0xbeef_0000u64.to_le_bytes()); // sa_tramp — mask must not come from here
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
            tramp: 0,
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
        let d = encode_oldact(SigAction { disp: Disposition::Dfl, tramp: 0, mask: 0, flags: 0 });
        assert_eq!(u64::from_le_bytes(d[0..8].try_into().unwrap()), 0);
        let i = encode_oldact(SigAction { disp: Disposition::Ign, tramp: 0, mask: 0, flags: 0 });
        assert_eq!(u64::from_le_bytes(i[0..8].try_into().unwrap()), 1);
    }

    // ---- Fast-follow: sigaltstack's decode/encode pair -----------------------------------------

    #[test]
    fn decode_stack_and_encode_oldstack_round_trip() {
        let mut b = [0u8; 24];
        b[0..8].copy_from_slice(&0x7000_0000u64.to_le_bytes());
        b[8..16].copy_from_slice(&0x4000u64.to_le_bytes());
        b[16..20].copy_from_slice(&0x1u32.to_le_bytes());
        let ss = decode_stack(&b);
        assert_eq!(ss, (0x7000_0000, 0x4000, 0x1));
        let out = encode_oldstack(ss);
        assert_eq!(
            out[0..20], b[0..20],
            "round trip must reproduce the same bytes decode_stack read"
        );
    }

    // The counterpart to `encode_oldact_is_exactly_16_bytes_with_no_sa_tramp`: pins the offsets and
    // the zero tail padding so record and replay cannot silently drift onto different layouts.
    #[test]
    fn encode_oldstack_is_24_bytes_with_ss_flags_at_16_and_zero_padding() {
        let out = encode_oldstack((0x9_0000, 0x2000, SS_ONSTACK));
        assert_eq!(out.len(), 24);
        assert_eq!(u64::from_le_bytes(out[0..8].try_into().unwrap()), 0x9_0000, "ss_sp at 0");
        assert_eq!(u64::from_le_bytes(out[8..16].try_into().unwrap()), 0x2000, "ss_size at 8");
        assert_eq!(
            u32::from_le_bytes(out[16..20].try_into().unwrap()),
            SS_ONSTACK as u32,
            "ss_flags at 16, narrowed to u32"
        );
        assert_eq!(&out[20..24], &[0u8; 4], "bytes 20..24 are padding and must be zero");
    }

    // ---- M12: the frame builder ---------------------------------------------------------------

    fn probe_ts() -> ThreadState {
        let mut x = [0u64; 29];
        for (i, xi) in x.iter_mut().enumerate() {
            *xi = 0x1000 + i as u64;
        }
        ThreadState { x, fp: 0xf000, lr: 0x1_0000, sp: 0x7fff_0000, pc: 0x1_2340, cpsr: 0x6000_0000 }
    }

    fn probe_input(base: u64) -> FrameInput {
        FrameInput {
            sig: 11,
            si_code: 1,
            si_addr: 0xdead_0000,
            esr: 0x9200_0046,
            far: 0xdead_0000,
            ts: probe_ts(),
            ns: NeonState { v: [0; 32], fpsr: 0, fpcr: 0 },
            mask: 0,
            act: SigAction {
                disp: Disposition::Handler(0xabc0),
                tramp: 0xdef0,
                mask: 0,
                flags: SA_SIGINFO,
            },
            frame_base: base,
            on_alt: false,
        }
    }

    #[test]
    fn frame_offsets_match_the_measured_layout() {
        // spikes/sigabi.c: siginfo_t=104, ucontext_t=56 (uc_mcontext is a POINTER), mcontext64=816.
        assert_eq!((FRAME_SIGINFO_OFF, FRAME_UCONTEXT_OFF, FRAME_MCONTEXT_OFF), (0, 104, 160));
        assert_eq!(FRAME_LEN, 976, "104 + 56 + 816");
    }

    #[test]
    fn build_frame_lays_out_siginfo_at_offset_zero() {
        let (bytes, _) = build_frame(&probe_input(0x7000_0000));
        assert_eq!(bytes.len(), FRAME_LEN);
        let si = &bytes[FRAME_SIGINFO_OFF..];
        assert_eq!(u32::from_le_bytes(si[0..4].try_into().unwrap()), 11, "si_signo at 0");
        assert_eq!(u32::from_le_bytes(si[8..12].try_into().unwrap()), 1, "si_code at 8");
        assert_eq!(u64::from_le_bytes(si[24..32].try_into().unwrap()), 0xdead_0000, "si_addr at 24");
    }

    #[test]
    fn build_frame_points_uc_mcontext_at_the_separate_block() {
        let base = 0x7000_0000u64;
        let (bytes, _) = build_frame(&probe_input(base));
        let uc = &bytes[FRAME_UCONTEXT_OFF..];
        assert_eq!(u32::from_le_bytes(uc[0..4].try_into().unwrap()), 0, "uc_onstack at 0");
        assert_eq!(u32::from_le_bytes(uc[4..8].try_into().unwrap()), 0, "uc_sigmask at 4");
        assert_eq!(u64::from_le_bytes(uc[40..48].try_into().unwrap()), 816, "uc_mcsize at 40");
        assert_eq!(
            u64::from_le_bytes(uc[48..56].try_into().unwrap()),
            base + FRAME_MCONTEXT_OFF as u64,
            "uc_mcontext at 48 is a POINTER to the mcontext block, not the mcontext itself"
        );
    }

    // `choose_frame_base` decides the handler runs on the alt stack; `uc_onstack` is the field that
    // TELLS the guest so. Hardcoding it to 0 would make `sigaltstack(NULL, &old)` inside a handler
    // report the thread is on its normal stack while it is demonstrably not.
    #[test]
    fn build_frame_reports_uc_onstack_when_the_frame_is_on_the_alt_stack() {
        let base = 0x9_0000u64;
        let mut inp = probe_input(base);
        inp.on_alt = true;
        let (bytes, _) = build_frame(&inp);
        assert_eq!(
            u32::from_le_bytes(bytes[FRAME_UCONTEXT_OFF..FRAME_UCONTEXT_OFF + 4].try_into().unwrap()),
            SS_ONSTACK as u32,
            "uc_onstack must mirror choose_frame_base's on_alt"
        );
    }

    #[test]
    fn build_frame_writes_the_exception_and_thread_state() {
        let base = 0x7000_0000u64;
        let (bytes, _) = build_frame(&probe_input(base));
        let mc = &bytes[FRAME_MCONTEXT_OFF..];
        // exception_state64 at mcontext+0: far(8) esr(4) exception(4)
        assert_eq!(u64::from_le_bytes(mc[0..8].try_into().unwrap()), 0xdead_0000, "__es.__far");
        assert_eq!(u32::from_le_bytes(mc[8..12].try_into().unwrap()), 0x9200_0046, "__es.__esr");
        assert_eq!(u32::from_le_bytes(mc[12..16].try_into().unwrap()), 0, "__es.__exception");
        // thread_state64 at mcontext+16: x[29] then fp,lr,sp,pc at 232,240,248,256 and cpsr at 264
        let ss = &mc[16..];
        assert_eq!(u64::from_le_bytes(ss[0..8].try_into().unwrap()), 0x1000, "__ss.__x[0]");
        assert_eq!(u64::from_le_bytes(ss[224..232].try_into().unwrap()), 0x1000 + 28, "__ss.__x[28]");
        assert_eq!(u64::from_le_bytes(ss[232..240].try_into().unwrap()), 0xf000, "__ss.__fp");
        assert_eq!(u64::from_le_bytes(ss[240..248].try_into().unwrap()), 0x1_0000, "__ss.__lr");
        assert_eq!(u64::from_le_bytes(ss[248..256].try_into().unwrap()), 0x7fff_0000, "__ss.__sp");
        assert_eq!(u64::from_le_bytes(ss[256..264].try_into().unwrap()), 0x1_2340, "__ss.__pc");
        assert_eq!(u32::from_le_bytes(ss[264..268].try_into().unwrap()), 0x6000_0000, "__ss.__cpsr");
    }

    #[test]
    fn build_frame_writes_the_neon_block() {
        let base = 0x7000_0000u64;
        let mut inp = probe_input(base);
        inp.ns.v[8] = 0x1122_3344_5566_7788_99aa_bbcc_ddee_ff00;
        inp.ns.fpsr = 0x1234;
        inp.ns.fpcr = 0x5678;
        let (bytes, _) = build_frame(&inp);
        // neon_state64 at mcontext+288: v[32] (16 bytes each) then fpsr(4) fpcr(4)
        let ns = &bytes[FRAME_MCONTEXT_OFF + 288..];
        assert_eq!(
            u128::from_le_bytes(ns[128..144].try_into().unwrap()),
            0x1122_3344_5566_7788_99aa_bbcc_ddee_ff00,
            "v8 at neon+8*16"
        );
        assert_eq!(u32::from_le_bytes(ns[512..516].try_into().unwrap()), 0x1234, "fpsr");
        assert_eq!(u32::from_le_bytes(ns[516..520].try_into().unwrap()), 0x5678, "fpcr");
    }

    // THE entry-contract test. Measured in spikes/sigtramp.c: sp IS the siginfo pointer.
    #[test]
    fn build_frame_returns_the_measured_entry_registers() {
        let base = 0x7000_0000u64;
        let (_, regs) = build_frame(&probe_input(base));
        assert_eq!(regs.x[0], 0xabc0, "x0 = the catcher (handler VA)");
        assert_eq!(regs.x[1], 30, "x1 = infostyle UC_FLAVOR");
        assert_eq!(regs.x[2], 11, "x2 = the signal number");
        assert_eq!(regs.x[3], base, "x3 = siginfo*, which is the frame base");
        assert_eq!(regs.x[4], base + FRAME_UCONTEXT_OFF as u64, "x4 = ucontext*");
        assert_eq!(regs.x[5], sigreturn_token(base + FRAME_UCONTEXT_OFF as u64), "x5 = the token");
        assert_eq!(regs.sp, base, "sp == x3: the frame base IS sp");
        assert_eq!(regs.pc, 0xdef0, "pc = sa_tramp, NOT the handler — the kernel enters the trampoline");
    }

    #[test]
    fn choose_frame_base_uses_the_current_stack_by_default() {
        let act = SigAction { disp: Disposition::Handler(1), tramp: 2, mask: 0, flags: 0 };
        let (base, on_alt) = choose_frame_base(0x7fff_1000, act, None, false);
        // 976-byte frame + 128 bytes of measured slack below the pre-signal sp, 16-byte aligned.
        assert_eq!(base, 0x7fff_1000 - 128 - 976);
        assert_eq!(base % 16, 0, "arm64 requires a 16-byte aligned sp");
        assert!(!on_alt);
    }

    #[test]
    fn choose_frame_base_honours_sa_onstack_when_an_altstack_is_installed() {
        let act = SigAction { disp: Disposition::Handler(1), tramp: 2, mask: 0, flags: SA_ONSTACK };
        let (base, on_alt) = choose_frame_base(0x7fff_1000, act, Some((0x9_0000, 0x4000, 0)), false);
        assert!(on_alt, "SA_ONSTACK + an installed alt stack means run on it");
        assert!(
            (0x9_0000..0x9_0000 + 0x4000).contains(&base),
            "the frame must sit INSIDE the alt stack [{:#x}, {:#x})",
            0x9_0000,
            0x9_0000 + 0x4000
        );
        assert_eq!(base, (0x9_0000 + 0x4000 - FRAME_LEN as u64) & !15);
    }

    #[test]
    fn choose_frame_base_does_not_re_enter_an_altstack_it_is_already_on() {
        let act = SigAction { disp: Disposition::Handler(1), tramp: 2, mask: 0, flags: SA_ONSTACK };
        let sp_on_alt = 0x9_2000;
        let (base, on_alt) = choose_frame_base(sp_on_alt, act, Some((0x9_0000, 0x4000, 0)), true);
        assert!(on_alt, "still on the alt stack");
        assert_eq!(
            base,
            sp_on_alt - 128 - 976,
            "already on it: keep growing DOWN from the current sp, do not reset to its top and \
             clobber the frame the outer handler is running on"
        );
    }

    #[test]
    fn choose_frame_base_ignores_sa_onstack_with_no_altstack_installed() {
        let act = SigAction { disp: Disposition::Handler(1), tramp: 2, mask: 0, flags: SA_ONSTACK };
        let (base, on_alt) = choose_frame_base(0x7fff_1000, act, None, false);
        assert_eq!(base, 0x7fff_1000 - 128 - 976);
        assert!(!on_alt);
    }

    #[test]
    fn sigreturn_token_is_deterministic_and_address_dependent() {
        assert_eq!(
            sigreturn_token(0x7000_0068),
            sigreturn_token(0x7000_0068),
            "a CONSTANT, unlike the host's process-randomized token: spikes/sigtramp.c returned a \
             different value on every run, which is exactly what must not enter a recording"
        );
        assert_ne!(sigreturn_token(0x7000_0068), sigreturn_token(0x7000_0078));
    }

    #[test]
    fn decode_act_now_captures_sa_tramp() {
        let mut b = [0u8; 24];
        b[0..8].copy_from_slice(&0xdead_0000u64.to_le_bytes());
        b[8..16].copy_from_slice(&0xbeef_0000u64.to_le_bytes()); // sa_tramp — M11 discarded it
        let a = decode_act(&b);
        assert_eq!(a.disp, Disposition::Handler(0xdead_0000));
        assert_eq!(a.tramp, 0xbeef_0000, "M12 needs it: the kernel enters the TRAMPOLINE, not the handler");
    }

    // The counterpart to M11's width test: capturing tramp must not widen the writeback.
    #[test]
    fn encode_oldact_still_omits_sa_tramp() {
        let out = encode_oldact(SigAction {
            disp: Disposition::Handler(0xdead_0000),
            tramp: 0xbeef_0000,
            mask: 0xff,
            flags: 0x42,
        });
        assert_eq!(out.len(), 16, "struct sigaction is 16 bytes and has NO sa_tramp");
        assert_eq!(
            u32::from_le_bytes(out[8..12].try_into().unwrap()),
            0xff,
            "sa_mask sits at offset 8 — if tramp leaked in here it would land at 8 and corrupt it"
        );
    }
}
