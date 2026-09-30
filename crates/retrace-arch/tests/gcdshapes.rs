//! M46 (spec §3b, §4): the validators that decide which `kevent_qos` (374) calls and which
//! `workq_kernreturn(THREAD_KEVENT_RETURN)` change entries the box models. Pure and VM-free.
//! Every shape and entry this file does not accept is one the recorder stops on by name.
use retrace_arch::{
    kevent_qos_shape, kevent_return_change, timer_fired_event, ChangeEntry, KeventQos, KeventShape,
    EVENT_MANAGER_QOS, EVFILT_TIMER, EVFILT_USER, EV_ADD, EV_DELETE, EV_ENABLE, EV_ONESHOT,
    KEVENT_QOS_SIZE, KQINIT, MANAGER_POKE, MEMSTATUS_ADD, TIMER_IDENT_BASE, UPTIME_TIMER_FFLAGS,
    USER_WAKE_EVENT,
};

/// M45's measured init call (`kqinit.rs`).
const INIT_ARGS: [u64; 8] = [0xffff_ffff, 0x27f_f348, 1, 0, 0, 0, 0, 0x21];
/// M45's measurement of the second call (`automationmodetool.entry.txt`): one change, a 16-entry
/// event list, `x7` = `WORKQ|ERROR_EVENTS|IMMEDIATE`. `x1` and `x3` are stack addresses.
const REG_ARGS: [u64; 8] = [0xffff_ffff, 0x27f_ed00, 1, 0x27f_edb8, 0x10, 0, 0, 0x23];

fn memstatus(udata: u64) -> [u8; KEVENT_QOS_SIZE] { KeventQos { udata, ..MEMSTATUS_ADD }.to_bytes() }

fn timer_add(tidx: u64, deadline: i64, leeway: u64, udata: u64) -> KeventQos {
    KeventQos {
        ident: TIMER_IDENT_BASE | tidx, filter: EVFILT_TIMER, flags: EV_ADD | EV_ENABLE | EV_ONESHOT,
        qos: EVENT_MANAGER_QOS, udata, fflags: UPTIME_TIMER_FFLAGS[tidx as usize], xflags: 0,
        data: deadline, ext: [0, leeway, 0, 0],
    }
}

/// `kqinit_shape` is the `Init` arm of the new dispatcher, so M45's refusal texts survive it
/// (`kqinit_e2e`'s `flags` mode matches on this one).
#[test]
fn the_init_keeps_its_m45_meaning_through_the_dispatcher() {
    assert_eq!(kevent_qos_shape(INIT_ARGS, &KQINIT.to_bytes()), Ok(KeventShape::Init));
    let mut e = KQINIT.to_bytes();
    e[10] ^= 0x04; // EV_ENABLE
    assert_eq!(kevent_qos_shape(INIT_ARGS, &e).unwrap_err(), "changelist[0].flags is 0x25, measured 0x21");
}

/// R1: the memory-pressure registration's udata is a heap pointer that varies per run. It is read
/// and handed to the knote table, never compared.
#[test]
fn the_memory_pressure_registration_is_classified_with_its_udata_read() {
    for udata in [0x6c850, 0x1_0000_0000, u64::MAX] {
        assert_eq!(kevent_qos_shape(REG_ARGS, &memstatus(udata)), Ok(KeventShape::MemoryStatusAdd { udata }));
    }
}

#[test]
fn every_compared_bit_of_the_memory_pressure_entry_is_refused() {
    for byte in (0..KEVENT_QOS_SIZE).filter(|b| !(16..24).contains(b)) {
        for bit in 0..8 {
            let mut e = memstatus(0x6c850);
            e[byte] ^= 1 << bit;
            assert!(kevent_qos_shape(REG_ARGS, &e).is_err(), "byte {byte} bit {bit} flipped and still accepted");
        }
    }
}

/// The poke carries only constants (`_dispatch_event_loop_poke`), udata included, so every one of
/// its 576 bits is compared.
#[test]
fn the_manager_poke_is_classified_and_every_bit_of_it_is_compared() {
    assert_eq!(kevent_qos_shape(REG_ARGS, &MANAGER_POKE.to_bytes()), Ok(KeventShape::ManagerPoke));
    for byte in 0..KEVENT_QOS_SIZE {
        for bit in 0..8 {
            let mut e = MANAGER_POKE.to_bytes();
            e[byte] ^= 1 << bit;
            assert!(kevent_qos_shape(REG_ARGS, &e).is_err(), "byte {byte} bit {bit} flipped and still accepted");
        }
    }
}

/// R2: `kq`, `nchanges`, `nevents` and `flags` are `int`s the kernel reads 32 bits of;
/// `data_out` and `data_available` are pointers, compared whole. `x1` and `x3` are where the lists
/// are, not what they hold, so they are not compared (M45 R4).
#[test]
fn argument_widths_follow_the_kernels_types() {
    let e = MANAGER_POKE.to_bytes();
    for i in [0, 2, 4, 7] {
        for bit in 32..64 {
            let mut a = REG_ARGS;
            a[i] ^= 1u64 << bit;
            assert_eq!(kevent_qos_shape(a, &e), Ok(KeventShape::ManagerPoke), "x{i} bit {bit}");
        }
    }
    for (i, width) in [(0, 32), (2, 32), (4, 32), (5, 64), (6, 64)] {
        for bit in 0..width {
            let mut a = REG_ARGS;
            a[i] ^= 1u64 << bit;
            let err = kevent_qos_shape(a, &e).unwrap_err();
            assert!(err.starts_with(&format!("x{i} (")), "x{i} bit {bit}: {err}");
        }
    }
    for x in [0, 0x10, u64::MAX] {
        let mut a = REG_ARGS;
        (a[1], a[3]) = (x, x);
        assert_eq!(kevent_qos_shape(a, &e), Ok(KeventShape::ManagerPoke), "x1 = x3 = {x:#x}");
    }
}

#[test]
fn an_unmeasured_flags_word_is_refused_naming_the_two_measured_ones() {
    for bit in 0..32 {
        let mut a = REG_ARGS;
        a[7] ^= 1 << bit;
        if a[7] & 0xffff_ffff == 0x21 { continue; } // bit 1: the init's word, judged by kqinit_shape
        let err = kevent_qos_shape(a, &MANAGER_POKE.to_bytes()).unwrap_err();
        assert!(err.starts_with("x7 (flags, as unsigned int) is ") && err.contains("0x21") && err.contains("0x23"),
            "bit {bit}: {err}");
    }
}

/// An immediate timer registration (libdispatch's deferred-list overflow, `event_kevent.c:955-975`)
/// or any other filter is refused by name (M46 §7; plan Halt 6).
#[test]
fn a_registration_of_any_other_filter_is_refused_naming_the_filter() {
    let t = KeventQos { filter: EVFILT_TIMER, ..MANAGER_POKE }.to_bytes();
    let err = kevent_qos_shape(REG_ARGS, &t).unwrap_err();
    assert!(err.starts_with("changelist[0].filter is 0xfff9, measured 0xfff2 (EVFILT_MEMORYSTATUS) or 0xfff6 (EVFILT_USER)"),
        "{err}");
}

/// `read_va_prefix` stops at the first byte that does not translate, so an untranslated entry
/// arrives short and must be refused, never padded.
#[test]
fn a_short_registration_entry_is_refused_as_untranslated() {
    for len in [0, 40, 71] {
        let err = kevent_qos_shape(REG_ARGS, &MANAGER_POKE.to_bytes()[..len]).unwrap_err();
        assert!(err.contains(&format!("read {len} of 72 bytes")), "len {len}: {err}");
    }
}

/// R1: a timer arm's deadline (`data`), leeway (`ext[1]`) and udata vary per call and are read.
#[test]
fn a_timer_arm_is_classified_with_its_deadline_leeway_and_udata_read() {
    for tidx in 0..3 {
        for (deadline, leeway, udata) in [(0x1_2345_6789i64, 0u64, 0x6c850u64), (i64::MAX, u64::MAX, 1)] {
            let e = timer_add(tidx, deadline, leeway, udata);
            assert_eq!(kevent_return_change(&e.to_bytes()),
                Ok(ChangeEntry::TimerAdd { ident: TIMER_IDENT_BASE | tidx, deadline: deadline as u64, leeway, udata }));
        }
    }
}

#[test]
fn every_compared_bit_of_a_timer_arm_is_refused() {
    // Read, not compared (R1): udata 16..24, data 32..40, ext[1] 48..56.
    let read = |b: usize| (16..24).contains(&b) || (32..40).contains(&b) || (48..56).contains(&b);
    for byte in (0..KEVENT_QOS_SIZE).filter(|&b| !read(b)) {
        for bit in 0..8 {
            let mut e = timer_add(0, 0x1_2345_6789, 0x100, 0x6c850).to_bytes();
            e[byte] ^= 1 << bit;
            assert!(kevent_return_change(&e).is_err(), "byte {byte} bit {bit} flipped and still accepted");
        }
    }
}

#[test]
fn a_timer_disarm_is_classified() {
    let e = KeventQos { flags: EV_DELETE | EV_ONESHOT, data: 0, ext: [0; 4], ..timer_add(1, 0, 0, 0x6c850) };
    assert_eq!(kevent_return_change(&e.to_bytes()), Ok(ChangeEntry::TimerDelete { ident: TIMER_IDENT_BASE | 1 }));
}

/// M46 §7: MONOTONIC and WALL timers name the clock the model lacks. fflags are judged before the
/// ident, so the refusal names them (kqmanager's WALL-timer test matches on this text).
#[test]
fn monotonic_and_wall_timers_are_refused_naming_their_fflags() {
    for (fflags, tidx) in [(0x198u32, 3u64), (0x9c, 6)] {
        let e = KeventQos { ident: TIMER_IDENT_BASE | tidx, fflags, ..timer_add(0, 1, 0, 1) };
        let err = kevent_return_change(&e.to_bytes()).unwrap_err();
        assert!(err.starts_with(&format!("fflags is {fflags:#x}, measured one of 0x118, 0x138, 0x158")), "{err}");
    }
}

/// The delivered events, as xnu lays them out (spec §2c), and as t0 M2 read them natively.
#[test]
fn the_delivered_events_are_the_measured_bytes() {
    let fired = timer_fired_event(TIMER_IDENT_BASE, 0x100, 0x6c850).to_bytes();
    assert_eq!(&fired[8..16], &[0xf9, 0xff, 0x35, 0x00, 0x00, 0x00, 0x00, 0x02], "filter -7, flags 0x35, qos");
    assert_eq!(&fired[32..40], &1i64.to_le_bytes(), "data: one expiration");
    assert_eq!(&fired[48..56], &0x100u64.to_le_bytes(), "ext[1]: the leeway");
    let user = USER_WAKE_EVENT.to_bytes();
    assert_eq!(&user[0..24], &[1, 0, 0, 0, 0, 0, 0, 0, 0xf6, 0xff, 0x21, 0x00, 0, 0, 0, 2,
                               0xf8, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    assert!(user[24..].iter().all(|&b| b == 0), "fflags, xflags, data and ext are zero");
    assert_eq!(USER_WAKE_EVENT.filter, EVFILT_USER);
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

/// The constants the SDK ships are the SDK's, read at test time (M44 R5's method).
/// `EVFILT_MEMORYSTATUS` is xnu-private (`event_private.h:81`), and this asserts the SDK still
/// lacks it, so a future SDK that ships it is noticed and cited instead.
#[test]
fn the_constants_are_the_sdks_where_the_sdk_has_them() {
    use retrace_arch as a;
    let ev = sdk_header("sys/event.h");
    for (name, v) in [
        ("EVFILT_TIMER", i64::from(a::EVFILT_TIMER)), ("EV_DELETE", i64::from(a::EV_DELETE)),
        ("EV_ONESHOT", i64::from(a::EV_ONESHOT)), ("EV_DISPATCH", i64::from(a::EV_DISPATCH)),
        ("EV_UDATA_SPECIFIC", i64::from(a::EV_UDATA_SPECIFIC)), ("NOTE_TRIGGER", i64::from(a::NOTE_TRIGGER)),
        ("NOTE_NSECONDS", i64::from(a::NOTE_NSECONDS)), ("NOTE_ABSOLUTE", i64::from(a::NOTE_ABSOLUTE)),
        ("NOTE_LEEWAY", i64::from(a::NOTE_LEEWAY)), ("NOTE_CRITICAL", i64::from(a::NOTE_CRITICAL)),
        ("NOTE_BACKGROUND", i64::from(a::NOTE_BACKGROUND)),
        ("NOTE_MACH_CONTINUOUS_TIME", i64::from(a::NOTE_MACH_CONTINUOUS_TIME)),
        ("NOTE_MACHTIME", i64::from(a::NOTE_MACHTIME)),
        ("KEVENT_FLAG_ERROR_EVENTS", i64::from(a::KEVENT_FLAG_ERROR_EVENTS)),
    ] {
        assert_eq!(define(&ev, name), Some(v), "{name}");
    }
    assert_eq!(define(&ev, "EVFILT_MEMORYSTATUS"), None,
        "the SDK now defines EVFILT_MEMORYSTATUS: cite it from the SDK instead of xnu");
}
