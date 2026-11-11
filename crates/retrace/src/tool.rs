//! M58 final review: an external tool (`dsymutil`, `atos`, `c++filt`) run with a deadline. The spec's rule for these
//! tools is that they degrade and never fail a command, and a tool that hangs would fail it by never returning, so
//! each run is bounded: on the deadline the child is killed and the caller degrades with `<tool> timed out after N s`.
//!
//! clippy denies `Instant::now` and `SystemTime::now` in this workspace (determinism), so the deadline is a count of
//! short sleeps between `try_wait` polls, not a clock read. It can only overshoot, by the time the polls themselves
//! take. stdout and stderr are drained by their own threads, so a child that writes more than a pipe holds is never
//! blocked by its own output. (`std::thread::Thread` is denied for the recorder; this is presentation code, and the
//! spawn builder is not the denied type, as in `demangle.rs`.)
use std::io::{Read, Write};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

/// How long `dsymutil` may take, in seconds.
pub const DSYMUTIL_SECS: u64 = 120;
/// How long one `atos` call may take, in seconds.
pub const ATOS_SECS: u64 = 30;
/// How long one `c++filt` batch may take, in seconds.
pub const CXXFILT_SECS: u64 = 30;

/// The poll interval.
const TICK: Duration = Duration::from_millis(10);

/// Why a bounded run gave no output.
#[derive(Debug, PartialEq, Eq)]
pub enum ToolError {
    /// The tool could not be started: missing, or not executable. The text is the OS's.
    Spawn(String),
    /// The deadline passed and the child was killed.
    TimedOut,
}

/// Run `cmd` with `input` on its stdin (or `/dev/null`), killed after `secs` seconds.
pub fn run(cmd: &mut Command, input: Option<String>, secs: u64) -> Result<Output, ToolError> {
    run_ticks(cmd, input, secs.saturating_mul(1000 / TICK.as_millis() as u64))
}

/// `run` with the deadline given as a count of `TICK`s.
fn run_ticks(cmd: &mut Command, input: Option<String>, ticks: u64) -> Result<Output, ToolError> {
    cmd.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| ToolError::Spawn(e.to_string()))?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        // Fed from a thread: a large batch must not deadlock against the pipe's capacity.
        std::thread::spawn(move || { let _ = stdin.write_all(text.as_bytes()); });
    }
    let drain = |p: Option<Box<dyn Read + Send>>| std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut p) = p { let _ = p.read_to_end(&mut buf); }
        buf
    });
    let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    for _ in 0..=ticks {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ok(Output { status, stdout: out.join().unwrap_or_default(), stderr: err.join().unwrap_or_default() });
            }
            Ok(None) => std::thread::sleep(TICK),
            Err(e) => { let _ = child.kill(); let _ = child.wait(); return Err(ToolError::Spawn(e.to_string())); }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    // The drain threads are left to end with the pipes: a grandchild holding one open must not hang us here.
    Err(ToolError::TimedOut)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_that_finishes_gives_its_output() {
        let o = run(Command::new("/bin/echo").arg("hi"), None, 5).unwrap();
        assert!(o.status.success());
        assert_eq!(o.stdout, b"hi\n");
    }

    #[test]
    fn input_reaches_stdin() {
        let o = run(&mut Command::new("/bin/cat"), Some("a\nb\n".into()), 5).unwrap();
        assert_eq!(o.stdout, b"a\nb\n");
    }

    #[test]
    fn a_tool_past_its_deadline_is_killed() {
        // 5 ticks of 10 ms against a 30 s sleep: the run must come back long before the sleep would end.
        assert_eq!(run_ticks(Command::new("/bin/sleep").arg("30"), None, 5), Err(ToolError::TimedOut));
    }

    #[test]
    fn a_missing_tool_is_a_spawn_error() {
        assert!(matches!(run(&mut Command::new("/nonexistent/tool"), None, 5), Err(ToolError::Spawn(_))));
    }
}
