// M11 t6, in the shape of pacposture.rs's from_checkpoint_carries_the_posture and M10 t4's
// fd-slot carriage. State a mid-run capture cannot re-derive must be CARRIED.
//
// If from_checkpoint installed a fresh SigTable, a seeked session would believe every signal is at
// its default disposition — so a post-seek raise of an IGNORED signal would terminate the guest,
// and reverse execution would diverge from the forward run. That is the same failure shape the fd
// slots and pac_enabled exist to prevent, and this is the fourth field to exist for that reason.
//
// M16 split the blocked mask, the pending set and the alternate stack off SigTable and onto Thread
// (they are per-thread; disposition stays process-wide) — `BoxState` already carries `threads`
// wholesale, so this test's mask/altstack assertions moved to `r.threads().mask_of(0)`/
// `altstack_of(0)` rather than being deleted, which is what proves the split didn't quietly drop the
// carriage. Fix round 2 (review finding 5) added the third field, `pending`: nothing writes it yet
// outside tests, but it is carried by the same wholesale clone as mask and altstack, and it is the
// field a seeked session losing would be hardest to notice — a pending signal that quietly evaporates
// across a seek is a divergence with no obvious cause, so it is proven here rather than deferred to
// whichever later task first starts pending real signals.
use retrace_box::{Box_, Disposition, SigAction};
use retrace_guest::{parse_macho, HELLO};

#[test]
fn from_checkpoint_carries_the_signal_table_and_per_thread_signal_state() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load_with_pac(&loaded, false);
    let _ = b.run(); // reach the first syscall so the checkpoint is genuinely mid-run

    b.sigtable_mut().set_action(6, SigAction { disp: Disposition::Ign, tramp: 0, mask: 0xf, flags: 0x2 });
    b.threads_mut().set_mask_of(0, retrace_arch::SIG_SETMASK, 0b1010);
    b.threads_mut().set_altstack_of(0, Some((0x9000, 0x4000, 0)));
    b.threads_mut().pend(0, 17); // arbitrary signal, distinct from the mask bits above

    let st = b.checkpoint();
    drop(b); // one VM per process — tear down before from_checkpoint creates the next one
    let r = Box_::from_checkpoint(&st);

    assert_eq!(r.sigtable().action(6),
               SigAction { disp: Disposition::Ign, tramp: 0, mask: 0xf, flags: 0x2 },
               "a seek must not resurrect a default disposition");
    assert_eq!(r.threads().mask_of(0), 0b1010, "the blocked mask must survive the restore");
    assert_eq!(r.threads().altstack_of(0), Some((0x9000, 0x4000, 0)));
    assert_eq!(r.threads().pending_of(0), 1 << 16,
               "a pending signal must survive the restore too, or a seek could silently drop it");
}
