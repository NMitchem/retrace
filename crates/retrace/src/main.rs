use std::path::Path;
use std::process::exit;

mod debug;
mod rsp;
mod gdbserver;

/// Where the bug banner sends a report (M50 §3b).
const ISSUES_URL: &str = "https://github.com/NMitchem/retrace/issues";
const USAGE: &str = "usage: retrace <record <guest> -o <trace> | record-dyn <exe> -o <trace> [-- <guest args...>] \
                     | replay <trace> | debug <trace> --script '...' | gdbserver <trace> [--port <n>] [--exe <path>]>";

/// One of retrace's own lines on its stderr. A reader that went away is not retrace's failure, so a
/// failed write is dropped rather than panicking, as `eprintln!` would (Review Focus 1).
fn say(line: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{line}");
}

/// M51 §3e: the program's console writes, printed on retrace's own stdout and stderr as it makes
/// them. stdout is flushed per write, so a terminal or a `2>&1` file interleaves the two streams in
/// the program's order. R5: a write that fails ends retrace's writes to that stream, and the
/// recording goes on; the program's own write returned its length already, so nothing about this
/// reaches the trace. A reader that went away (Rust ignores SIGPIPE, so it is EPIPE) is silent. Any
/// other error is reported once per stream through `say`, since it is not the reader's choice.
#[derive(Default)]
struct LiveOutput { stdout_gone: bool, stderr_gone: bool }

impl retrace_core::OutputSink for LiveOutput {
    fn write(&mut self, stream: retrace_core::Stream, bytes: &[u8]) {
        use std::io::Write;
        let gone = match stream {
            retrace_core::Stream::Stdout => &mut self.stdout_gone,
            retrace_core::Stream::Stderr => &mut self.stderr_gone,
        };
        if *gone { return; }
        let r = match stream {
            retrace_core::Stream::Stdout => {
                let mut o = std::io::stdout().lock();
                o.write_all(bytes).and_then(|()| o.flush())
            }
            retrace_core::Stream::Stderr => std::io::stderr().lock().write_all(bytes),
        };
        if let Err(e) = r {
            *gone = true;
            if let Some(line) = write_failure_line(&e) { say(&line); }
        }
    }
}

/// Only a vanished reader is silent. Any other failure (a full disk) is retrace failing to deliver
/// the output, so it is said; the caller sets `gone` first, so that is once per stream.
fn write_failure_line(e: &std::io::Error) -> Option<String> {
    (e.kind() != std::io::ErrorKind::BrokenPipe)
        .then(|| format!("retrace: cannot write the program's output: {e}"))
}


/// The options every CLI recording takes: the live sink.
fn live() -> retrace_core::RecordOpts {
    retrace_core::RecordOpts { sink: Some(Box::new(LiveOutput::default())) }
}

fn main() {
    // M50 §3c, C7: a panic that is not a wall is a bug in retrace. The hook prints the banner and
    // returns, and the catch below exits 70 (EX_SOFTWARE, R3: 101 is also what a Rust guest's own
    // panic exits). A wall's panic is silent here: `record` and `replay` catch it themselves and print
    // its line after the guest's output.
    std::panic::set_hook(Box::new(|info| {
        if retrace_arch::Unsupported::from_panic(info.payload()).is_some() { return; }
        let msg = info.payload().downcast_ref::<String>().map(String::as_str)
            .or_else(|| info.payload().downcast_ref::<&str>().copied())
            .unwrap_or("a panic with no message");
        let msg = msg.split_whitespace().collect::<Vec<_>>().join(" ");
        let at = info.location().map(|l| format!(" (at {}:{}:{})", l.file(), l.line(), l.column())).unwrap_or_default();
        eprintln!("retrace: internal error - this is a bug in retrace, not in your program.");
        eprintln!("  please report it at {ISSUES_URL} and include the next line:");
        eprintln!("  {msg}{at}");
        // RUST_BACKTRACE still works for a bug (spec §3c). A wall never reaches this line.
        let bt = std::backtrace::Backtrace::capture();
        if bt.status() == std::backtrace::BacktraceStatus::Captured { eprintln!("{bt}"); }
    }));
    let a: Vec<String> = std::env::args().collect();
    let code = match std::panic::catch_unwind(|| run(&a)) {
        Ok(code) => code,
        Err(payload) => match retrace_arch::Unsupported::from_panic(payload.as_ref()) {
            // C7: only `debug` and `gdbserver` drive replay without `replay()`'s own catch.
            Some(u) => {
                eprintln!("{}", u.line());
                if matches!(a.get(1).map(String::as_str), Some("debug" | "gdbserver")) { 5 } else { 4 }
            }
            None => 70,
        },
    };
    exit(code);
}

/// One subcommand, as its exit code (spec §3b): the guest's own exit, or 128 + its signal; 2 for bad
/// input to retrace; 3 for a divergence; 4 for a wall; 5 for a `debug` or `gdbserver` error.
fn run(a: &[String]) -> i32 {
    // M50 §3f: the bug banner's control, in debug builds only. A release build cannot reach it, so it
    // adds no bug path a program could take; the panic stands in for a broken internal invariant.
    #[cfg(debug_assertions)]
    if std::env::var_os("RETRACE_DEBUG_PANIC").is_some() {
        panic!("RETRACE_DEBUG_PANIC is set: a deliberate internal error, the bug-banner control (M50 §3f)");
    }
    match a.get(1).map(String::as_str) {
        Some("record") => record(a, false),
        Some("record-dyn") => record(a, true),
        Some("replay") => replay(a),
        Some("debug") => debug_cmd(a),
        Some("gdbserver") => gdbserver_cmd(a),
        _ => bad_input(USAGE),
    }
}

/// Bad input to retrace itself (R2): one `retrace:` line, and exit 2.
fn bad_input(line: &str) -> i32 {
    eprintln!("retrace: {line}");
    2
}

/// R2: the one line for a process the hypervisor refuses, most often for want of the entitlement.
fn no_hypervisor(why: &str) -> i32 {
    let me = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "retrace".into());
    bad_input(&format!("cannot create a VM ({why}). retrace needs the com.apple.security.hypervisor \
        entitlement: sign it with codesign --sign - --force --entitlements retrace.entitlements {me}, \
        using retrace.entitlements from the root of the retrace repository"))
}

/// The value after `flag` among retrace's own arguments, which end at `--`; None when the flag is
/// absent or has no value. A guest argument after `--` is never read as retrace's.
fn flag_value<'a>(a: &'a [String], flag: &str) -> Option<&'a str> {
    let own = &a[..a.iter().position(|s| s == "--").unwrap_or(a.len())];
    own.iter().position(|s| s == flag).and_then(|i| own.get(i + 1)).map(String::as_str)
}

/// `record <guest> -o <trace>`, and `record-dyn <exe> -o <trace> [-- <guest args...>]`, which loads
/// the exe and its dylinker (/usr/lib/dyld, arm64e slice) and records it running through REAL dyld.
fn record(a: &[String], dynamic: bool) -> i32 {
    let usage = if dynamic { "usage: retrace record-dyn <exe> -o <trace> [-- <guest args...>]" }
                else { "usage: retrace record <guest> -o <trace>" };
    let (Some(guest), Some(out)) = (a.get(2).filter(|g| !g.starts_with('-')), flag_value(a, "-o")) else {
        return bad_input(usage);
    };
    let bytes = match std::fs::read(guest) {
        Ok(b) => b,
        Err(e) => return bad_input(&format!("cannot read {guest}: {e}")),
    };
    let exe = match retrace_guest::try_parse_macho(&bytes) {
        Ok(l) => l,
        Err(why) => return bad_input(&format!("{guest} {why}")),
    };
    if let Err(why) = retrace_core::hypervisor_check() { return no_hypervisor(&why); }
    let summary = if dynamic {
        // Guest argv: argv[0] is the exe path (what the kernel passes and what dyld's
        // `executable_path=` is derived from); everything after `--` is the guest's own.
        let mut argv = vec![guest.clone()];
        if let Some(i) = a.iter().position(|s| s == "--") { argv.extend_from_slice(&a[i + 1..]); }
        let dyld_path = exe.dylinker.clone().unwrap_or_else(|| retrace_guest::DYLD_PATH.to_string());
        let dyld_bytes = std::fs::read(&dyld_path).unwrap_or_else(|e| panic!("read dyld {dyld_path}: {e}"));
        let dyld = retrace_guest::parse_macho(retrace_guest::slice_arm64e(&dyld_bytes));
        retrace_core::record_dynamic_opts(&exe, &dyld, &argv, Path::new(out), live())
    } else {
        retrace_core::record_opts(&exe, Path::new(out), live())
    };
    match summary {
        Ok(s) => finish(s.fall_throughs, &s.outcome),
        Err(e) => bad_input(&format!("cannot write the trace {out}: {e}")),
    }
}

/// `replay <trace>`.
fn replay(a: &[String]) -> i32 {
    let Some(trace) = a.get(2) else { return bad_input("usage: retrace replay <trace>") };
    if let Err(e) = std::fs::metadata(trace) { return bad_input(&format!("cannot read {trace}: {e}")); }
    if let Err(why) = retrace_core::hypervisor_check() { return no_hypervisor(&why); }
    match retrace_core::replay_opts(Path::new(trace), Some(Box::new(LiveOutput::default()))) {
        Ok(r) => finish(r.fall_throughs, &r.outcome),
        Err(d) => {
            eprintln!("DIVERGENCE at landmark {} pc=0x{:x}: {}", d.landmark, d.pc, d.detail);
            eprintln!("repro: retrace replay {trace}");
            3
        }
    }
}

/// How the program ended, after its output (printed live), and the exit code (spec §3b).
fn finish(fall_throughs: u64, outcome: &retrace_core::Outcome) -> i32 {
    // M23 t2: both sides report the EL1 vector fall-through count so the gate can compare them
    // (crates/retrace/tests/fallthrough_e2e.rs). It is not in the trace — an Event variant would
    // renumber every landmark. M51 §3e (R4): it is a diagnostic, printed under RETRACE_VERBOSE.
    if retrace_arch::verbose() { say(&format!("[retrace] fall-throughs: {fall_throughs}")); }
    match *outcome {
        retrace_core::Outcome::Exit { code } => code as i32,
        retrace_core::Outcome::Crash { pc, esr, far } => {
            say(&format!("guest crashed: pc={pc:#x} far={far:#x} esr={esr:#x}"));
            139
        }
        retrace_core::Outcome::Signal { sig } => {
            say(&format!("guest terminated by signal {sig}"));
            // 128 + sig: the convention M6 already uses for a crash (139 == 128 + SIGSEGV).
            // SIGABRT therefore exits 134.
            128 + sig as i32
        }
        // M50 §3b: the guest's output is already printed; the wall's line comes last.
        retrace_core::Outcome::Refused(ref u) => { say(&u.line()); 4 }
        retrace_core::Outcome::Incomplete(ref at) => { say(&retrace_arch::incomplete_line(at)); 4 }
    }
}

/// `debug <trace> --script '<cmd>; <cmd>; …'`. A missing trace, a missing `--script`, or a
/// `--script` with no value is a usage error (exit 2), validated before any file I/O or VM work.
fn debug_cmd(a: &[String]) -> i32 {
    let script = a.iter().position(|s| s == "--script").and_then(|i| a.get(i + 1));
    let (Some(trace), Some(script)) = (a.get(2), script) else {
        return bad_input("usage: retrace debug <trace> --script '<cmds>'");
    };
    if let Err(why) = retrace_core::hypervisor_check() { return no_hypervisor(&why); }
    match debug::run_script(Path::new(trace), script, &mut std::io::stdout()) {
        Ok(()) => 0,
        Err(e) => { eprintln!("DEBUG ERROR: {e}"); 5 }
    }
}

/// `gdbserver <trace> [--port <n>] [--exe <path>]` (M43): a gdb-remote server for lldb over one
/// recording. Usage errors exit 2 before any socket or VM work. `opt` is None for an absent flag and
/// Some(None) for one given last, with no value: that is a usage error, never the default.
fn gdbserver_cmd(a: &[String]) -> i32 {
    let opt = |name: &str| a.iter().position(|s| s == name).map(|i| a.get(i + 1));
    let port = match opt("--port") { None => Some(0u16), Some(p) => p.and_then(|p| p.parse::<u16>().ok()) };
    let exe = match opt("--exe") { None => Some(None), Some(e) => e.map(|e| Some(e.clone())) };
    let (Some(trace), Some(port), Some(exe)) = (a.get(2).filter(|t| !t.starts_with("--")), port, exe) else {
        return bad_input("usage: retrace gdbserver <trace> [--port <n>] [--exe <path>]");
    };
    if let Err(why) = retrace_core::hypervisor_check() { return no_hypervisor(&why); }
    match gdbserver::serve(Path::new(trace), port, exe) {
        Ok(()) => 0,
        Err(e) => { eprintln!("GDBSERVER ERROR: {e}"); 5 }
    }
}

#[cfg(test)]
mod live_output_tests {
    use super::*;

    /// Std treats EBADF on stdio as success and macOS has no /dev/full, so no CLI run can make the
    /// sink's write fail for a reason other than EPIPE; the decision is pinned here instead.
    #[test]
    fn a_write_error_other_than_a_vanished_reader_is_reported() {
        let epipe = std::io::Error::from(std::io::ErrorKind::BrokenPipe);
        assert_eq!(write_failure_line(&epipe), None, "a vanished reader is silent (R5)");
        let full = std::io::Error::from_raw_os_error(28); // ENOSPC
        let line = write_failure_line(&full).expect("a full disk is reported");
        assert!(line.starts_with("retrace: cannot write the program's output: "), "{line}");
    }
}
