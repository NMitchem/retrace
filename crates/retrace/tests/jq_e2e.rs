// M9 rung-2 gate. `brew jq` — the first guest that loads dylibs which are NOT in the dyld shared
// cache (libjq.1.dylib and libonig.5.dylib, both files on disk under /opt/homebrew).
//
// `-n` (null input) is deliberate: it makes jq do real work driven by a real argument without
// opening the stdin surface, so this gate tests one new capability, not three.
//
// NOT a repo artifact: jq comes from Homebrew. When it is absent the test announces the skip loudly
// rather than passing quietly — a silent skip reads as a green it did not earn.
mod util;

const JQ: &str = "/opt/homebrew/bin/jq";

#[test]
fn jq_records_and_replays() {
    if !std::path::Path::new(JQ).exists() {
        eprintln!("SKIPPED jq_records_and_replays: {JQ} not installed (`brew install jq`). \
                   This gate did NOT run — it is not evidence of anything.");
        return;
    }
    let out = util::assert_rung_records_and_replays(JQ, &["-n", "1+1"], b"2\n");
    assert_eq!(out.stdout, b"2\n");
}
