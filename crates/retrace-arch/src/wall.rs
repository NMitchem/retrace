//! M50 (spec §3b, §3c, C1, C7): a wall, and the lines a user sees.
//!
//! A **wall** is something a program does that retrace does not model yet. It ends the recording
//! with one line that names the thing and links the docs that explain it, and exit 4. Anything else
//! that stops retrace is a **bug** in retrace, which the CLI reports with a banner and exit 70.
//!
//! The class is decided at the site that refuses, never by reading message text afterwards (R6).
//! A box method returns `Err(wall(category, what))`; a deep shared helper panics with
//! `refuse!(Category, "…")`, whose payload is the same string. Both start with [`WALL_MARKER`],
//! which nothing else writes (`crates/retrace-box/tests/refuse_audit.rs` pins that), and the record
//! and replay boundaries read it back with [`Unsupported::from_marked`] and
//! [`Unsupported::from_panic`].
use std::any::Any;

/// Where every wall line points. A URL, not a path: a Homebrew install has no repository (M52).
pub const WALLS_URL: &str = "https://github.com/NMitchem/retrace/blob/main/docs/walls.md";

/// The start of a marked string, `[retrace-wall:<anchor>] <what>`. Readable on purpose: a marked
/// divergence detail can reach `debug`'s error line, and it should still say what happened.
pub const WALL_MARKER: &str = "[retrace-wall:";

/// The seven kinds of wall (spec §3b). Each names the `docs/walls.md` section its line links to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallCategory { Syscalls, MachMessages, ProcessCreation, Signals, Threads, Memory, Exceptions }

impl WallCategory {
    pub const ALL: [WallCategory; 7] = [WallCategory::Syscalls, WallCategory::MachMessages,
        WallCategory::ProcessCreation, WallCategory::Signals, WallCategory::Threads, WallCategory::Memory,
        WallCategory::Exceptions];

    /// The `docs/walls.md` anchor this category's line links to.
    pub fn anchor(self) -> &'static str {
        match self {
            WallCategory::Syscalls => "syscalls",
            WallCategory::MachMessages => "mach-messages",
            WallCategory::ProcessCreation => "process-creation",
            WallCategory::Signals => "signals",
            WallCategory::Threads => "threads",
            WallCategory::Memory => "memory",
            WallCategory::Exceptions => "exceptions",
        }
    }

    pub fn from_anchor(anchor: &str) -> Option<WallCategory> {
        WallCategory::ALL.into_iter().find(|c| c.anchor() == anchor)
    }
}

/// One wall: what the program did that retrace does not model, and which kind of thing it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported { pub category: WallCategory, pub what: String }

impl Unsupported {
    pub fn new(category: WallCategory, what: impl Into<String>) -> Unsupported {
        Unsupported { category, what: what.into() }
    }

    /// The string a wall travels as, inside an `Err` or as a `refuse!` payload (C1).
    pub fn marked(&self) -> String { format!("{WALL_MARKER}{}] {}", self.category.anchor(), self.what) }

    /// The wall a marked string carries, or None. A string the marker does not START is never a
    /// wall: a wrapper that adds context in front (`format!("…: {m}")`) or an `unwrap` of a marked
    /// `Err` has made it a bug's text at that boundary.
    pub fn from_marked(s: &str) -> Option<Unsupported> {
        let (anchor, what) = s.strip_prefix(WALL_MARKER)?.split_once("] ")?;
        Some(Unsupported::new(WallCategory::from_anchor(anchor)?, what))
    }

    /// The wall a panic carries: a `refuse!`, or a `panic!("{m}")` of a marked `m`.
    pub fn from_panic(payload: &(dyn Any + Send)) -> Option<Unsupported> {
        let s = payload.downcast_ref::<String>().map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())?;
        Unsupported::from_marked(s)
    }

    /// The line a user sees (spec §3b): `retrace: unsupported: <what> - see <WALLS_URL>#<anchor>`.
    pub fn line(&self) -> String {
        format!("retrace: unsupported: {} - see {WALLS_URL}#{}", one_line(&self.what), self.category.anchor())
    }
}

/// A box method's refusal of a guest's call (C1): `return Err(wall(WallCategory::Threads, …))`.
pub fn wall(category: WallCategory, what: impl Into<String>) -> String {
    Unsupported::new(category, what).marked()
}

/// `<ctx>: <e>`, keeping a wall a wall: context added to a marked string goes inside the marker, so
/// the record loop still reads the category. Every `map_err` that adds context on a wall row's path
/// uses it.
pub fn in_context(ctx: &str, e: String) -> String {
    match Unsupported::from_marked(&e) {
        Some(u) => wall(u.category, format!("{ctx}: {}", u.what)),
        None => format!("{ctx}: {e}"),
    }
}

/// Test support: the text of a marked wall, for assertions that read what a refusal says. Panics on
/// an unmarked string, so a test that expects a wall fails if its site was never marked.
#[doc(hidden)]
pub trait Unmark { fn unmark(self) -> String; }
impl Unmark for String {
    fn unmark(self) -> String {
        Unsupported::from_marked(&self).unwrap_or_else(|| panic!("not a marked wall: {self}")).what
    }
}

/// A deep shared helper's refusal (spec §3c B4): a panic whose payload is the marked string, so
/// every `#[should_panic(expected = …)]` that names the message still matches it. Every call site
/// is on `crates/retrace-box/tests/refuse-sites.txt`.
#[macro_export]
macro_rules! refuse {
    ($category:ident, $($arg:tt)+) => {
        ::std::panic::panic_any($crate::Unsupported::new($crate::WallCategory::$category, format!($($arg)+)).marked())
    };
}

/// C2: replay's last line for a recording that has no terminal event.
pub fn incomplete_line(at: &str) -> String {
    format!("retrace: incomplete recording: it ends at {} - see {WALLS_URL}#incomplete-recordings", one_line(at))
}

/// §3b: a call retrace answers with an error the program survives (exec, fork, a message queue).
pub fn refused_line(what: &str, returned: &str) -> String {
    format!("retrace: refused: {}, returned {returned} to the program", one_line(what))
}

/// M51 §3e (R4): whether retrace prints its own diagnostic lines (`[retrace warn] …`, `[retrace]
/// forwarding …`, `[retrace] fall-throughs: N`). Off unless `RETRACE_VERBOSE` is set; `RETRACE_TRACE`
/// turns it on too. The documented `retrace: ` lines print either way.
pub fn verbose() -> bool {
    std::env::var_os("RETRACE_VERBOSE").is_some() || std::env::var_os("RETRACE_TRACE").is_some()
}

/// `s` as one ASCII line (R1): each run of whitespace becomes one space, an en or em dash becomes
/// `-`, and any other non-ASCII or control character becomes `?`, so a plain `grep` and a terminal
/// both show the line whole.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").chars()
        .map(|c| match c {
            '\u{2013}' | '\u{2014}' => '-',
            c if c.is_ascii() && !c.is_ascii_control() => c,
            _ => '?',
        })
        .collect()
}

/// A syscall or mach trap as a wall line names it: `fsync (syscall 95)`,
/// `semaphore_timedwait_trap (mach trap -38)`, or the bare number when no name is known.
pub fn syscall_display(num: u64) -> String {
    let n = num as i64;
    if n < 0 {
        match mach_trap_name(n) { Some(name) => format!("{name} (mach trap {n})"), None => format!("mach trap {n}") }
    } else {
        match crate::sysnames::syscall_name(num) {
            Some(name) => format!("{name} (syscall {num})"),
            None => format!("syscall {num}"),
        }
    }
}

/// The mach traps a wall line can name, and only measured ones: the semaphore family is read off
/// libsystem_kernel's own stubs (`is_mach_semaphore_trap`'s table); the last three are the traps the
/// `cf_walls` modes reached, named for the function each mode called (t0 M4).
fn mach_trap_name(n: i64) -> Option<&'static str> {
    Some(match n {
        -33 => "semaphore_signal_trap",
        -34 => "semaphore_signal_all_trap",
        -35 => "semaphore_signal_thread_trap",
        -36 => "semaphore_wait_trap",
        -37 => "semaphore_wait_signal_trap",
        -38 => "semaphore_timedwait_trap",
        -39 => "semaphore_timedwait_signal_trap",
        -59 => "swtch_pri",
        -62 => "clock_sleep_trap",
        -90 => "mach_wait_until",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wall_line_is_one_ascii_line_naming_the_thing_and_its_anchor() {
        let u = Unsupported::new(WallCategory::Syscalls, syscall_display(230));
        assert_eq!(u.line(), "retrace: unsupported: poll (syscall 230) - see \
            https://github.com/NMitchem/retrace/blob/main/docs/walls.md#syscalls");
    }

    #[test]
    fn what_is_flattened_to_one_ascii_line() {
        let u = Unsupported::new(WallCategory::Threads,
            "M14: DEADLOCK \u{2014} no runnable\n   thread.\tStates: [\u{e9}\u{7}]");
        assert_eq!(u.line(), format!("retrace: unsupported: M14: DEADLOCK - no runnable thread. \
            States: [??] - see {WALLS_URL}#threads"));
        let l = incomplete_line("landmark 7\n(pc 0x10)");
        assert_eq!(l, format!("retrace: incomplete recording: it ends at landmark 7 (pc 0x10) - see \
            {WALLS_URL}#incomplete-recordings"));
        assert_eq!(refused_line("fork (syscall 2)", "EAGAIN"),
            "retrace: refused: fork (syscall 2), returned EAGAIN to the program");
    }

    #[test]
    fn a_marked_string_round_trips_and_an_unmarked_one_is_never_a_wall() {
        let m = wall(WallCategory::MachMessages, "mach_msg2 msgh_id 205");
        assert_eq!(Unsupported::from_marked(&m),
            Some(Unsupported::new(WallCategory::MachMessages, "mach_msg2 msgh_id 205")));
        assert_eq!(Unsupported::from_marked("M48: psynch psynch_mutexwait (301) is not modelled"), None);
        assert_eq!(Unsupported::from_marked(&format!("refused on replay: {m}")), None,
            "the marker must start the string: context added in front makes it a bug's text");
        assert_eq!(Unsupported::from_marked("[retrace-wall:nonsense] x"), None, "an unknown anchor is no wall");
        assert_eq!(Unsupported::from_marked(&in_context("decode", m.clone())),
            Some(Unsupported::new(WallCategory::MachMessages, "decode: mach_msg2 msgh_id 205")));
        assert_eq!(in_context("decode", "short read".into()), "decode: short read");
    }

    #[test]
    fn a_refuse_panic_is_a_wall_and_any_other_panic_is_not() {
        let sig = 0;
        let p = std::panic::catch_unwind(|| { crate::refuse!(Signals, "signal {sig} out of range") }).unwrap_err();
        assert_eq!(Unsupported::from_panic(p.as_ref()),
            Some(Unsupported::new(WallCategory::Signals, "signal 0 out of range")));
        let p = std::panic::catch_unwind(|| panic!("M14: an internal invariant")).unwrap_err();
        assert_eq!(Unsupported::from_panic(p.as_ref()), None);
        // A B1 wrapper's `panic!("{m}")` of a marked `m` is a wall too: the payload is `m` itself.
        let m = wall(WallCategory::Memory, "x");
        let p = std::panic::catch_unwind(move || panic!("{m}")).unwrap_err();
        assert_eq!(Unsupported::from_panic(p.as_ref()), Some(Unsupported::new(WallCategory::Memory, "x")));
    }

    #[test]
    fn every_category_has_a_distinct_anchor_that_parses_back_and_a_section_in_walls_md() {
        let anchors: Vec<&str> = WallCategory::ALL.iter().map(|c| c.anchor()).collect();
        assert_eq!(anchors, ["syscalls", "mach-messages", "process-creation", "signals", "threads", "memory",
                             "exceptions"]);
        for c in WallCategory::ALL { assert_eq!(WallCategory::from_anchor(c.anchor()), Some(c)); }
        // GitHub derives a heading's anchor by lowercasing it and turning spaces into hyphens.
        let doc = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/walls.md")).unwrap();
        let slugs: Vec<String> = doc.lines().filter_map(|l| l.strip_prefix("## "))
            .map(|h| h.trim().to_lowercase().replace(' ', "-")).collect();
        for a in anchors.iter().copied().chain(["incomplete-recordings"]) {
            assert!(slugs.iter().any(|s| s == a), "docs/walls.md has no `## ` heading for #{a}: {slugs:?}");
        }
    }

    #[test]
    fn syscall_display_names_bsd_numbers_from_the_sdk_and_measured_mach_traps() {
        assert_eq!(syscall_display(95), "fsync (syscall 95)");
        assert_eq!(syscall_display(375), "kevent_id (syscall 375)");
        assert_eq!(syscall_display((-38i64) as u64), "semaphore_timedwait_trap (mach trap -38)");
        assert_eq!(syscall_display((-1000i64) as u64), "mach trap -1000");
        assert_eq!(syscall_display(100_000), "syscall 100000");
    }

    #[test]
    fn the_generated_syscall_names_match_the_sdk_header() {
        let sdk = std::process::Command::new("xcrun").arg("--show-sdk-path").output().expect("xcrun");
        let header = format!("{}/usr/include/sys/syscall.h", String::from_utf8(sdk.stdout).unwrap().trim());
        let text = std::fs::read_to_string(&header).unwrap_or_else(|e| panic!("{header}: {e}"));
        let mut n = 0;
        for line in text.lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 3 || f[0] != "#define" || !f[1].starts_with("SYS_") || f[1] == "SYS_MAXSYSCALL" { continue; }
            let num: u64 = f[2].parse().unwrap_or_else(|_| panic!("{line}"));
            assert_eq!(crate::sysnames::syscall_name(num), Some(&f[1][4..]), "{line}");
            n += 1;
        }
        assert_eq!(n, 458, "the SDK's SYS_ count moved: re-run tools/gen-sysnames.sh");
    }
}
