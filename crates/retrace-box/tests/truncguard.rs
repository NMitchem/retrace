use retrace_box::{Box_, Stop};

// M27: the guard band is DIRECT evidence, not a heuristic. Between the pre-image and post-image
// copies the only thing that runs is `host_svc` — the guest vCPU is halted and recorder threads are
// banned — so any pre != post byte in a band placed PAST the diff window is provably a kernel write
// that ran past that window. Kernel writes into a destination buffer are contiguous from the buffer
// start, so an overrun cannot skip the band.
#[test]
fn a_changed_guard_band_means_the_write_ran_past_the_window() {
    assert!(Box_::overran_window(&[0u8; 64], &[1u8; 64]),
        "a wholly rewritten band is an overrun");
    assert!(Box_::overran_window(&[0u8; 64], &{ let mut b = [0u8; 64]; b[0] = 1; b }),
        "ONE changed byte is enough: writes are contiguous, so the first byte past the window is \
         the one an overrun touches first");
}

#[test]
fn an_unchanged_guard_band_is_not_an_overrun() {
    assert!(!Box_::overran_window(&[0u8; 64], &[0u8; 64]));
    assert!(!Box_::overran_window(&[7u8; 64], &[7u8; 64]));
}

// A band that could not be taken (the window already covers the whole backing) is never an
// overrun: nothing can be past the backing without a separate memory-safety bug, which is a
// different failure with its own loud symptom.
#[test]
fn an_empty_guard_band_is_never_an_overrun() {
    assert!(!Box_::overran_window(&[], &[]));
}

// M28: the POSITIVE control. Everything else touching the guard band is a NEGATIVE control —
// `bigread_e2e` and `memdiff`'s M26 guard prove it does not FALSE-fire. Nothing proved it fires at
// all. `overran_window`'s unit tests above cover `!pre.is_empty() && pre != post`, the one part
// that cannot be wrong; the offset (`hp.add(win)`), the sizing (`GUARD_BAND.min(avail - win)`) and
// whether the assert is REACHED were covered by nothing. `let band = 0;` passed the entire
// 523-test gate identically to the shipped code, and the M27 band has never been observed to fire
// on a real syscall — M26's "fired exactly once" was the tail-of-window PROTOTYPE, a different
// detector, and /bin/ps was measured NOT to trip this one.
//
// `fstat` is the right syscall here and `read` is the wrong one. `read` is in
// `retrace_arch::dest_buffer`, so `diff_window` widens its window to the full byte count no matter
// how small the cap is, and this test would be vacuously green. `fstat` is deliberately absent from
// that table (its length is not in a register), so a shrunken cap really does truncate it.
//
// MEASURED: `sizeof(struct stat)` is 144 bytes on this SDK, so a 64-byte window
// is genuinely overrun by a real kernel write.
#[test]
#[should_panic(expected = "syscall 189 changed a byte in the")]
fn the_band_fires_when_the_kernel_writes_past_the_window() {
    const CAP: usize = 64;
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::FILEIO).unwrap());
    let mut b = Box_::load(&loaded);
    b.set_window_cap_for_test(CAP);
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_FSTAT => {
                b.forward_and_diff(num, args);
                // Deliberately worded to share NO substring with the assert's message, so
                // `should_panic` cannot be satisfied by this panic instead of the real one.
                panic!("NOT-THE-GUARD-BAND: fstat wrote past a {CAP}-byte window and nothing fired");
            }
            Stop::Syscall { num, args } => {
                let (ret, _e, _w) = b.forward_and_diff(num, args);
                b.set_x0_and_return(ret);
            }
            Stop::Other { esr } => panic!("unexpected exit esr=0x{esr:x}"),
            Stop::Fault { pc, esr, far } => panic!("guest crashed pc=0x{pc:x} esr=0x{esr:x} far=0x{far:x}"),
            Stop::Step => unreachable!("run() does not single-step"),
        }
    }
}

// M28: a changed guard-band byte proves A KERNEL WRITE in that range — the guest vCPU is halted
// across `host_svc` and recorder threads are banned, so nothing else could have touched it. It does
// NOT prove the write was THIS argument's overrun. `forward_and_diff` takes a window for EVERY
// argument that looks like a mapped pointer (including a non-pointer whose value collides with a
// mapped IPA — see the dyld pread-count case in that function), so a write belonging to another
// argument of the same call, fully captured by ITS window, would trip this argument's band and
// panic a correct recording.
#[test]
fn a_band_with_no_neighbours_keeps_its_full_length() {
    assert_eq!(Box_::band_not_covered(0x1000, 256, 64, &[]), 64);
    // A window entirely past the band does not shrink it: band is [0x1100, 0x1140).
    assert_eq!(Box_::band_not_covered(0x1000, 256, 64, &[(0x2000, 16)]), 64);
}

#[test]
fn a_neighbour_starting_inside_the_band_truncates_it_there() {
    // band is [0x1100, 0x1140); a neighbour at 0x1120 leaves the first 0x20 bytes unambiguous.
    assert_eq!(Box_::band_not_covered(0x1000, 256, 64, &[(0x1120, 8)]), 0x20);
}

// The rule is SPAN INTERSECTION, not start position. A window beginning BEFORE the band but
// extending into it overlaps exactly as much as one beginning inside it, and a rule phrased on
// start position alone would miss precisely this case.
#[test]
fn a_neighbour_starting_before_the_band_but_reaching_into_it_still_truncates() {
    // band is [0x1100, 0x1140); neighbour spans [0x10f0, 0x1110) and covers the band's start.
    assert_eq!(Box_::band_not_covered(0x1000, 256, 64, &[(0x10f0, 0x20)]), 0);
}

// The argument's OWN window ends exactly where its band begins, so it can never suppress its own
// band. This is why the caller may pass every span without filtering itself out.
#[test]
fn an_argument_never_suppresses_its_own_band() {
    assert_eq!(Box_::band_not_covered(0x1000, 256, 64, &[(0x1000, 256)]), 64);
}

// `band_not_covered`'s loop is a running `min` over every entry in `others`, not a first-match: a
// caller with three or more overlapping arguments (measured on `/bin/ps`'s `sysctl`) depends on the
// MOST restrictive overlap winning regardless of list order, not just the first one seen.
#[test]
fn two_overlapping_neighbours_produce_the_minimum_of_their_truncations() {
    // band is [0x1100, 0x1140). Alone, (0x1120, 8) truncates to 0x20 and (0x1108, 8) truncates to
    // 0x08 — the second is strictly more restrictive, so together the result must be 0x08, and it
    // must be 0x08 in EITHER order: a running min cannot depend on which neighbour comes first.
    assert_eq!(Box_::band_not_covered(0x1000, 256, 64, &[(0x1120, 8), (0x1108, 8)]), 0x08);
    assert_eq!(Box_::band_not_covered(0x1000, 256, 64, &[(0x1108, 8), (0x1120, 8)]), 0x08);
}

// `band == 0` means the window already covered the whole backing, so there is no band to shrink —
// and it must stay 0 rather than underflow, even against a neighbour that would otherwise truncate
// deeply into it.
#[test]
fn a_zero_length_band_never_underflows() {
    assert_eq!(Box_::band_not_covered(0x1000, 256, 0, &[]), 0);
    assert_eq!(Box_::band_not_covered(0x1000, 256, 0, &[(0x1000, 300)]), 0);
}

// `len == 0` is the argument's own window being degenerate (e.g. a zero-byte `dest_buffer` length).
// Its span in `others` is then a zero-length window at the same `ipa` it starts from — and that
// must still fail to suppress its own band, for the same reason a normal-length self entry does:
// `oe == start` fails the strict `oe > start` test.
#[test]
fn a_zero_length_argument_still_never_suppresses_its_own_band() {
    assert_eq!(Box_::band_not_covered(0x1000, 0, 64, &[(0x1000, 0)]), 64);
}
