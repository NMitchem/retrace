mod util;
#[test]
fn remap_records_and_replays() {
    let (rec, trace) = util::record(retrace_guest::REMAP);
    assert_eq!(rec.code, 0);
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, rec.stdout, "replay stdout must match record stdout");
}
