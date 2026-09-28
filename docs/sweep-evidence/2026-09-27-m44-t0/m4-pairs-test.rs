// M44 t0 M4: the throwaway test that produced m4-pairs.log. It ran as
// crates/retrace-arch/tests/t0_m4_pairs.rs (deleted after the run, never committed there) with
// `cargo test -p retrace-arch --test t0_m4_pairs -- --test-threads=1 --nocapture`, reading
// m4-pairs.txt (written by m4-pairs.sh from the SDK's sys/syscall.h).
#[test]
fn t0_m4_pairs() {
    let pairs = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"),
        "/../../.superpowers/sdd/2026-09-27-retrace-m44-owed/t0-m4-pairs.txt")).unwrap();
    for line in pairs.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        let (nc, ncn, p, pn) = (f[0], f[1].parse::<u64>().unwrap(), f[2], f[3]);
        let pn: u64 = pn.parse().unwrap();
        let a = retrace_arch::arg_kinds(pn);
        let b = retrace_arch::arg_kinds(ncn);
        let verdict = match (a, b) {
            (None, None) => "BOTH-NONE",
            (Some(x), Some(y)) if x == y => "SAME",
            (Some(_), Some(_)) => "DIFFERENT",
            _ => "ONE-SIDED",
        };
        println!("PAIR {p} {pn} {} | {nc} {ncn} {} | {verdict}", a.is_some(), b.is_some());
        if verdict != "BOTH-NONE" { println!("  plain={a:?}\n  nocancel={b:?}"); }
    }
}
