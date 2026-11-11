use std::path::{Path, PathBuf};
use std::process::exit;

mod debug;
mod demangle;
mod json;
mod rsp;
mod gdbserver;
mod image;
mod source;
mod tool;
mod unwind;
mod lldbcmd;

/// Where the bug banner sends a report.
const ISSUES_URL: &str = "https://github.com/NMitchem/retrace/issues";
const USAGE: &str = "usage: retrace <record [-o <trace>] <program> [<args>...] | replay <trace> \
                     | lldb <trace> [--exe <path>] [-- <lldb args>...] | debug <trace> [--json] [--script '...'] \
                     | gdbserver <trace> [--port <n>] [--exe <path>]>";
const RECORD_USAGE: &str = "usage: retrace record [-o <trace>] <program> [<args>...]";
/// The text of `--help`, and of `retrace` with no command.
fn help() -> String {
    format!("usage: retrace <command>

  record [-o <trace>] <program> [<args>...]   record a program (static or dynamic); the trace is <-o>, or
                                              the program's basename plus .retrace in the working directory
  replay <trace>                              replay a recording, bit for bit
  lldb <trace> [--exe <path>] [-- <lldb args>...]   debug a recording in lldb, with reverse commands
  debug <trace> [--json] [--script '<cmds>']  run debugger commands over a recording; with no script it runs
                                              `summary`: how the run ended, each panic with its backtrace
  gdbserver <trace> [--port <n>] [--exe <path>]   serve a recording to a gdb-remote client

  retrace --version    print the version

  RETRACE_DIR=<dir>  record without -o into <dir>/<basename>.retrace

Programs retrace cannot run yet are listed at {}", retrace_arch::WALLS_URL)
}

/// One of retrace's own lines on its stderr. A reader that went away is not retrace's failure, so a
/// failed write is dropped rather than panicking, as `eprintln!` would (Review Focus 1).
fn say(line: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{line}");
}

/// A line on retrace's stdout (`--help`, `--version`), dropped when the reader has gone away.
fn say_out(line: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stdout(), "{line}");
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
        panic!("RETRACE_DEBUG_PANIC is set: a deliberate internal error, the bug-banner control");
    }
    match a.get(1).map(String::as_str) {
        Some("record") => record(a),
        Some("record-dyn") => bad_input("record-dyn is now record: retrace record [-o <trace>] <program> [<args>...]"),
        Some("--version" | "-V") => { say_out(&format!("retrace {} ({})", env!("CARGO_PKG_VERSION"), retrace_arch::host::supported_text())); 0 }
        Some("--help" | "-h" | "help") => { say_out(&help()); 0 }
        Some("replay") => replay(a),
        Some("debug") => debug_cmd(a),
        Some("gdbserver") => gdbserver_cmd(a),
        Some("lldb") => lldbcmd::run(a),
        // `retrace` alone: the same text as `--help`, and still bad input.
        None => { say(&help()); 2 }
        _ => bad_input(USAGE),
    }
}

/// Spec M56 section 3g, final review: `retrace replay` and `retrace lldb` take a recording. A file whose
/// header is not the trace magic is one line of bad input, before any VM work; an arm64 Mach-O is told
/// to be recorded first. A trace of another format version passes here, so `open_checked`'s own
/// message about it still reaches the user.
fn check_recording(path: &str) -> Result<(), String> {
    use std::io::Read;
    let mut head = Vec::with_capacity(4);
    std::fs::File::open(path).and_then(|f| f.take(4).read_to_end(&mut head))
        .map_err(|e| format!("cannot read {path}: {e}"))?;
    if head == retrace_trace::TRACE_MAGIC { return Ok(()); }
    if retrace_trace::Reader::format_skew(path).ok().flatten().is_some() { return Ok(()); }
    let macho = [[0xcf, 0xfa, 0xed, 0xfe], [0xca, 0xfe, 0xba, 0xbe], [0xca, 0xfe, 0xba, 0xbf]].iter().any(|m| head == m[..]);
    let hint = if macho && std::fs::read(path).is_ok_and(|b| retrace_guest::try_parse_macho(&b).is_ok()) {
        format!(" - record it first: retrace record {path}")
    } else { String::new() };
    Err(format!("{path} is not a retrace recording{hint}"))
}

/// Bad input to retrace itself (R2): one `retrace:` line, and exit 2.
fn bad_input(line: &str) -> i32 {
    say(&format!("retrace: {line}"));
    2
}

/// M57 §3e: the macOS the startup check reads. In a debug build only, `RETRACE_DEBUG_MACOS=<product>,<build>` stands in
/// for it, so `osgate_e2e` can show the refusal for a major this Mac does not run. A release build has no such variable
/// (RETRACE_DEBUG_PANIC's precedent, `run`).
fn gate_os() -> retrace_arch::HostOs {
    #[cfg(debug_assertions)]
    if let Some(os) = std::env::var("RETRACE_DEBUG_MACOS").ok().as_deref().and_then(retrace_arch::host::parse_os_override) {
        return os;
    }
    retrace_arch::host_os()
}

/// M57 §3e (R4): a recording made on another macOS build is bad input, before any VM work.
fn host_pin(trace: &str) -> Result<(), i32> {
    match retrace_core::host_refusal(Path::new(trace)) { Some(why) => Err(bad_input(&why)), None => Ok(()) }
}

/// M57: what every command that creates a VM checks first, in order: the guest address width this Mac allows (§3c),
/// then the entitlement (M50), after the OS gate (§3e). `Err` is the exit code, its line printed.
fn vm_preflight() -> Result<(), i32> {
    if let Some(u) = retrace_arch::host::os_refusal(&gate_os()) { say(&u.line()); return Err(4); }
    if let Err(why) = retrace_core::ipa_check() {
        say(&retrace_arch::Unsupported::new(retrace_arch::WallCategory::MacosVersion, why).line());
        return Err(4);
    }
    if let Err(why) = retrace_core::hypervisor_check() { return Err(no_hypervisor(&why)); }
    Ok(())
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
        > \"${{TMPDIR:-/tmp}}/retrace.entitlements\" && codesign --sign - --force --entitlements \
        \"${{TMPDIR:-/tmp}}/retrace.entitlements\" {me}")
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

/// `retrace record --help` (or `-h`) in the program position is a request for help. A program with that
/// name is still reachable as `./--help`, or after `--`.
fn record_wants_help(a: &[String]) -> bool {
    let mut i = 2;
    while a.get(i).map(String::as_str) == Some("-o") { i += 2; }
    matches!(a.get(i).map(String::as_str), Some("--help" | "-h"))
}

/// The default trace name: the program's basename plus `.retrace`, in the working directory.
fn default_trace_name(program: &str) -> String {
    let base = Path::new(program).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| program.into());
    format!("{base}.retrace")
}

/// Where a recording goes: `-o` wins; else `<dir>/<basename>.retrace` when `dir` is set and non-empty
/// (`RETRACE_DIR`); else the default name in the working directory.
fn trace_path(out: Option<&str>, program: &str, dir: Option<&std::ffi::OsStr>) -> PathBuf {
    if let Some(o) = out { return PathBuf::from(o); }
    match dir.filter(|d| !d.is_empty()) {
        Some(d) => Path::new(d).join(default_trace_name(program)),
        None => PathBuf::from(default_trace_name(program)),
    }
}

/// `record [-o <trace>] <program> [<args>...]`. LC_LOAD_DYLINKER present records through the named
/// dylinker (real dyld); absent, the freestanding static path.
fn record(a: &[String]) -> i32 {
    if record_wants_help(a) { say_out(&help()); return 0; }
    let r = match parse_record_args(a) { Ok(r) => r, Err(u) => return bad_input(&u) };
    let guest = &r.program;
    let bytes = match std::fs::read(guest) {
        Ok(b) => b,
        Err(e) => {
            let hint = if guest.contains('/') { String::new() }
                else { format!(" - give a path, such as ./{guest} or /bin/{guest}") };
            return bad_input(&format!("cannot read {guest}: {e}{hint}"));
        }
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
    let dir = std::env::var_os("RETRACE_DIR");
    let out_path = trace_path(r.out.as_deref(), guest, dir.as_deref());
    if !named && dir.as_deref().is_some_and(|d| !d.is_empty()) {
        if let Some(parent) = out_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return bad_input(&format!("cannot create {}: {e}", parent.display()));
            }
        }
    }
    let out = out_path.to_string_lossy().into_owned();
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
    if let Err(code) = vm_preflight() { return code; }
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
    if let Err(line) = check_recording(trace) { return bad_input(&line); }
    if let Err(code) = vm_preflight() { return code; }
    if let Err(code) = host_pin(trace) { return code; }
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

/// `debug <trace> --script '<cmd>; <cmd>; …'`. A missing trace, a `--script` with no value, or an
/// argument other than `--json` and `--script` is a usage error (exit 2), validated before any file
/// I/O or VM work.
fn debug_cmd(a: &[String]) -> i32 {
    const DEBUG_USAGE: &str = "usage: retrace debug <trace> [--json] [--script '<cmds>']";
    let Some(trace) = a.get(2).filter(|t| *t != "--json" && *t != "--script") else { return bad_input(DEBUG_USAGE) };
    // Final review: an argument `debug` does not know (`--jsno`) is bad input, never silently a text-mode summary.
    let unknown = |arg: &str| bad_input(&format!("unknown argument to debug: {arg} ({DEBUG_USAGE})"));
    if trace.starts_with("--") { return unknown(trace); }
    let (mut json, mut script) = (false, "summary");
    let mut rest = a.iter().skip(3);
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--script" => match rest.next() {
                Some(s) => script = s.as_str(),
                None => return bad_input(DEBUG_USAGE),
            },
            other => return unknown(other),
        }
    }
    if let Err(e) = check_recording(trace) { return bad_input(&e); }
    if let Err(code) = vm_preflight() { return code; }
    if let Err(code) = host_pin(trace) { return code; }
    let mode = if json { debug::Mode::Json } else { debug::Mode::Text };
    match debug::run_script_mode(Path::new(trace), script, mode, &mut std::io::stdout()) {
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
    if let Err(code) = vm_preflight() { return code; }
    if let Err(code) = host_pin(trace) { return code; }
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

    #[test]
    fn trace_path_prefers_o_then_dir_then_default() {
        use std::ffi::OsStr;
        assert_eq!(trace_path(Some("t.bin"), "./p", Some(OsStr::new("d"))), PathBuf::from("t.bin"));
        assert_eq!(trace_path(None, "./x/p", Some(OsStr::new("d"))), PathBuf::from("d/p.retrace"));
        assert_eq!(trace_path(None, "./p", None), PathBuf::from("p.retrace"));
        assert_eq!(trace_path(None, "./p", Some(OsStr::new(""))), PathBuf::from("p.retrace"));
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
        assert!(line.contains("&& codesign") && !line.contains("; codesign"), "the second command runs only if the first did: {line}");
        assert!(line.contains("\"${TMPDIR:-/tmp}/retrace.entitlements\""), "{line}");
        assert!(!line.contains("> /tmp/"), "{line}");
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
