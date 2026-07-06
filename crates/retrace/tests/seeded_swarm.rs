use std::process::Command;
use retrace_sim::{Rng, pick_fault, apply_fault};

// CARGO_BIN_EXE_retrace is a separate binary that this test spawns itself — it never
// passes through `.cargo/config.toml`'s codesign `runner`, so it lacks the hypervisor
// entitlement and every hv_* call would get HV_DENIED. Sign it here the same way e2e.rs
// does before exec'ing it (see tools/codesign-run.sh, retrace.entitlements).
fn bin() -> &'static str {
    let p = env!("CARGO_BIN_EXE_retrace");
    let ent = concat!(env!("CARGO_MANIFEST_DIR"), "/../../retrace.entitlements");
    let out = Command::new("codesign")
        .args(["-s", "-", "-f", "--entitlements", ent, p])
        .output()
        .expect("codesign");
    assert!(out.status.success(), "codesign -f --entitlements failed for {p}: {}", String::from_utf8_lossy(&out.stderr));
    p
}

// The M0 exit gate: record hello, inject a seeded trace-IO fault, replay. Every seed must
// end in a byte-identical `hello\n`/exit-0 replay OR a clean, named divergence (exit 3) —
// never a panic, a silent wrong answer, or any other exit code.
#[test]
fn n_seeds_never_diverge_silently() {
    const N: u64 = 200;
    let bin = bin();
    for seed in 0..N {
        let trace = std::env::temp_dir().join(format!("retrace-swarm-{}-{seed}.bin", std::process::id()));

        // Record (subprocess = its own VM).
        let rec = Command::new(bin)
            .args(["record", retrace_guest::HELLO, "-o", trace.to_str().unwrap()])
            .output().unwrap();
        assert!(rec.status.success(), "seed {seed}: record failed: {}", String::from_utf8_lossy(&rec.stderr));

        // Inject a seeded trace-IO fault. record_offsets are cumulative on-disk record
        // boundaries computed exactly as the writer framed them (8-byte header + body),
        // so TruncateAfter(index) cuts on a real record boundary.
        let events = retrace_trace::Reader::open(&trace).unwrap();
        let mut bytes = std::fs::read(&trace).unwrap();
        let mut offsets = vec![0usize];
        {
            let mut off = 0usize;
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

        // Replay must exit 0 (identical) OR 3 (named divergence) — never anything else, never a panic.
        let rep = Command::new(bin).args(["replay", trace.to_str().unwrap()]).output().unwrap();
        let code = rep.status.code().unwrap_or(-1);
        assert!(code == 0 || code == 3,
            "seed {seed} fault {fault:?}: replay exit {code} (expected 0 or 3)\nstderr: {}",
            String::from_utf8_lossy(&rep.stderr));
        if code == 0 {
            assert_eq!(rep.stdout, b"hello\n", "seed {seed} fault {fault:?}: exit 0 but wrong output");
        }

        let _ = std::fs::remove_file(&trace);
    }
}
