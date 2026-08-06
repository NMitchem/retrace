// M11 headline guest: a real full-std Rust binary that panics. Built with -C panic=abort, so
// libstd's panic path ends in abort(), which raises SIGABRT on itself — the exact path that killed
// the recorder before M11.
//
// The -C panic=abort is load-bearing and was MEASURED, not assumed: with the default panic=unwind,
// `panic!()` in main unwinds back to lang_start, prints the panic message, and the process exits
// 101 — it never raises a signal at all, so the guest would exercise nothing this milestone added.
// Under panic=abort the same program exits 134 (= 128 + SIGABRT). Verified natively on this host
// before the fixture was wired in.
//
// Prints first so the trace proves it reached its own code rather than dying inside dyld.
fn main() {
    println!("about to panic");
    panic!("M11");
}
