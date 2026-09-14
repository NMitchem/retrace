// M37 fix round 1 (review I2). The two console predicates against a real `Box_`, after `dup2`:
// an alias of stdout is a console WRITE but not a console CLOSE (its host mapping is a dup, so
// its close goes the generic way on both sides — host dup closed, slot retired); a displaced
// console slot is neither. Only the identity slots (0/1/2 still `Console(n)` at index n) have
// their close faked, because only those host descriptors are retrace's own.
//
// A static guest box (`Box_::load`) is enough: the predicates read the fd table, never guest
// memory. One VM per process — this file creates one box per test, sequentially.
use retrace_arch::{SYS_CLOSE, SYS_CLOSE_NOCANCEL, SYS_WRITE, SYS_WRITE_NOCANCEL};
use retrace_box::{Box_, FdSlot};
use retrace_guest::{parse_macho, HELLO};

fn fresh_box() -> Box_ {
    Box_::load(&parse_macho(&std::fs::read(HELLO).unwrap()))
}

#[test]
fn an_alias_of_stdout_is_a_console_write_but_closes_the_generic_way() {
    let mut b = fresh_box();
    for num in [SYS_CLOSE, SYS_CLOSE_NOCANCEL] {
        assert!(b.is_console_close(num, 1), "close(1) on the identity slot is faked");
        assert!(b.is_console_close(num, 0), "close(0) on the identity slot is faked");
        assert!(!b.is_console_close(num, 3), "an ordinary fd is not the console");
    }
    // dup2(1, 17): 17 becomes an alias of stdout.
    assert_eq!(b.fds_mut().dup2(1, 17, Some(41)).unwrap(), (17, None));
    assert_eq!(b.fds().slots()[17], FdSlot::Console(1));
    for num in [SYS_WRITE, SYS_WRITE_NOCANCEL] {
        assert!(b.is_console_write(num, 17), "a write to the alias is mirrored (spec §3a)");
        assert!(b.is_console_write(num, 1), "and to stdout itself, still");
    }
    for num in [SYS_CLOSE, SYS_CLOSE_NOCANCEL] {
        assert!(!b.is_console_close(num, 17),
            "close(17) must NOT be faked: its host mapping is the dup (41), not retrace's fd 1 — \
             faking it would leak the dup and leave slot 17 open forever (review I2)");
        assert!(b.is_console_close(num, 1), "close(1) is still the identity slot");
    }
    // The generic path's table half then retires the alias like any descriptor.
    assert!(b.fds_mut().close(17));
    assert_eq!(b.fds().slots()[17], FdSlot::Closed);
    assert!(!b.is_console_write(SYS_WRITE, 17), "a closed alias is not a console write");
}

#[test]
fn a_displaced_console_slot_is_neither_a_console_write_nor_a_console_close() {
    let mut b = fresh_box();
    let f = b.fds_mut().alloc();
    b.fds_mut().bind(f, 30);
    // dup2(f, 1): stdout's slot is now a plain descriptor of f's file; retrace's own fd 1 is the
    // displaced identity mapping, which guest_dup2 never closes.
    assert_eq!(b.fds_mut().dup2(f, 1, Some(32)).unwrap(), (1, Some(1)));
    assert_eq!(b.fds().slots()[1], FdSlot::Open);
    assert!(!b.is_console_write(SYS_WRITE, 1), "a write to 1 now goes to the file (spec §3a)");
    assert!(!b.is_console_close(SYS_CLOSE, 1),
        "close(1) now closes the dup (32) through the generic path — not retrace's stdout");
    assert!(b.is_console_close(SYS_CLOSE, 2), "stderr's identity slot is untouched");
    assert!(b.is_console_write(SYS_WRITE, 2));
}

#[test]
fn a_closed_identity_slot_is_no_longer_the_console_on_either_predicate() {
    // C1's shape at the predicate level: after record's arm retires slot 1 (`FdTable::close`), a
    // write to 1 is not mirrored and a second close(1) is not faked — both reach the generic path
    // and answer EBADF from the table, the kernel's answer.
    let mut b = fresh_box();
    assert!(b.is_console_close(SYS_CLOSE, 1));
    assert!(b.fds_mut().close(1));
    assert_eq!(b.fds().slots()[1], FdSlot::Closed);
    assert!(!b.is_console_write(SYS_WRITE, 1));
    assert!(!b.is_console_close(SYS_CLOSE, 1));
    assert_eq!(b.fds().host(1), None, "the host mapping is dropped; retrace's fd 1 itself is untouched");
    assert_eq!(b.fds_mut().dup2(1, 17, Some(41)), Err(retrace_box::EBADF), "dup2 from a closed console slot is EBADF");
}

// M37 fix wave (final review M3). An identity slot RE-ALIASED by dup2 — `dup2(1, 17); dup2(17, 1)`,
// or the shell's `saved = dup(1); …; dup2(saved, 1)` — is `Console(1)` at index 1 again, but its
// host mapping is now a dup, not retrace's own fd 1. Its close must go the generic way (host dup
// closed, slot retired), not be faked: faking it would drop the mapping and leak the dup. The
// guest sees the same either way; only the recorder's descriptor table differs.
#[test]
fn a_re_aliased_identity_slot_closes_the_generic_way() {
    let mut b = fresh_box();
    let saved = b.fds_mut().dup(1).unwrap();
    b.fds_mut().bind(saved, 41);                    // record: the host dup of retrace's stdout
    let f = b.fds_mut().alloc(); b.fds_mut().bind(f, 30);
    assert_eq!(b.fds_mut().dup2(f, 1, Some(32)).unwrap(), (1, Some(1)), "redirect: the identity mapping is displaced");
    assert!(!b.is_console_close(SYS_CLOSE, 1), "a displaced slot is not the console");
    assert_eq!(b.fds_mut().dup2(saved, 1, Some(43)).unwrap(), (1, Some(32)), "restore: stdout's kind is back");
    assert_eq!(b.fds().slots()[1], FdSlot::Console(1));
    assert_eq!(b.fds().host(1), Some(43), "but its host mapping is the dup, not retrace's fd 1");
    assert!(b.is_console_write(SYS_WRITE, 1), "a write to 1 is mirrored again");
    for num in [SYS_CLOSE, SYS_CLOSE_NOCANCEL] {
        assert!(!b.is_console_close(num, 1),
            "close(1) on the re-aliased slot must NOT be faked: its host mapping is the dup (43), \
             which the generic path closes — faking it would leak the dup (final review M3)");
    }
    assert!(b.is_console_close(SYS_CLOSE, 2), "stderr's identity slot is untouched");
    assert!(b.is_console_close(SYS_CLOSE, 0), "and stdin's");
}
