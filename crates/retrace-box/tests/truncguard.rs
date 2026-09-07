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
// MEASURED, and corrected by M30: this test drives the RAW trap 189, which writes a 120-byte reply
// — not the 144 bytes of `sizeof(struct stat)`, which belongs to trap 339, the call libc's `fstat()`
// routes to and this guest never issues. The conclusion is unchanged and is what matters: a 64-byte
// window is genuinely overrun by a real kernel write, with 56 bytes of it landing in the band.
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

// M29: `dest_buffer`'s job is to widen the diff window past the flat cap for syscalls whose
// destination is bigger than the cap. That is a pure function of the table and the arguments, so
// it is tested at the seam rather than through a guest that would have to be built to call each
// of these four syscalls with a large buffer.
//
// `avail` is passed as 1 MiB so the backing never becomes the binding constraint — this test is
// about the table, and a too-small `avail` would silently make every case clamp to the same
// number and pass for the wrong reason.
#[test]
fn the_window_widens_for_each_m29_reg_addition() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::HELLO).unwrap());
    let b = Box_::load(&loaded);
    const AVAIL: usize = 1 << 20;
    const FLAT: usize = 64 * 1024; // PTR_WINDOW_CAP

    let mut args = [0u64; 8];

    args[2] = 200_000; // getdirentries64 bufsize
    assert_eq!(b.diff_window_for_test(retrace_arch::SYS_GETDIRENTRIES64, 1, AVAIL, &args), 200_000,
        "getdirentries64's destination is x1 and its length x2");

    args[1] = 150_000; // getfsstat64 bufsize, in bytes
    assert_eq!(b.diff_window_for_test(retrace_arch::SYS_GETFSSTAT64, 0, AVAIL, &args), 150_000,
        "getfsstat64's destination is x0 and its length x1");

    args[2] = 90_000; // recvfrom len
    assert_eq!(b.diff_window_for_test(retrace_arch::SYS_RECVFROM, 1, AVAIL, &args), 90_000);
    assert_eq!(b.diff_window_for_test(retrace_arch::SYS_RECVFROM_NOCANCEL, 1, AVAIL, &args), 90_000,
        "the _nocancel spelling must widen identically — that pairing is the trap M9/M10/M27 each hit");

    // An argument index that is NOT this syscall's destination still gets the flat cap. Without
    // this the test would pass even if `dest_buffer` widened every pointer argument, which would
    // be a far worse bug than the one it is guarding.
    assert_eq!(b.diff_window_for_test(retrace_arch::SYS_GETDIRENTRIES64, 3, AVAIL, &args), FLAT,
        "x3 is getdirentries64's *position, not its buffer");
    // A syscall absent from the table gets the flat cap at every index.
    assert_eq!(b.diff_window_for_test(retrace_arch::SYS_WRITE, 1, AVAIL, &args), FLAT);
}

// M29 Phase B. Two tests over ONE guest, split because a panic ends a test: the first drives only
// the legal call and must complete, the second drives both and must abort on the second.
//
// `expected` pins the SYSCALL NUMBER, not just a message fragment — M28's positive-control lesson.
// A message-only match would also be satisfied by the refusal firing on the wrong call.
//
// Every arm resumes the guest with `set_x0_err_and_return` after forwarding — `forward_and_diff`
// only forwards and diffs, it never advances the vCPU (that is `retrace-core`'s job in production).
// Without it `b.run()` re-traps the SAME un-advanced `svc`, which silently turns "drive to the
// second sysctl" into "drive to the first sysctl twice" (`failsys.rs`/`the_band_fires...` above
// establish this pattern).
#[test]
#[should_panic(expected = "syscall 202 asked for")]
fn an_oldlenp_past_its_backing_is_refused() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::OLDLENSYSCTL).unwrap());
    let mut b = Box_::load(&loaded);
    let mut seen = 0;
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_SYSCTL => {
                seen += 1;
                let (ret, err, _writes) = b.forward_and_diff(num, args);
                assert!(seen < 2, "NOT-THE-REFUSAL: the second sysctl carries *oldlenp = 1 TiB and \
                                   forward_and_diff returned normally");
                b.set_x0_err_and_return(ret, err);
            }
            Stop::Syscall { num, args } => {
                let (ret, err, _writes) = b.forward_and_diff(num, args);
                b.set_x0_err_and_return(ret, err);
            }
            other => panic!("NOT-THE-REFUSAL: guest stopped with {other:?} before the second sysctl"),
        }
    }
}

// The other half, and the one that makes the refusal narrow rather than blunt: `oldp == NULL` is a
// legal sysctl asking only for the size. There is no destination to bound, so it must forward
// untouched — a refusal that fired here would break every size-query in every guest.
#[test]
fn a_null_oldp_sysctl_is_not_refused() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::OLDLENSYSCTL).unwrap());
    let mut b = Box_::load(&loaded);
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_SYSCTL => {
                assert_eq!(args[2], 0, "the FIRST sysctl this guest issues has oldp == NULL");
                b.forward_and_diff(num, args); // must not panic
                return;
            }
            Stop::Syscall { num, args } => {
                let (ret, err, _writes) = b.forward_and_diff(num, args);
                b.set_x0_err_and_return(ret, err);
            }
            other => panic!("guest stopped with {other:?} before its first sysctl"),
        }
    }
}

// The boundary the refusal turns on, pinned directly. M29 shipped the `want <= avail` comparison
// with no test that exercises `want == avail`: `a_null_oldp_sysctl_is_not_refused` passes
// `oldp = NULL`, so `host_span(0)` is `None` and it never reaches the comparison, and
// `an_oldlenp_past_its_backing_is_refused` asks for `1 << 40`. Mutation-tested at M29: the
// INVERSION `want >= avail` is caught by those two, but the STRICTNESS mutation `want < avail`
// survives them both.
//
// M29's own status-log entry proposed closing this with a third *fitting* sysctl in the guest
// fixture (`*oldlenp = 64`). That would not have worked: `avail` is the distance from the buffer to
// the end of its BACKING, not the end of the 64-byte symbol, so a 64-byte request sits far under it
// and `want < avail` stays green. Only `want == avail` separates the two, and a freestanding guest
// cannot know `avail`. Hence a pure predicate, tested directly — the same treatment `clamp_count`
// and `overran_window` already get, and for the same stated reason: the policy is reviewable apart
// from the plumbing that feeds it.
#[test]
fn a_deref_len_is_refused_only_past_its_backing() {
    assert!( Box_::deref_len_fits(/*want=*/63, /*avail=*/64)); // under  => forwards (kills `want >= avail`)
    assert!( Box_::deref_len_fits(/*want=*/64, /*avail=*/64)); // EXACT  => forwards (kills `want < avail`)
    assert!(!Box_::deref_len_fits(/*want=*/65, /*avail=*/64)); // past   => refused  (kills `want > avail` never firing)
    assert!( Box_::deref_len_fits(/*want=*/0,  /*avail=*/0));  // a zero-length request into a full backing still fits
    assert!(!Box_::deref_len_fits(/*want=*/1,  /*avail=*/0));  // no room at all => refused
}

// M30: a repo-owned reproduction of the false negative M27 measured on `/bin/ps`, and the reason
// `canary_intact` exists. The window cap is placed so the only bytes the kernel writes past the
// window are trap 189's trailing zero field — written over a band that is already zero. The band
// therefore reads identically before and after a REAL kernel overrun, and `overran_window`, which
// can only report a change, has nothing to report.
//
// This asserts the BLINDNESS, and it stays green after M30 closes the hole: it documents the
// question the old predicate asks, not the answer the new one gives. Task 4 adds the matching
// caught-half.
#[test]
fn the_old_comparison_is_blind_to_zeros_written_over_zeros() {
    // Measured, not assumed: trap 189 writes a 120-byte reply whose trailing zero run is [90,120).
    // 96 leaves 24 kernel-written ZERO bytes past the window — a real overrun with no signal in it.
    //
    // Nothing in THIS test demonstrates that the overrun is real: its own assertions can only show
    // the band unchanged, which is equally consistent with no kernel write at all. The evidence is
    // `the_canary_catches_zeros_written_over_zeros` below, which drives the same fixture at the same
    // cap and observes the canary destroyed there. The two are a pair; neither alone says this.
    const CAP: usize = 96;
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::FILEIO).unwrap());
    let mut b = Box_::load(&loaded);
    b.set_window_cap_for_test(CAP);
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_FSTAT => {
                let (hp, avail) = b.host_span_for_test(args[1]).expect("stat buffer is mapped");
                let win = CAP.min(avail);
                let band = retrace_box::GUARD_BAND.min(avail - win);
                let pre: Vec<u8> = unsafe { std::slice::from_raw_parts(hp.add(win), band) }.to_vec();
                b.forward_and_diff(num, args);
                let post: Vec<u8> = unsafe { std::slice::from_raw_parts(hp.add(win), band) }.to_vec();
                assert!(pre.iter().all(|&x| x == 0), "precondition: the band starts zeroed");
                assert!(!Box_::overran_window(&pre, &post),
                    "this test exists because the old detector is blind here; if it now fires, \
                     the fixture no longer reproduces the false negative and must be re-measured");
                return;
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

// M30: the other half of the reproduction directly above. Same guest, same window cap, same real
// kernel overrun — but asked the NEW question. Together the two tests are the milestone's headline
// claim in one fixture: the old detector is blind here (that test) and the canary is not (this one).
// If this ever fails while `the_old_comparison_is_blind_to_zeros_written_over_zeros` still passes,
// the canary has stopped being written or stopped being checked.
//
// This asserts CATCHING, not merely restoring. Phase A only reports, so the disturbance is not
// observable through a panic — it is observable through `canary_disturbances_for_test`, the
// in-process channel that exists precisely so this claim does not have to be made by a harness that
// pipes and greps stderr. The restore is asserted too, because both claims matter: a canary that
// caught the overrun but leaked into the guest's memory would be a determinism bug, not a fix.
#[test]
fn the_canary_catches_zeros_written_over_zeros() {
    // The SAME cap as `the_old_comparison_is_blind_to_zeros_written_over_zeros`, and the identity
    // is the whole point: that test shows the old detector blind on this exact fixture and cap,
    // this one shows the new detector catching THE SAME overrun. If the two caps differed, neither
    // test would prove anything about the other. Measured (trap 189 writes 120 bytes, trailing zero
    // run [90,120)): 96 leaves 24 kernel-written ZERO bytes past the window.
    const CAP: usize = 96;
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::FILEIO).unwrap());
    let mut b = Box_::load(&loaded);
    b.set_window_cap_for_test(CAP);
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_FSTAT => {
                let (hp, avail) = b.host_span_for_test(args[1]).expect("stat buffer is mapped");
                let win = CAP.min(avail);
                let band = retrace_box::GUARD_BAND.min(avail - win);
                // Snapshotted around the call, not read absolutely: the guest issues other syscalls
                // before its fstat, and any of them could legitimately move the count, so an
                // absolute expectation would be brittle for reasons unrelated to this claim.
                let before = b.canary_disturbances_for_test();
                b.forward_and_diff(num, args);
                assert_eq!(b.canary_disturbances_for_test(), before + 1,
                    "the canary must catch the same overrun \
                     `the_old_comparison_is_blind_to_zeros_written_over_zeros` cannot see");
                // `forward_and_diff` restores the band before returning, so what is readable here
                // is the guest's own pre-syscall bytes — zeros — and not the canary.
                let restored: Vec<u8> =
                    unsafe { std::slice::from_raw_parts(hp.add(win), band) }.to_vec();
                assert!(restored.iter().all(|&x| x == 0),
                    "the band must be restored to its pre-syscall bytes before the guest resumes");
                return;
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

// M30 fix round 1, defect (a): the fill is UNCONDITIONAL but the write-capture loop runs only when
// the syscall succeeded, so a failing call used to leave the canary in guest memory permanently.
// `forward_and_diff` is record-side only — replay applies recorded writes instead — so leaked bytes
// are memory the recording has and the replay does not, i.e. a final full-memory divergence. 36
// such restores were measured in one `jq -n '1+1'` recording, so this path is ordinary traffic
// rather than a corner.
//
// The failing call is an `open` of the FILEIO guest's own path with its leading `/` skipped: a
// relative path that cannot exist, NUL-terminated by the fixture, and mapped so a window and band
// are really taken. Nothing about the failure depends on errno, so the assertion is on `err`.
#[test]
fn a_failing_syscall_still_restores_the_canary() {
    const CAP: usize = 64;
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::FILEIO).unwrap());
    let mut b = Box_::load(&loaded);
    b.set_window_cap_for_test(CAP);
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_OPEN => {
                let path = args[0] + 1; // drop the leading '/' => a relative path that cannot exist
                let (_, avail) = b.host_span_for_test(path).expect("the path buffer is mapped");
                let win = CAP.min(avail);
                let band = retrace_box::GUARD_BAND.min(avail - win);
                assert!(band > 0, "precondition: this test needs a non-empty band to leak");
                let base = path + win as u64;
                let pre = b.read_bytes_for_test(base, band);
                assert!(!Box_::canary_intact(&pre, base),
                    "precondition: the band must not already look like a canary, or a missing \
                     restore would be indistinguishable from a correct one");

                let mut a = [0u64; 8];
                a[0] = path;
                let (_ret, err, writes) = b.forward_and_diff(retrace_arch::SYS_OPEN, a);
                assert!(err, "precondition: opening {path:#x} as a relative path must FAIL, or this \
                              test drives the success path it is not about");
                assert!(writes.is_empty(), "a failed syscall captures nothing");

                assert_eq!(b.read_bytes_for_test(base, band), pre,
                    "the canary must be restored on the FAILING path too — a leak here is bytes \
                     the recording has and the replay does not");
                return;
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

// M30 fix round 1, defect (b): two arguments of one call can hold the SAME value — measured on 60
// of 341 traps in one `jq` recording, including `open`'s x0 == x3 and `stat64`'s x0 == x2 — which
// pushes two `windows` entries with the same ipa, len and band. `band_not_covered` deliberately
// does not let an entry suppress its own band, so it does not suppress a duplicate's either, and
// both entries check the same bytes. Restoring per entry made the first erase the canary the second
// was about to read, reporting a disturbance no kernel caused: a phantom straight into the Phase A
// tally the milestone's flip decision turns on.
//
// The cap is 256 deliberately: trap 189 writes 120 bytes, so nothing overruns and the ONLY thing
// that could move the counter is the ordering defect. At Task 2's cap of 96 a real overrun would
// increment it and hide the bug.
#[test]
fn a_duplicated_pointer_argument_does_not_manufacture_a_disturbance() {
    const CAP: usize = 256;
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::FILEIO).unwrap());
    let mut b = Box_::load(&loaded);
    b.set_window_cap_for_test(CAP);
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_FSTAT => {
                let (_, avail) = b.host_span_for_test(args[1]).expect("stat buffer is mapped");
                assert!(avail > CAP, "precondition: a band must exist past a {CAP}-byte window");
                // x2 is not an operand of fstat, so duplicating the buffer pointer there changes
                // nothing the kernel does — only how many entries the pre-pass pushes.
                let mut a = args;
                a[2] = args[1];
                let before = b.canary_disturbances_for_test();
                b.forward_and_diff(num, a);
                assert_eq!(b.canary_disturbances_for_test(), before,
                    "two entries sharing one band must not report a disturbance: no kernel write \
                     reaches past a {CAP}-byte window for a 120-byte fstat reply");
                return;
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
