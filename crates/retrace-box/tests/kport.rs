//! M46 Ruling T4-a, box level: the synthetic thread kport's shape. `crates/retrace/tests/kport.rs`
//! pins one child's kport end to end; this pins the scheme across several threads on a static box.
use retrace_box::Box_;

fn tb() -> Box_ {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::SPINLOOP).unwrap());
    Box_::load(&loaded)
}

/// libplatform's `os_unfair_lock` reads bit 0 of the owner's port name as its no-waiters flag, and
/// libdispatch's `DLOCK_OWNER_MASK` reads bits 0 and 1. A real name always ends in binary 11. M46
/// Task 4 measured what a name with bit 0 clear costs: tid 2's recursive `@synchronized` did not
/// see itself as the owner and waited on itself in `__ulock_wait2`. So every kport ends in `0b11`,
/// and masking the two bits off still leaves one owner per thread.
#[test]
fn every_box_spawned_kport_ends_in_binary_11_like_a_real_port_name() {
    let mut b = tb();
    b.set_thread_start_pc(0x0001_804b_2000);
    // Three pthread structs inside the static box's one mapped stack page (0x4000 bytes), each
    // with room for its `+0xf8` kport field.
    let pthreads: Vec<u64> = (1..=3).map(|n| b.stack_top() - 0x4000 + n * 0x1000).collect();
    for &p in &pthreads {
        b.guest_bsdthread_create([0x1_0002_4e00, 0, p - 0x100, p, 0, 0, 0, 0]);
    }
    let kports: Vec<u32> = (1..=3).map(|tid| b.kport_of(tid).expect("a spawned thread's pthread is mapped")).collect();
    assert_eq!(kports[0], 0x0BAD_7007, "tid 1: GUEST_THREAD_PORT_BASE | (1 << 2) | 3");
    for (i, &k) in kports.iter().enumerate() {
        assert_eq!(k & 3, 3, "tid {}'s kport {k:#x} must end in binary 11", i + 1);
    }
    let mut owners: Vec<u32> = kports.iter().map(|k| k & !3).collect();
    owners.sort_unstable();
    owners.dedup();
    assert_eq!(owners.len(), kports.len(), "distinct kports, even under DLOCK_OWNER_MASK: {kports:#x?}");
}

/// Spawn tids `1..=n` through `bsdthread_create`, all on one pthread struct, so each spawn
/// overwrites its `+0xf8` kport field and `kport_of(n)` reads the name tid `n` was given.
fn spawn_through(n: usize) -> Box_ {
    let mut b = tb();
    b.set_thread_start_pc(0x0001_804b_2000);
    let p = b.stack_top() - 0x1000;
    for _ in 1..=n {
        b.guest_bsdthread_create([0x1_0002_4e00, 0, p - 0x100, p, 0, 0, 0, 0]);
    }
    b
}

/// `GUEST_THREAD_PORT_BASE` (`0x0BAD_7000`) has bits 12–14 set, so `tid << 2` fits under them only
/// up to tid `0x3ff`. That last tid still gets a name ending in binary 11.
#[test]
fn tid_0x3ff_is_the_last_kport_the_scheme_can_name_and_it_ends_in_binary_11() {
    let b = spawn_through(0x3ff);
    let k = b.kport_of(0x3ff).expect("the shared pthread is mapped");
    assert_eq!(k, 0x0BAD_7FFF, "GUEST_THREAD_PORT_BASE | (0x3ff << 2) | 3");
    assert_eq!(k & 3, 3, "{k:#x} must end in binary 11");
}

/// From tid `0x400` on, `tid << 2` reaches the base's bits, and tid `0x401` would be handed tid 1's
/// `0x0BAD_7007`. Two threads with one name alias silently in `thread_of_port` and in lock owners,
/// and M18 never reuses a worker, so the box stops at the 1024th spawn instead.
#[test]
#[should_panic(expected = "M18 never reuses a parked worker, so a guest that spawns more than 1023 threads stops here by design")]
fn tid_0x400_is_refused_because_its_kport_would_alias_an_earlier_threads() {
    spawn_through(0x400);
}
