//! M45 (spec §3b, §4): the validator that decides whether a `kevent_qos` (374) is libdispatch's
//! measured workqueue-kqueue init. Pure and VM-free. `kqinit_shape` is the whole modelled surface:
//! `Box_::guest_kevent_qos` returns 0 on `Ok` and refuses on `Err`, so every shape this file does
//! not accept is a shape retrace stops on by name.
use retrace_arch::{kqinit_shape, KEVENT_QOS_SIZE, KQINIT};

/// The call as M44 t0 M1 measured it (`[trap] num=374 pc=0x1804afa48
/// args=[0xffffffff,0x27ff348,0x1,0x0,0x0,0x0,0x0,0x21]`), re-measured by M45 t0 M1.
const MEASURED: [u64; 8] = [0xffff_ffff, 0x27f_f348, 1, 0, 0, 0, 0, 0x21];

fn entry() -> [u8; KEVENT_QOS_SIZE] { KQINIT.to_bytes() }

#[test]
fn the_measured_init_is_accepted() {
    assert_eq!(kqinit_shape(MEASURED, &entry()), Ok(()));
}

/// R1: the entry is compared exactly, all 72 bytes. A field left unchecked would survive a flip.
#[test]
fn every_single_bit_flip_of_the_entry_is_refused() {
    for byte in 0..KEVENT_QOS_SIZE {
        for bit in 0..8 {
            let mut e = entry();
            e[byte] ^= 1 << bit;
            assert!(kqinit_shape(MEASURED, &e).is_err(), "byte {byte} bit {bit} flipped and still accepted");
        }
    }
}

/// Each field's refusal names it, and flipping bit 0 of the byte at xnu's offset is what reaches
/// it, so this pins the layout (`event_private.h:115-125`) as well as the message.
#[test]
fn each_entry_field_is_named_at_its_xnu_offset() {
    let at = [("ident", 0), ("filter", 8), ("flags", 10), ("qos", 12), ("udata", 16), ("fflags", 24),
        ("xflags", 28), ("data", 32), ("ext[0]", 40), ("ext[1]", 48), ("ext[2]", 56), ("ext[3]", 64)];
    for (name, off) in at {
        let mut e = entry();
        e[off] ^= 1;
        let err = kqinit_shape(MEASURED, &e).unwrap_err();
        assert!(err.starts_with(&format!("changelist[0].{name} is ")), "offset {off}: {err}");
    }
}

/// R2: `kq`, `nchanges`, `nevents` and `flags` are C `int`/`unsigned int`, and the kernel reads 32
/// bits of each. `mov x0, #-1` (`0xffffffffffffffff`) is the same call as the measured
/// `0xffffffff`.
#[test]
fn an_int_arguments_upper_half_is_ignored_as_the_kernel_ignores_it() {
    for i in [0, 2, 4, 7] {
        for bit in 32..64 {
            let mut a = MEASURED;
            a[i] ^= 1u64 << bit;
            assert_eq!(kqinit_shape(a, &entry()), Ok(()), "x{i} bit {bit}: an int's upper half is not the kernel's");
        }
    }
}

#[test]
fn every_bit_the_kernel_reads_of_each_checked_argument_is_refused_by_register() {
    for (i, width) in [(0, 32), (2, 32), (3, 64), (4, 32), (5, 64), (6, 64), (7, 32)] {
        for bit in 0..width {
            let mut a = MEASURED;
            a[i] ^= 1u64 << bit;
            let err = kqinit_shape(a, &entry()).unwrap_err();
            assert!(err.starts_with(&format!("x{i} (")), "x{i} bit {bit}: {err}");
        }
    }
}

/// `x1` is WHERE the entry is, not what it is. The box reads the entry through it; the validator
/// judges the bytes.
#[test]
fn the_change_list_address_is_not_compared() {
    for x1 in [0, 0x10, 0x27f_f348, 0x1_0000_4000, u64::MAX] {
        let mut a = MEASURED;
        a[1] = x1;
        assert_eq!(kqinit_shape(a, &entry()), Ok(()), "x1 = {x1:#x}");
    }
}

/// `read_va_prefix` stops at the first byte that does not translate, so an entry the guest's page
/// tables do not fully map arrives short. It must be refused by name, never padded or guessed.
#[test]
fn a_short_entry_is_refused_as_untranslated() {
    for len in [0, 1, 40, 71] {
        let err = kqinit_shape(MEASURED, &entry()[..len]).unwrap_err();
        assert!(err.contains(&format!("read {len} of 72 bytes")), "len {len}: {err}");
    }
}

fn sdk_header(rel: &str) -> String {
    let out = std::process::Command::new("xcrun").arg("--show-sdk-path").output().expect("run xcrun");
    assert!(out.status.success(), "xcrun --show-sdk-path failed");
    let path = format!("{}/usr/include/{rel}", String::from_utf8(out.stdout).unwrap().trim());
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The value of `#define <name> <value>` in `text`: parentheses stripped, decimal or `0x` hex, an
/// optional leading minus. `None` if the header does not define `name`.
fn define(text: &str, name: &str) -> Option<i64> {
    text.lines().find_map(|l| {
        let mut w = l.split_whitespace();
        if w.next() != Some("#define") || w.next() != Some(name) { return None; }
        let v = w.next()?.trim_start_matches('(').trim_end_matches(')');
        let (neg, v) = match v.strip_prefix('-') { Some(r) => (true, r), None => (false, v) };
        let n = match v.strip_prefix("0x") { Some(h) => i64::from_str_radix(h, 16).ok()?, None => v.parse().ok()? };
        Some(if neg { -n } else { n })
    })
}

/// The constants are the SDK's, read from its headers at test time (M44 R5's method), so a value
/// typed from memory cannot satisfy this. `KEVENT_FLAG_WORKQ` is the one the SDK does not define;
/// it is xnu's `event_private.h:141`. This asserts that too, so an SDK that starts shipping it is
/// noticed and cited instead.
#[test]
fn the_constants_are_the_sdks() {
    let ev = sdk_header("sys/event.h");
    assert_eq!(define(&ev, "EVFILT_USER"), Some(i64::from(retrace_arch::EVFILT_USER)));
    assert_eq!(define(&ev, "EV_ADD"), Some(i64::from(retrace_arch::EV_ADD)));
    assert_eq!(define(&ev, "EV_ENABLE"), Some(i64::from(retrace_arch::EV_ENABLE)));
    assert_eq!(define(&ev, "EV_CLEAR"), Some(i64::from(retrace_arch::EV_CLEAR)));
    assert_eq!(define(&ev, "KEVENT_FLAG_IMMEDIATE"), Some(i64::from(retrace_arch::KEVENT_FLAG_IMMEDIATE)));
    assert_eq!(define(&ev, "KEVENT_FLAG_WORKQ"), None,
        "the SDK now defines KEVENT_FLAG_WORKQ: cite it from the SDK instead of xnu");
    let sc = sdk_header("sys/syscall.h");
    assert_eq!(define(&sc, "SYS_kevent_qos"), Some(retrace_arch::SYS_KEVENT_QOS as i64));
}
