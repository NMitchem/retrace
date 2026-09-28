// M44 t0 M3: the throwaway test that produced t0-m3-paths.log and t0-m3-paths2.log. It ran as
// crates/retrace/tests/t0_m3_paths.rs (deleted after the run, never committed there) with
// `export T0_PATHS=<list>` then `cargo test -p retrace --test t0_m3_paths -- --test-threads=1 --nocapture`.
// For each `<trace> <landmark> <addr>...` line, it seeks the recording to the trap that ENDS window
// `landmark` (the window's full length, where the syscall's argument registers point at its path
// strings) and prints the NUL-terminated string at each guest address.
use std::path::Path;

#[test]
fn t0_m3_paths() {
    let list = std::fs::read_to_string(std::env::var("T0_PATHS").expect("T0_PATHS")).unwrap();
    for line in list.lines().filter(|l| !l.trim().is_empty()) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let tr = Path::new(f[0]);
        let n: usize = f[1].parse().unwrap();
        let len = retrace_core::seek(tr, n, 0).unwrap().window_len_here().unwrap();
        let s = retrace_core::seek(tr, n, len).unwrap();
        for a in &f[2..] {
            let addr = u64::from_str_radix(a.trim_start_matches("0x"), 16).unwrap();
            let bytes = s.read_mem(addr, 256).unwrap_or_default();
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            println!("T0M3P {} landmark {n} pc={:#x} {a} = {:?}", tr.display(), s.pc(), String::from_utf8_lossy(&bytes[..end]));
        }
    }
}
