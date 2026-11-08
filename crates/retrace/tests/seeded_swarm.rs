use std::process::Command;
use retrace_sim::{Rng, Fault, pick_fault, apply_fault};

mod util;

// The M1 exit gate: record a general-syscall guest, inject a seeded trace-IO fault, replay.
// Every seed must end in one of three outcomes: a byte-identical replay of the guest's known
// output (exit 0), a clean, named divergence (exit 3), or an incomplete recording (exit 4) -
// never a panic, a silent wrong answer, or any other exit code. Since M50, a fault that removes
// the terminal event replays as an incomplete recording (exit 4), the same as a killed recorder's
// trace; that outcome is allowed only for a real fault, with one named line, no panic or bug
// banner, and output that is a prefix of the expected output. Run over both M1 guests: the static
// file-I/O guest (open/fstat/read/write/close, exercising the memory-diff engine on a real
// kernel-filled buffer) and the anonymous-mmap guest (exercising the mmap special case + the
// final full-memory divergence oracle).
fn swarm_over(bin: &str, tag: &str, guest: &str, expected_stdout: &[u8]) {
    const N: u64 = 200;
    for seed in 0..N {
        // `tag` keeps the two guests' trace files distinct even if both tests in this
        // binary happen to run concurrently (the gate always passes --test-threads=1,
        // but the filenames shouldn't rely on that to avoid colliding).
        let trace = std::env::temp_dir().join(format!("retrace-swarm-{tag}-{}-{seed}.bin", std::process::id()));

        // Record (subprocess = its own VM).
        let rec = Command::new(bin)
            .args(["record", "-o", trace.to_str().unwrap(), guest])
            .output().unwrap();
        assert!(rec.status.success(), "seed {seed}: record failed: {}", String::from_utf8_lossy(&rec.stderr));

        // Inject a seeded trace-IO fault. record_offsets are cumulative on-disk record
        // boundaries computed exactly as the writer framed them (4-byte magic header, then
        // each record's 8-byte header + body), so TruncateAfter(index) cuts on a real record
        // boundary. The accumulator starts at 4 (past the magic), not 0.
        let events = retrace_trace::Reader::open(&trace).unwrap();
        let mut bytes = std::fs::read(&trace).unwrap();
        let mut offsets = vec![4usize];
        {
            let mut off = 4usize;
            for e in &events {
                let body = bincode::serialize(e).unwrap();
                off += 8 + body.len();
                offsets.push(off);
            }
        }
        let mut rng = Rng::seed(seed);
        let fault = pick_fault(&mut rng, events.len());
        apply_fault(&mut bytes, &fault, &offsets);
        std::fs::write(&trace, &bytes).unwrap();

        // Replay must exit 0 (identical), 3 (named divergence) or 4 (incomplete recording) - never
        // anything else, never a panic.
        let rep = Command::new(bin).args(["replay", trace.to_str().unwrap()]).output().unwrap();
        let code = rep.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&rep.stderr);
        assert!(code == 0 || code == 3 || code == 4,
            "guest {guest} seed {seed} fault {fault:?}: replay exit {code} (expected 0, 3 or 4)\nstderr: {stderr}");
        if code == 0 {
            assert_eq!(rep.stdout, expected_stdout, "guest {guest} seed {seed} fault {fault:?}: exit 0 but wrong output");
        }
        if code == 3 {
            assert!(stderr.contains("DIVERGENCE"),
                "guest {guest} seed {seed}: exit 3 but stderr did not name the divergence: {stderr}");
        }
        if code == 4 {
            assert!(!matches!(fault, Fault::None),
                "guest {guest} seed {seed}: exit 4 with no fault injected\nstderr: {stderr}");
            let lines: Vec<&str> = stderr.lines().filter(|l| l.starts_with("retrace: incomplete recording:")).collect();
            assert!(lines.len() == 1 && lines[0].contains("#incomplete-recordings"),
                "guest {guest} seed {seed} fault {fault:?}: exit 4 without exactly one incomplete-recording line: {stderr}");
            assert!(!stderr.contains("panicked at") && !stderr.contains("retrace: internal error"),
                "guest {guest} seed {seed} fault {fault:?}: exit 4 with a panic or the bug banner: {stderr}");
            assert!(expected_stdout.starts_with(&rep.stdout),
                "guest {guest} seed {seed} fault {fault:?}: exit 4 but stdout is not a prefix of the expected output");
        }
        let _ = std::fs::remove_file(&trace);
    }
}

#[test]
fn n_seeds_never_diverge_silently_fileio() {
    swarm_over(util::bin(), "fileio", retrace_guest::FILEIO, b"retrace-m1-fixture\n");
}

#[test]
fn n_seeds_never_diverge_silently_mmap() {
    swarm_over(util::bin(), "mmap", retrace_guest::MMAPGUEST, &[0xAB, 0xCD]);
}
