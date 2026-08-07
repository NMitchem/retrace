// M12: the five delivery fixtures build, export, and parse. Behaviour is Task 9's gate set — this
// only proves the build.rs wiring and the path constants, which is what Tasks 7 and 8 need in order
// to reference them.
//
// These guests CANNOT pass behaviourally yet and must not be made to: nothing delivers a signal
// until Task 7. Asserting anything about their exit codes here would be a test that goes green for
// the wrong reason.
#[test]
fn every_m12_guest_is_built_and_parses_as_a_macho() {
    for (name, path) in [
        ("sigframe", retrace_guest::SIGFRAME),
        ("segvcatch", retrace_guest::SEGVCATCH),
        ("altstack", retrace_guest::ALTSTACK),
        ("vecsurvive", retrace_guest::VECSURVIVE),
        ("blockedfault", retrace_guest::BLOCKEDFAULT),
    ] {
        let bytes =
            std::fs::read(path).unwrap_or_else(|e| panic!("{name} not built at {path}: {e}"));
        let loaded = retrace_guest::parse_macho(&bytes);
        assert!(loaded.entry != 0, "{name} has no entry point");
        assert!(!loaded.segments.is_empty(), "{name} has no segments");
    }
}
