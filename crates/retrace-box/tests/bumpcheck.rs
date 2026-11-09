// M57 t8: every bump placement checks what it hands out. Since Task 3 widened the IPA to 40 bits, a
// FIXED reservation above COMMPAGE2_IPA no longer keeps the cursor ahead of it, and a hinted first fit
// floats the cursor past it unchecked; a later hint-0 bump must still never land inside it.
use retrace_box::Box_;
use retrace_guest::{parse_macho, HELLO};

const G: u64 = 1 << 30;
const RES: u64 = 0x78_0000_0000;
const HINT: u64 = 0x77_0000_0000;
const ANYWHERE: u64 = retrace_arch::VM_FLAGS_ANYWHERE;
const FIXED: u64 = 0;
const MAP_ANON_PRIVATE: u64 = 0x1002;

fn boxed() -> Box_ {
    Box_::load(&parse_macho(&std::fs::read(HELLO).unwrap()))
}

fn check(placed: &[(u64, u64)]) {
    for (i, &(a, l)) in placed.iter().enumerate() {
        assert!(a + l <= RES || a >= RES + 4 * G, "placement {a:#x}+{l:#x} lands inside the reservation");
        for &(b, m) in &placed[..i] {
            assert!(a + l <= b || b + m <= a, "placement {a:#x}+{l:#x} overlaps earlier {b:#x}+{m:#x}");
        }
    }
}

fn drive(mut next: impl FnMut(&mut Box_, u64) -> Result<u64, u64>, first: impl FnOnce(&mut Box_) -> u64) {
    let mut b = boxed();
    b.guest_vm_reserve_masked(RES, 4 * G, 0, FIXED).expect("reserve");
    let mut placed = vec![(first(&mut b), 4 << 20)];
    let mut total = 0;
    while total < 8 * G {
        let a = next(&mut b, G).expect("room remains");
        placed.push((a, G));
        total += G;
    }
    check(&placed);
}

#[test]
fn a_bump_never_lands_in_a_reservation_above_the_old_ceiling() {
    drive(|b, l| b.guest_vm_map_masked(0, l, 0, ANYWHERE, false),
          |b| b.guest_vm_map_masked(HINT, 4 << 20, 0, ANYWHERE, false).unwrap());
}

#[test]
fn a_reserve_bump_never_lands_in_a_reservation_above_the_old_ceiling() {
    drive(|b, l| b.guest_vm_reserve_masked(0, l, 0, ANYWHERE),
          |b| b.guest_vm_map_masked(HINT, 4 << 20, 0, ANYWHERE, false).unwrap());
}

#[test]
fn a_bsd_mmap_bump_never_lands_in_a_reservation_above_the_old_ceiling() {
    drive(|b, l| b.guest_mmap(0, l, 3, MAP_ANON_PRIVATE),
          |b| b.guest_vm_map_masked(HINT, 4 << 20, 0, ANYWHERE, false).unwrap());
}
