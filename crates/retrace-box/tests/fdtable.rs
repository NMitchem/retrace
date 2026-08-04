// M10 t2. The guest-visible half of the fd table: allocation, close, dup aliasing, console seeding.
// Pure data structure — no VM, so this test creates no vcpu and is unaffected by the one-VM-per-
// process rule (it still runs under --test-threads=1 with the rest of the crate).
use retrace_box::{FdSlot, FdTable};

#[test]
fn console_fds_are_preseeded_open() {
    let t = FdTable::new();
    for gfd in 0..=2 {
        assert!(t.is_open(gfd), "fd {gfd} is the console and starts open");
    }
    assert!(!t.is_open(3), "fd 3 starts free");
}

#[test]
fn alloc_returns_lowest_free_starting_at_three() {
    let mut t = FdTable::new();
    // THE determinism property: a guest's first open is 3, not whatever number the host had free.
    // Measured pre-M10, jq saw 17 here, purely because retrace itself holds 0-16 open.
    assert_eq!(t.alloc(), 3);
    assert_eq!(t.alloc(), 4);
    assert_eq!(t.alloc(), 5);
}

#[test]
fn close_frees_the_slot_for_reuse_and_reports_bad_closes() {
    let mut t = FdTable::new();
    let a = t.alloc(); // 3, gets closed
    let b = t.alloc(); // 4, stays open
    assert!(t.close(a));
    assert!(!t.is_open(a));
    assert!(!t.close(a), "closing an already-closed fd reports failure (the caller answers EBADF)");
    assert!(!t.close(99), "closing a never-opened fd reports failure");
    assert_eq!(t.alloc(), a, "the freed slot is the lowest free and is reused");
    assert!(t.is_open(b), "closing one fd must not disturb another");
}

#[test]
fn bind_and_host_map_the_guest_fd_to_a_host_fd() {
    let mut t = FdTable::new();
    let g = t.alloc();
    t.bind(g, 17); // the host handed back 17; the guest must never see it
    assert_eq!(t.host(g), Some(17));
    assert_eq!(t.host(99), None, "an unallocated guest fd has no host mapping");
    t.close(g);
    assert_eq!(t.host(g), None, "closing clears the host mapping");
}

#[test]
fn dup_aliases_two_guest_fds_onto_one_host_fd() {
    let mut t = FdTable::new();
    let g = t.alloc();
    t.bind(g, 17);
    let d = t.alloc();
    t.bind(d, 17); // dup: a second guest fd over the same host fd
    assert_ne!(g, d);
    assert_eq!(t.host(g), t.host(d));
    // Closing the alias must not disturb the original's host mapping.
    t.close(d);
    assert_eq!(t.host(g), Some(17));
}

#[test]
fn console_fds_map_identically_onto_retraces_own() {
    // M9 intercepts console WRITES and CLOSES upstream, but nothing else: stdio still fstat()s and
    // ioctl()s fd 1 to choose a buffering mode. Those forward, so 0/1/2 must resolve to retrace's
    // own 0/1/2 — leaving them unmapped answers EBADF and crashed watch_dyn's guest.
    let t = FdTable::new();
    for gfd in 0..=2 {
        assert_eq!(t.host(gfd), Some(gfd as i32),
            "console fd {gfd} must map identically onto retrace's own descriptor");
    }
}

#[test]
fn a_closed_console_fd_loses_its_identity_mapping() {
    // The guest closing fd 1 is faked upstream and never reaches here, but if it ever did, the
    // mapping must not survive — that is the M9 bug (a guest closing RETRACE's stdout) in table form.
    let mut t = FdTable::new();
    assert!(t.close(1));
    assert_eq!(t.host(1), None);
    assert!(!t.is_open(1));
}

#[test]
fn slots_round_trip_through_from_slots() {
    let mut t = FdTable::new();
    let a = t.alloc();
    let b = t.alloc();
    t.close(a);
    let restored = FdTable::from_slots(&t.slots());
    assert!(!restored.is_open(a), "a closed fd stays closed across a round trip");
    assert!(restored.is_open(b));
    for gfd in 0..=2 {
        assert!(restored.is_open(gfd));
    }
    assert_eq!(restored.slots(), t.slots());
    assert_eq!(restored.slots()[a as usize], FdSlot::Closed,
        "Closed must stay distinguishable from Free across the round trip");
}

// M10 t4, in the shape of M9 t3's regression test. State a mid-run capture cannot re-derive must be
// CARRIED — this is the third field in BoxState to exist for that reason (after pac_enabled and
// stack_top). If from_checkpoint installed a fresh table, a seeked session would believe every fd is
// Free, so a post-seek guest pread returns EBADF and reverse execution diverges from the forward run.
#[test]
fn fd_table_survives_checkpoint_restore() {
    let mut t = FdTable::new();
    let a = t.alloc(); // 3, stays open
    let b = t.alloc(); // 4, gets closed
    t.bind(a, 17);
    t.close(b);

    let restored = FdTable::from_slots(&t.slots());
    assert!(restored.is_open(a), "an open fd must survive the restore");
    assert!(!restored.is_open(b), "a CLOSED fd must stay closed — else a seek resurrects it");
    assert_eq!(restored.slots()[b as usize], FdSlot::Closed,
        "Closed must stay distinguishable from Free across the restore");
    for gfd in 0..=2 {
        assert!(restored.is_open(gfd), "console fd {gfd} must survive the restore open");
        assert_eq!(restored.host(gfd), Some(gfd as i32),
            "the console identity mapping is a CONSTANT and must be rederived on restore — \
             without it a seeked session answers EBADF to fstat(1)");
    }
    // A guest-opened fd's host mapping is record-only and must NOT come back.
    assert_eq!(restored.host(a), None, "a guest fd's host mapping is record-only");
}

#[test]
fn from_slots_carries_no_host_mapping() {
    // The host half is record-only by construction: replay opens no host fd, so a restored table
    // must not claim one. This is what keeps host fd numbers out of the trace entirely.
    let mut t = FdTable::new();
    let g = t.alloc();
    t.bind(g, 17);
    let restored = FdTable::from_slots(&t.slots());
    assert!(restored.is_open(g), "guest-visible state survives");
    assert_eq!(restored.host(g), None, "host mapping does NOT survive");
}
