use std::path::Path;
use std::process::exit;

mod debug;

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
                    match s.outcome {
                        retrace_core::Outcome::Exit { code } => exit(code as i32),
                        retrace_core::Outcome::Crash { pc, esr, far } => {
                            eprintln!("guest crashed: pc={pc:#x} far={far:#x} esr={esr:#x}");
                            exit(139);
                        }
                        retrace_core::Outcome::Signal { sig } => {
                            eprintln!("guest terminated by signal {sig}");
                            // 128 + sig: the convention M6 already uses for a crash
                            // (139 == 128 + SIGSEGV). SIGABRT therefore exits 134.
                            exit(128 + sig as i32);
                        }
                    }
                }
                Err(e) => { eprintln!("RECORD ERROR: {e}"); exit(4); }
            }
        }
        Some("record-dyn") => {
            // retrace record-dyn <exe> -o <trace> [-- <guest args…>]: load the dynamically-linked
            // exe + its dylinker (/usr/lib/dyld, arm64e slice) and record it running through REAL
            // dyld.
            let guest = &a[2];
            let out = a.iter().position(|s| s == "-o").map(|i| a[i+1].clone()).expect("-o <trace>");
            // Guest argv: argv[0] is the exe path (what the kernel passes and what dyld's
            // `executable_path=` is derived from); everything after `--` is the guest's own.
            let mut argv = vec![guest.clone()];
            if let Some(i) = a.iter().position(|s| s == "--") { argv.extend_from_slice(&a[i+1..]); }
            let exe_bytes = std::fs::read(guest).expect("read guest");
            let exe = retrace_guest::parse_macho(&exe_bytes);
            let dyld_path = exe.dylinker.clone().unwrap_or_else(|| retrace_guest::DYLD_PATH.to_string());
            let dyld_bytes = std::fs::read(&dyld_path).unwrap_or_else(|e| panic!("read dyld {dyld_path}: {e}"));
            let dyld = retrace_guest::parse_macho(retrace_guest::slice_arm64e(&dyld_bytes));
            match retrace_core::record_dynamic(&exe, &dyld, &argv, Path::new(&out)) {
                Ok(s) => {
                    use std::io::Write;
                    std::io::stdout().write_all(&s.stdout).unwrap();
                    match s.outcome {
                        retrace_core::Outcome::Exit { code } => exit(code as i32),
                        retrace_core::Outcome::Crash { pc, esr, far } => {
                            eprintln!("guest crashed: pc={pc:#x} far={far:#x} esr={esr:#x}");
                            exit(139);
                        }
                        retrace_core::Outcome::Signal { sig } => {
                            eprintln!("guest terminated by signal {sig}");
                            // 128 + sig: the convention M6 already uses for a crash
                            // (139 == 128 + SIGSEGV). SIGABRT therefore exits 134.
                            exit(128 + sig as i32);
                        }
                    }
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
                    match r.outcome {
                        retrace_core::Outcome::Exit { code } => exit(code as i32),
                        retrace_core::Outcome::Crash { pc, esr, far } => {
                            eprintln!("guest crashed: pc={pc:#x} far={far:#x} esr={esr:#x}");
                            exit(139);
                        }
                        retrace_core::Outcome::Signal { sig } => {
                            eprintln!("guest terminated by signal {sig}");
                            // 128 + sig: the convention M6 already uses for a crash
                            // (139 == 128 + SIGSEGV). SIGABRT therefore exits 134.
                            exit(128 + sig as i32);
                        }
                    }
                }
                Err(d) => {
                    eprintln!("DIVERGENCE at landmark {} pc=0x{:x}: {}", d.landmark, d.pc, d.detail);
                    eprintln!("repro: retrace replay {trace}");
                    exit(3);
                }
            }
        }
        Some("debug") => {
            // retrace debug <trace> --script '<cmd>; <cmd>; …'
            // A missing trace, a missing `--script`, or a `--script` with no value is a usage error
            // (exit 2) — validated BEFORE any file I/O or VM work, never a panic.
            let script = a.iter().position(|s| s == "--script").and_then(|i| a.get(i + 1));
            match (a.get(2), script) {
                (Some(trace), Some(script)) => {
                    match debug::run_script(Path::new(trace), script, &mut std::io::stdout()) {
                        Ok(()) => exit(0),
                        Err(e) => { eprintln!("DEBUG ERROR: {e}"); exit(5); }
                    }
                }
                _ => { eprintln!("usage: retrace debug <trace> --script '<cmds>'"); exit(2); }
            }
        }
        _ => { eprintln!("usage: retrace <record <guest> -o <trace> | record-dyn <exe> -o <trace> [-- <guest args…>] | replay <trace> | debug <trace> --script '…'>"); exit(2); }
    }
}
