use std::path::Path;
use std::process::exit;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("record") => {
            // retrace record <guest> -o <trace>
            let guest = &a[2];
            let out = a.iter().position(|s| s == "-o").map(|i| a[i+1].clone()).expect("-o <trace>");
            let bytes = std::fs::read(guest).expect("read guest");
            let loaded = retrace_guest::parse_macho(&bytes);
            match retrace_core::record(&loaded, Path::new(&out)) {
                Ok(s) => {
                    use std::io::Write;
                    std::io::stdout().write_all(&s.stdout).unwrap();
                    exit(s.exit_code as i32);
                }
                Err(e) => { eprintln!("RECORD ERROR: {e}"); exit(4); }
            }
        }
        Some("replay") => {
            let trace = &a[2];
            match retrace_core::replay(Path::new(trace)) {
                Ok(r) => {
                    use std::io::Write;
                    std::io::stdout().write_all(&r.stdout).unwrap();
                    exit(r.exit_code as i32);
                }
                Err(d) => {
                    eprintln!("DIVERGENCE at landmark {} pc=0x{:x}: {}", d.landmark, d.pc, d.detail);
                    eprintln!("repro: retrace replay {trace}");
                    exit(3);
                }
            }
        }
        _ => { eprintln!("usage: retrace <record <guest> -o <trace> | replay <trace>>"); exit(2); }
    }
}
