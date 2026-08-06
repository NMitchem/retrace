// M11 t6, in the shape of pacposture.rs's from_checkpoint_carries_the_posture and M10 t4's
// fd-slot carriage. State a mid-run capture cannot re-derive must be CARRIED.
//
// If from_checkpoint installed a fresh SigTable, a seeked session would believe every signal is at
// its default disposition — so a post-seek raise of an IGNORED signal would terminate the guest,
// and reverse execution would diverge from the forward run. That is the same failure shape the fd
// slots and pac_enabled exist to prevent, and this is the fourth field to exist for that reason.
use retrace_box::{Box_, Disposition, SigAction};
use retrace_guest::{parse_macho, HELLO};

#[test]
fn from_checkpoint_carries_the_signal_table() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load_with_pac(&loaded, false);
    let _ = b.run(); // reach the first syscall so the checkpoint is genuinely mid-run

    b.sigtable_mut().set_action(6, SigAction { disp: Disposition::Ign, mask: 0xf, flags: 0x2 });
    b.sigtable_mut().set_mask(retrace_arch::SIG_SETMASK, 0b1010);
    b.sigtable_mut().set_altstack(Some((0x9000, 0x4000, 0)));

    let st = b.checkpoint();
    drop(b); // one VM per process — tear down before from_checkpoint creates the next one
    let r = Box_::from_checkpoint(&st);

    assert_eq!(r.sigtable().action(6),
               SigAction { disp: Disposition::Ign, mask: 0xf, flags: 0x2 },
               "a seek must not resurrect a default disposition");
    assert_eq!(r.sigtable().mask(), 0b1010, "the blocked mask must survive the restore");
    assert_eq!(r.sigtable().altstack(), Some((0x9000, 0x4000, 0)));
}
