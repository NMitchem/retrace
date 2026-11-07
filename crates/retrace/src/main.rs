use std::path::Path;
use std::process::exit;

mod debug;
mod rsp;
mod gdbserver;
mod lldbcmd;

/// Where the bug banner sends a report (M50 §3b).
const ISSUES_URL: &str = "https://github.com/NMitchem/retrace/issues";
const USAGE: &str = "usage: retrace <record [-o <trace>] <program> [<args>...] | replay <trace> \
                     | lldb <trace> [--exe <path>] [-- <lldb args>...] | debug <trace> --script '...' \
                     | gdbserver <trace> [--port <n>] [--exe <path>]>";
const RECORD_USAGE: &str = "usage: retrace record [-o <trace>] <program> [<args>...]";
const HELP: &str = "usage: retrace <command>

  record [-o <trace>] <program> [<args>...]   record a program (static or dynamic); -o defaults to <program>.retrace
  replay <trace>                              replay a recording, bit for bit
  lldb <trace> [--exe <path>] [-- <lldb args>...]   debug a recording in lldb, with reverse commands
  debug <trace> --script '<cmds>'             run debugger commands over a recording
  gdbserver <trace> [--port <n>] [--exe <path>]   serve a recording to a gdb-remote client

  retrace --version    print the version

Programs retrace cannot run yet are listed in docs/walls.md.";

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

/// The options every CLI recording takes: the live sink, and the program's environment.
fn live(envp: Vec<Vec<u8>>) -> retrace_core::RecordOpts {
    retrace_core::RecordOpts { sink: Some(Box::new(LiveOutput::default())), envp }
}

/// M51 §3f (R2): the environment the program gets: retrace's own, as raw `name=value` bytes in the
/// order it arrived, minus every `RETRACE_*` variable, which configures retrace and not the program.
///
/// It is read from the kernel's own envp, the array that follows argv's terminator, and not from
/// `std::env`: a framework retrace links (CoreFoundation, under Hypervisor) `setenv`s
/// `__CF_USER_TEXT_ENCODING` into the live `environ` at load when it is absent, a variable the
/// program would then see though nothing passed it (measured by `envp_e2e`: native count 3, recorded 4).
///
/// `main` calls this first and keeps the result (`PROGRAM_ENV`), so the read happens as early as it
/// can; a load-time constructor in a linked framework still runs before it and is outside our reach.
static PROGRAM_ENV: std::sync::OnceLock<Vec<Vec<u8>>> = std::sync::OnceLock::new();

fn program_env() -> Vec<Vec<u8>> {
    PROGRAM_ENV.get_or_init(read_kernel_envp).clone()
}

fn read_kernel_envp() -> Vec<Vec<u8>> {
    extern "C" {
        fn _NSGetArgc() -> *mut i32;
        fn _NSGetArgv() -> *mut *mut *const std::ffi::c_char;
    }
    let mut env = Vec::new();
    // SAFETY: on macOS the kernel lays out argc, argv[argc], NULL, envp[..], NULL. Darwin's libc
    // leaves that array untouched when `setenv` adds a new name (the CoreFoundation case), but edits
    // it in place when a variable is overwritten (`setenv(.., 1)`, `putenv`) and shifts it on
    // `unsetenv`. So it is intact unless something in this process overwrites or unsets an existing
    // variable before this read; nothing in retrace does today.
    unsafe {
        // argc from the kernel is never negative, so the cast cannot wrap.
        let argc = *_NSGetArgc() as usize;
        let mut p = (*_NSGetArgv()).add(argc + 1);
        while !(*p).is_null() {
            let e = std::ffi::CStr::from_ptr(*p).to_bytes();
            if !e.starts_with(b"RETRACE_") { env.push(e.to_vec()); }
            p = p.add(1);
        }
    }
    env
}

fn main() {
    let _ = program_env();
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
                say(&u.line());
                if matches!(a.get(1).map(String::as_str), Some("debug" | "gdbserver")) { 5 } else { 4 }
            }
            None => 70,
        },
    };
    exit(code);
}

/// One subcommand, as its exit code (spec §3b): the guest's own exit, or 128 + its signal; 2 for bad
/// input to retrace; 3 for a divergence; 4 for a wall; 5 for a `debug` or `gdbserver` error. `lldb`
/// exits with lldb's own status, or with the server's code (5) when it ends before lldb starts.
fn run(a: &[String]) -> i32 {
    // M50 §3f: the bug banner's control, in debug builds only. A release build cannot reach it, so it
    // adds no bug path a program could take; the panic stands in for a broken internal invariant.
    #[cfg(debug_assertions)]
    if std::env::var_os("RETRACE_DEBUG_PANIC").is_some() {
        panic!("RETRACE_DEBUG_PANIC is set: a deliberate internal error, the bug-banner control (M50 §3f)");
    }
    match a.get(1).map(String::as_str) {
        Some("record") => record(a),
        Some("record-dyn") => bad_input("record-dyn is now record: retrace record [-o <trace>] <program> [<args>...]"),
        Some("--version" | "-V") => { println!("retrace {}", env!("CARGO_PKG_VERSION")); 0 }
        Some("--help" | "-h" | "help") => { println!("{HELP}"); 0 }
        Some("replay") => replay(a),
        Some("debug") => debug_cmd(a),
        Some("gdbserver") => gdbserver_cmd(a),
        Some("lldb") => lldbcmd::run(a),
        _ => bad_input(USAGE),
    }
}

/// Bad input to retrace itself (R2): one `retrace:` line, and exit 2.
fn bad_input(line: &str) -> i32 {
    say(&format!("retrace: {line}"));
    2
}

/// R2: the one line for a process the hypervisor refuses, most often for want of the entitlement.
fn no_hypervisor(why: &str) -> i32 {
    let exe = std::env::current_exe().unwrap_or_else(|_| "retrace".into());
    bad_input(&format!("cannot create a VM ({why}). retrace needs the com.apple.security.hypervisor \
        entitlement. {}", signing_fix(&exe)))
}

/// The fix text for an unsigned `exe` (M56 section 3f), one that works without the repository: the
/// entitlement file beside a Homebrew install, else a self-contained command (T0-6's two-line form),
/// and the repository form only when `exe` sits under a tree that holds both files.
fn signing_fix(exe: &Path) -> String {
    let me = exe.display();
    if let Some(bin) = exe.parent() {
        let shared = bin.join("../share/retrace/retrace.entitlements");
        if shared.is_file() {
            return format!("Sign it with: codesign --sign - --force --entitlements {} {me}", shared.display());
        }
    }
    for dir in exe.ancestors().skip(1) {
        let ent = dir.join("retrace.entitlements");
        if ent.is_file() && dir.join("Cargo.toml").is_file() {
            return format!("Sign it with: codesign --sign - --force --entitlements {} {me}", ent.display());
        }
    }
    format!("Sign it with these two commands: printf '%s\\n' '<?xml version=\"1.0\" encoding=\"UTF-8\"?>' \
        '<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">' \
        '<plist version=\"1.0\"><dict><key>com.apple.security.hypervisor</key><true/></dict></plist>' \
        > /tmp/retrace.entitlements ; codesign --sign - --force --entitlements /tmp/retrace.entitlements {me}")
}

/// What `retrace record` was given (M56 section 3b): options come before the program, and
/// everything after the program is the program's.
#[derive(Debug, PartialEq)]
struct RecordArgs { out: Option<String>, program: String, args: Vec<String> }

/// `a` is the whole argv. `Err` is the usage line.
fn parse_record_args(a: &[String]) -> Result<RecordArgs, String> {
    let mut i = 2;
    let mut out = None;
    loop {
        match a.get(i).map(String::as_str) {
            Some("-o") => match a.get(i + 1) {
                Some(t) => { out = Some(t.clone()); i += 2; }
                None => return Err(RECORD_USAGE.into()),
            },
            Some("--") => { i += 1; break; }
            _ => break,
        }
    }
    match a.get(i) {
        Some(p) => Ok(RecordArgs { out, program: p.clone(), args: a[i + 1..].to_vec() }),
        None => Err(RECORD_USAGE.into()),
    }
}

/// The default trace name: the program's basename plus `.retrace`, in the working directory.
fn default_trace_name(program: &str) -> String {
    let base = Path::new(program).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| program.into());
    format!("{base}.retrace")
}

/// `record [-o <trace>] <program> [<args>...]`. LC_LOAD_DYLINKER present records through the named
/// dylinker (real dyld); absent, the freestanding static path.
fn record(a: &[String]) -> i32 {
    let r = match parse_record_args(a) { Ok(r) => r, Err(u) => return bad_input(&u) };
    let guest = &r.program;
    let bytes = match std::fs::read(guest) {
        Ok(b) => b,
        Err(e) => return bad_input(&format!("cannot read {guest}: {e}")),
    };
    let exe = match retrace_guest::try_parse_macho(&bytes) {
        Ok(l) => l,
        Err(why) => return bad_input(&format!("{guest} {why}")),
    };
    let dynamic = exe.dylinker.is_some();
    if !dynamic && !r.args.is_empty() {
        return bad_input(&format!("a static program takes no arguments ({guest}); {RECORD_USAGE}"));
    }
    let named = r.out.is_some();
    let out = r.out.clone().unwrap_or_else(|| default_trace_name(guest));
    // Guest argv: argv[0] is the path as typed (what dyld's `executable_path=` is derived from).
    let mut argv = vec![guest.clone()];
    argv.extend(r.args.iter().cloned());
    let envp = if dynamic { program_env() } else { Vec::new() };
    // M51 section 3f (R3): refused before the VM exists, so nothing is written.
    let size = retrace_core::start_args_bytes(&argv, &envp);
    if size > retrace_core::START_ARGS_LIMIT {
        return bad_input(&format!("the arguments and environment take {} KiB, more than retrace's limit of {} KiB - run it \
            under `env -i` with only what the program needs", size.div_ceil(1024), retrace_core::START_ARGS_LIMIT / 1024));
    }
    if let Err(why) = retrace_core::hypervisor_check() { return no_hypervisor(&why); }
    let summary = if dynamic {
        let dyld_path = exe.dylinker.clone().unwrap_or_else(|| retrace_guest::DYLD_PATH.to_string());
        let dyld_bytes = std::fs::read(&dyld_path).unwrap_or_else(|e| panic!("read dyld {dyld_path}: {e}"));
        let dyld = retrace_guest::parse_macho(retrace_guest::slice_arm64e(&dyld_bytes));
        retrace_core::record_dynamic_opts(&exe, &dyld, &argv, Path::new(&out), live(envp))
    } else {
        retrace_core::record_opts(&exe, Path::new(&out), live(envp))
    };
    match summary {
        Ok(s) => {
            let code = finish(s.fall_throughs, &s.outcome);
            if !named { say(&format!("retrace: saved the recording to {out}")); }
            code
        }
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
            say(&format!("DIVERGENCE at landmark {} pc=0x{:x}: {}", d.landmark, d.pc, d.detail));
            say(&format!("repro: retrace replay {trace}"));
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
        retrace_core::Outcome::Crash { sig, .. } | retrace_core::Outcome::Signal { sig } => {
            // M51 §3c: the program's output is already printed; one closing line, and native's code.
            if let Some(l) = retrace_core::closing_line(outcome) { say(&l); }
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
        Err(e) => { say(&format!("DEBUG ERROR: {e}")); 5 }
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
        Err(e) => { say(&format!("GDBSERVER ERROR: {e}")); 5 }
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

#[cfg(test)]
mod record_args_tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> { a.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn a_bare_program() {
        let r = parse_record_args(&v(&["retrace", "record", "./p"])).unwrap();
        assert_eq!(r, RecordArgs { out: None, program: "./p".into(), args: vec![] });
    }

    #[test]
    fn options_after_the_program_are_the_programs() {
        let r = parse_record_args(&v(&["retrace", "record", "-o", "t", "./p", "-o", "x", "--help"])).unwrap();
        assert_eq!(r, RecordArgs { out: Some("t".into()), program: "./p".into(), args: v(&["-o", "x", "--help"]) });
    }

    #[test]
    fn dashdash_ends_retraces_options() {
        let r = parse_record_args(&v(&["retrace", "record", "--", "-p"])).unwrap();
        assert_eq!(r.program, "-p");
    }

    #[test]
    fn missing_operands_are_errors() {
        for a in [&["retrace", "record"][..], &["retrace", "record", "-o"], &["retrace", "record", "-o", "t"]] {
            assert!(parse_record_args(&v(a)).is_err(), "{a:?}");
        }
    }

    #[test]
    fn default_names() {
        assert_eq!(default_trace_name("./sub/crashy"), "crashy.retrace");
        assert_eq!(default_trace_name("/bin/ls"), "ls.retrace");
    }
}

#[cfg(test)]
mod signing_fix_tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("retrace-signfix-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_fix_names_a_file_that_exists_or_needs_none() {
        // A Homebrew-shaped prefix: the entitlement sits in share/retrace beside bin.
        let brew = tmp("brew");
        std::fs::create_dir_all(brew.join("bin")).unwrap();
        std::fs::create_dir_all(brew.join("share/retrace")).unwrap();
        std::fs::write(brew.join("share/retrace/retrace.entitlements"), "x").unwrap();
        let exe = brew.join("bin/retrace");
        let line = signing_fix(&exe);
        let named = brew.join("bin/../share/retrace/retrace.entitlements");
        assert!(line.contains(&named.display().to_string()), "{line}");
        assert!(named.exists());

        // A bare directory: no file to name, so the self-contained command, never a repo path.
        let bare = tmp("bare");
        let line = signing_fix(&bare.join("retrace"));
        assert!(line.contains("com.apple.security.hypervisor"), "{line}");
        assert!(line.contains("codesign"), "{line}");
        assert!(!line.contains("root of the retrace repository"), "{line}");
        assert!(!line.contains("--entitlements retrace.entitlements"), "{line}");

        // A repo-shaped tree: the file exists at the root, with Cargo.toml.
        let repo = tmp("repo");
        std::fs::create_dir_all(repo.join("target/debug")).unwrap();
        std::fs::write(repo.join("Cargo.toml"), "").unwrap();
        std::fs::write(repo.join("retrace.entitlements"), "x").unwrap();
        let line = signing_fix(&repo.join("target/debug/retrace"));
        assert!(line.contains(&repo.join("retrace.entitlements").display().to_string()), "{line}");
    }
}
