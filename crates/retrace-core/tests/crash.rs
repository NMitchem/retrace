// M6: a stage-1 guest fault records as Event::Crash and replays bit-for-bit. Static guests
// (CRASH/CRASHJMP) — the dynamic path is Task 3.
use retrace_core::{record, replay, Outcome};
use retrace_trace::{Event, Reader, Writer};

const GARBAGE_VA: u64 = 0x4000_DEAD_0000; // mirrors asm/crash.s (source-defined, not layout)

// parse_macho(read(guest)) + a deterministic temp path (mirrors replay.rs's record_* helpers).
fn load_and_path(guest: &str, tag: &str) -> (retrace_guest::Loaded, std::path::PathBuf) {
    let loaded = retrace_guest::parse_macho(&std::fs::read(guest).unwrap());
    let p = std::env::temp_dir().join(format!("retrace-{tag}-{}.bin", std::process::id()));
    (loaded, p)
}

#[test]
fn data_abort_records_as_crash_and_replays() {
    let (loaded, trace) = load_and_path(retrace_guest::CRASH, "crash-da");
    let rec = record(&loaded, &trace).expect("record must SUCCEED on a guest crash");
    let Outcome::Crash { pc, esr, far } = rec.outcome else {
        panic!("expected crash outcome, got {:?}", rec.outcome);
    };
    assert_eq!(far, GARBAGE_VA);
    assert_eq!((esr >> 26) & 0x3f, 0x24, "lower-EL data abort EC");
    assert!(pc != 0);
    // The trace's terminal events are Crash then the final Snapshot.
    let events = Reader::open(&trace).unwrap();
    assert!(matches!(events[events.len() - 2], Event::Crash { .. }));
    assert!(matches!(events[events.len() - 1], Event::Snapshot { .. }));
    // Replay verifies the identical triple (the divergence oracle) — twice.
    for _ in 0..2 {
        let rep = replay(&trace).expect("replay of a crash trace succeeds");
        assert_eq!(rep.outcome, rec.outcome);
    }
}

#[test]
fn instruction_abort_records_as_crash() {
    let (loaded, trace) = load_and_path(retrace_guest::CRASHJMP, "crash-ia");
    let rec = record(&loaded, &trace).unwrap();
    let Outcome::Crash { pc, esr, far } = rec.outcome else { panic!("{:?}", rec.outcome) };
    assert_eq!((esr >> 26) & 0x3f, 0x20, "lower-EL instruction abort EC");
    assert_eq!(far, GARBAGE_VA);
    assert_eq!(pc, GARBAGE_VA, "instruction abort: the faulting pc IS the branch target");
    let rep = replay(&trace).unwrap();
    assert_eq!(rep.outcome, rec.outcome);
}

#[test]
fn perturbed_crash_triple_is_a_loud_divergence() {
    // Re-write the trace with a perturbed far via Writer (valid CRC — a raw byte flip would fail
    // the record CRC before the divergence compare ever ran) => replay must report Divergence.
    let (loaded, trace) = load_and_path(retrace_guest::CRASH, "crash-perturb");
    record(&loaded, &trace).unwrap();
    let events = Reader::open(&trace).unwrap();
    let tampered = trace.with_extension("tampered.bin");
    let mut w = Writer::create(&tampered).unwrap();
    for e in &events {
        match e {
            Event::Crash { pc, esr, far } =>
                w.append(&Event::Crash { pc: *pc, esr: *esr, far: far + 8 }).unwrap(),
            other => w.append(other).unwrap(),
        }
    }
    drop(w);
    let err = replay(&tampered).expect_err("perturbed crash triple must diverge");
    assert!(err.detail.contains("crash mismatch"), "got: {}", err.detail);
}
