// M10 rung-3 gate. `jq` reading a real file argument — the first guest that opens a path of its own
// and reads it, which is what makes the fd table load-bearing rather than merely motivated.
//
// **This capability already worked before M10.** Measured at HEAD 84983dc, before a line of the fd
// table existed: `jq '.name' t.json` recorded and replayed bit-for-bit, because the forward-and-
// record path captures the file's bytes as recorded kernel writes and replay executes no syscall.
// The gate exists to PIN it, not to claim it was newly earned — see the M10 Status section.
//
// NOT a repo artifact: jq comes from Homebrew. When absent, announce the skip loudly rather than
// passing quietly — a silent skip reads as a green it did not earn.
mod util;

const JQ: &str = "/opt/homebrew/bin/jq";
const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rung3.json");

#[test]
fn jq_reads_a_file_argument_and_replays() {
    if !std::path::Path::new(JQ).exists() {
        eprintln!("SKIPPED jq_reads_a_file_argument_and_replays: {JQ} not installed \
                   (`brew install jq`). This gate did NOT run — it is not evidence of anything.");
        return;
    }
    let out = util::assert_rung_records_and_replays(JQ, &[".name", FIXTURE], b"\"retrace\"\n");
    assert_eq!(out.stdout, b"\"retrace\"\n");
}

/// The trace must be self-contained: replay reads no file, so mutating the input afterwards cannot
/// change what replay produces. This is what proves the file's bytes live in the recording rather
/// than being re-read at replay time.
#[test]
fn the_replay_does_not_depend_on_the_input_file() {
    if !std::path::Path::new(JQ).exists() {
        eprintln!("SKIPPED the_replay_does_not_depend_on_the_input_file: {JQ} not installed \
                   (`brew install jq`). This gate did NOT run — it is not evidence of anything.");
        return;
    }
    // Record from a scratch copy so the repo fixture is never mutated.
    let scratch = std::env::temp_dir().join(format!("retrace-rung3-{}.json", std::process::id()));
    std::fs::copy(FIXTURE, &scratch).expect("copy fixture");
    let (rec, trace) = util::record_dynamic_args(JQ, &[".name", scratch.to_str().unwrap()]);
    assert_eq!(rec.code, 0, "record must exit 0. stderr:\n{}", rec.stderr);
    assert_eq!(rec.stdout, b"\"retrace\"\n");

    // Now change the input out from under the recording. Replay must not notice.
    std::fs::write(&scratch, b"{\"name\":\"TAMPERED\"}\n").expect("tamper");
    let rep = util::replay(&trace);
    assert_eq!(rep.code, 0, "replay must exit 0. stderr:\n{}", rep.stderr);
    assert_eq!(rep.stdout, b"\"retrace\"\n",
        "replay reproduced the TAMPERED input — the file's bytes are being re-read at replay time \
         instead of coming from the recording");
    let _ = std::fs::remove_file(&scratch);
}
