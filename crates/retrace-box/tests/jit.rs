//! M48 Task 6, box level: the SPRR register, the `MAP_JIT` view over stage 1, and the step-safe
//! flush, driven through `Box_`'s public methods on a static box (the `checkpointparity.rs`
//! pattern). A static guest has no commpage (Ruling T6-f), so `tb()` stages one where
//! `load_dynamic` freezes the host's, holding the three words `pthread_jit_write_protect_np`
//! reads (§2c, `sprr.out`).
use retrace_box::{jit::View, Box_, COMMPAGE_IPA};

const ANON: u64 = 0x1002;         // MAP_ANON | MAP_PRIVATE
const FIXED: u64 = 0x10;          // MAP_FIXED
const JIT_RWX: u64 = 0x1802;      // MAP_JIT | MAP_ANON | MAP_PRIVATE, sprr.c's page
const V8_FLAGS: u64 = 0x41842;    // V8's reservation (P5)
const WE: u64 = 0x2010_0020_3030_0000; // commpage +0x110 on the probe host: write-enable
const PR: u64 = 0x2010_0020_3010_0000; // +0x118: protect

fn tb() -> Box_ {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    assert_eq!(b.guest_mmap(COMMPAGE_IPA, 0x4000, 3, ANON | FIXED), Ok(COMMPAGE_IPA));
    b.poke_guest(COMMPAGE_IPA + 0x10c, &[3]); // SPRR (§2c)
    b.poke_guest(COMMPAGE_IPA + 0x110, &WE.to_le_bytes());
    b.poke_guest(COMMPAGE_IPA + 0x118, &PR.to_le_bytes());
    b
}

/// Review Focus item 4, and its primary pin. V8 maps its code range `PROT_NONE` with `MAP_JIT`,
/// then `mprotect`s a sub-range RWX. `guest_mprotect` routes that through `unprotect`, which stamps
/// `ATTR_DATA` unconditionally: without the view's restamp the committed range is writable and
/// non-executable under the protected view until the next toggle restamps it. No natively valid flow
/// calls into it before that toggle (a committed MAP_JIT page is never re-protected, t6-jitprobe, so
/// its code is written after a write-enable toggle), so `jitwp_e2e`'s `v8` mode cannot go red when
/// the restamp is missing. This test does.
#[test]
fn an_unprotect_inside_a_jit_range_is_restamped_by_the_view_not_left_data() {
    let mut b = tb();
    let a = b.guest_mmap(0, 0x10_0000, 0, V8_FLAGS).unwrap();
    assert_eq!(b.dbg_jit().ranges(), &[(a, 0x10_0000)], "the MAP_JIT range is tracked");
    assert!(b.ipa_is_noaccess(a) && b.ipa_is_noaccess(a + 0xf_c000), "mapped PROT_NONE whole");
    // A committed sub-range inside the PROT_NONE reservation, as V8 maps it. The `+0x40000` head
    // is a fixture shape: V8's own commit starts at the first 256 KiB boundary past the base and
    // leaves a PROT_NONE tail too (t0 M5, Ruling T6-LAYOUT).
    b.guest_mprotect(a + 0x4_0000, 0xc_0000, 7);
    assert!(b.ipa_is_noaccess(a + 0x3_c000), "the uncommitted head keeps M13's ATTR_NONE");
    assert!(b.ipa_is_exec(a + 0x4_0000) && b.ipa_is_exec(a + 0xf_c000),
        "protected view: the committed range is ATTR_CODE, not the ATTR_DATA unprotect stamped");
    assert!(!b.ipa_is_el0_writable(a + 0x4_0000), "and not writable");
    b.sprr_write(WE);
    assert_eq!(b.dbg_jit().view(), View::Rw, "the write-enable flipped the view");
    assert!(b.ipa_is_el0_writable(a + 0x4_0000) && !b.ipa_is_exec(a + 0x4_0000), "write-enabled: ATTR_DATA");
    assert!(b.ipa_is_noaccess(a + 0x3_c000), "the view never stamps a no-access page");
    b.sprr_write(PR);
    assert!(b.ipa_is_exec(a + 0x4_0000) && !b.ipa_is_el0_writable(a + 0x4_0000), "protected again");
    assert_eq!(b.threads().sprr_of(0), PR, "the register holds what was written, for the read-back");
}

/// §3f "Flips": the view is the RUNNING thread's mode. A switch to a thread in the other mode
/// flips it, and the switch back restores it, with no write in between.
#[test]
fn the_view_follows_the_running_thread_across_a_switch() {
    let mut b = tb();
    let a = b.guest_mmap(0, 0x4000, 7, JIT_RWX).unwrap();
    assert!(b.ipa_is_exec(a), "a new range carries the current view: protected (R1)");
    // A second thread from the live vCPU, the checkpointparity.rs pattern.
    let mut child = b.save_ctx();
    child.regs.sp_el0 -= 0x2000;
    let tid = b.threads_mut().spawn(child, (0, 0));
    b.sprr_write(WE);
    assert!(b.ipa_is_el0_writable(a), "main's own write flips the view");
    b.switch_to_thread(tid);
    assert_eq!(b.threads().sprr_of(tid), 0, "the child was never write-enabled (R1)");
    assert!(b.ipa_is_exec(a) && !b.ipa_is_el0_writable(a), "a switch to a protected thread flips back");
    b.switch_to_thread(0);
    assert!(b.ipa_is_el0_writable(a), "and the switch back restores main's mode");
}

/// P9 (walls.md §1, §4 item 2): a view flip inside `step()` runs `flush_guest_tlb` with
/// `MDSCR_EL1.SS` armed. Before M48 the EL1 stub then took a software-step exception and panicked
/// (`tlbi stub faulted at EL1: EC=SoftStep`).
#[test]
fn a_flush_with_the_step_bits_armed_does_not_step_the_stub() {
    let mut b = tb();
    b.dbg_leak_ss(); // MDSCR_EL1.SS and PSTATE.SS, armed exactly as step() arms them
    let before = b.regs_snapshot();
    b.flush_guest_tlb();
    assert_eq!(b.regs_snapshot(), before, "every register restored, PSTATE.SS included");
    assert_eq!(b.dbg_watch0_hw().2 & 1, 1, "MDSCR_EL1.SS restored for the step that armed it");
}

/// R2: only the commpage's two words are admitted. The refusal names the value and the pc.
#[test]
#[should_panic(expected = "M48: SPRR write 0x1 at pc ")]
fn an_inadmissible_sprr_value_is_refused_by_value() {
    let mut b = tb();
    b.sprr_write(1);
}

/// Ruling T6-d: a munmap of a JIT range trims it, and the released pages go back to `ATTR_DATA`,
/// so a later ordinary mapping there does not inherit an `ATTR_CODE` leaf (M13's argument). Ruling
/// T6-c: a FIXED map over what remains is refused by value before anything changes. This is the
/// one test file that names the `M48: MAP_JIT ` prefix (Task 10's audit, check 9); the other
/// `MAP_JIT` refusals are pinned by `jit.rs`'s unit tests.
#[test]
fn a_munmap_trims_a_jit_range_to_data_pages_and_a_fixed_map_over_one_is_refused() {
    let mut b = tb();
    let a = b.guest_mmap(0, 0x1_0000, 7, JIT_RWX).unwrap();
    assert!(b.ipa_is_exec(a) && b.ipa_is_exec(a + 0xc000), "protected view over the whole range");
    b.guest_munmap(a, 0x4000); // a head trim
    assert_eq!(b.dbg_jit().ranges(), &[(a + 0x4000, 0xc000)]);
    assert!(b.ipa_is_el0_writable(a) && !b.ipa_is_exec(a), "the released page is back to ATTR_DATA");
    assert!(b.ipa_is_exec(a + 0x4000), "the rest keeps the view");
    // The refusal is a panic (R5). `place_fixed` raises it before it touches anything, so the box
    // stays usable; the unwind leaks only the fresh host pages `guest_mmap` allocated.
    let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        b.guest_mmap(a + 0x4000, 0x4000, 3, ANON | FIXED)
    })).expect_err("a FIXED map over a MAP_JIT range must be refused");
    let msg = refused.downcast_ref::<String>().expect("a formatted refusal");
    assert!(msg.starts_with(&format!("M48: MAP_JIT range [{:#x}, {:#x}) overlapped by a FIXED mapping at ",
        a + 0x4000, a + 0x1_0000)), "{msg}");
    assert_eq!(b.dbg_jit().ranges(), &[(a + 0x4000, 0xc000)], "a refusal changes nothing");
    assert!(b.ipa_is_exec(a + 0x4000), "and leaves the view stamped");
    b.guest_munmap(a + 0x4000, 0xc000);
    assert!(b.dbg_jit().ranges().is_empty(), "the whole range is gone");
    assert_eq!(b.guest_mmap(a, 0x1_0000, 3, ANON | FIXED), Ok(a));
    assert!(b.ipa_is_el0_writable(a + 0xc000) && !b.ipa_is_exec(a + 0xc000),
        "a data map over the old range is plain data");
}

/// Ruling T6-EACCES (t6-jitprobe): natively a committed MAP_JIT page refuses every `mprotect` with
/// EACCES and nothing changes. Retrace refuses it by value, before anything changes, rather than
/// answer 0 where native answers EACCES.
#[test]
fn an_mprotect_over_a_committed_jit_page_is_refused_by_value_and_changes_nothing() {
    let mut b = tb();
    let a = b.guest_mmap(0, 0x10_0000, 0, V8_FLAGS).unwrap();
    b.guest_mprotect(a + 0x4_0000, 0xc_0000, 7); // the commit, over PROT_NONE: admitted (T6-a)
    let noaccess = b.noaccess().to_vec();
    let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        b.guest_mprotect(a + 0x8_0000, 0x4000, 0)
    })).expect_err("an mprotect over a committed MAP_JIT page must be refused");
    let msg = refused.downcast_ref::<String>().expect("a formatted refusal");
    assert!(msg.starts_with(&format!("M48: MAP_JIT mprotect [{:#x}, +0x4000) prot 0x0 over the committed extent \
        [{:#x}, {:#x}): native answers EACCES (measured, t6-jitprobe), unmodelled", a + 0x8_0000, a + 0x4_0000,
        a + 0x10_0000)), "{msg}");
    assert_eq!(b.noaccess(), noaccess.as_slice(), "a refusal changes nothing: no page went PROT_NONE");
    assert!(b.ipa_is_exec(a + 0x8_0000), "and the committed page keeps the protected view");
}
