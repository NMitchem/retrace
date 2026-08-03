// M9. argc/argv must reach a real dynamically-linked guest through dyld's process-start stack.
mod util;

#[test]
fn argv_reaches_a_dynamic_guest_and_replays() {
    let out = util::assert_rung_records_and_replays(
        retrace_guest::ARGV_ECHO, &["M9-ARGV"], b"M9-ARGV\n");
    assert_eq!(out.stdout, b"M9-ARGV\n");
}

#[test]
fn no_argv_still_works() {
    // argc==1 must remain valid — every existing dynamic guest passes no arguments, and this is
    // what pins that the widening did not change the argc=1 layout dyld already accepts.
    let (rec, _trace) = util::record_dynamic(retrace_guest::ARGV_ECHO);
    assert_eq!(rec.code, 1, "with no argument the guest takes its NOARG branch. stderr:\n{}", rec.stderr);
    assert_eq!(rec.stdout, b"NOARG\n");
}
