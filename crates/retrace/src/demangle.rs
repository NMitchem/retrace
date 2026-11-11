//! M58 B2: readable names for Rust (legacy and v0) and C++ symbols. Presentation only.

use retrace_core::symbols::Symbols;

use crate::tool::{self, ToolError};

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

/// The readable form of a Mach-O symbol, or `None` when it equals the raw one (C names, Swift).
/// Rust names (v0 `_R…`, legacy `_ZN…17h<hash>E`) drop the hash; any other `_Z…` is C++ and is
/// demangled one name at a time here; the `Readable` cache batches them instead.
#[cfg_attr(not(test), allow(dead_code))] // the cache (`Readable::get`) batches C++; this is the one-name form
pub fn readable(raw: &str) -> Option<String> {
    let s = raw.strip_prefix('_').unwrap_or(raw);
    if let Some(r) = rust(s) { return Some(r); }
    if s.starts_with("_Z") {
        return cxx_batch(&[raw]).pop().flatten();
    }
    None
}

/// Rust legacy or v0, hash dropped. `s` has the Mach-O underscore already stripped.
fn rust(s: &str) -> Option<String> {
    if !(s.starts_with("_R") || s.starts_with("_ZN")) { return None; }
    let d = rustc_demangle::try_demangle(s).ok()?;
    let r = format!("{d:#}");
    (r != s).then_some(r)
}

/// One `c++filt` call for the whole batch, names on stdin one per line. Any failure: all `None`.
/// A failure says why once per process, on stderr (stdout stays pure for `--json`).
pub fn cxx_batch(raws: &[&str]) -> Vec<Option<String>> {
    static SAID: AtomicBool = AtomicBool::new(false);
    let (out, why) = cxx_batch_with(Path::new("/usr/bin/c++filt"), raws);
    notice_once(&SAID, why, &mut |m| eprintln!("{m}"));
    out
}

/// Emit `reason` through `sink` unless a notice was already emitted through `said`.
fn notice_once(said: &AtomicBool, reason: Option<String>, sink: &mut dyn FnMut(&str)) {
    if let Some(r) = reason {
        if !said.swap(true, Ordering::Relaxed) {
            sink(&format!("retrace: c++filt failed ({r}); C++ names are shown mangled"));
        }
    }
}

/// `cxx_batch` with the tool's path given; the second value is why it failed, if it did.
fn cxx_batch_with(tool: &Path, raws: &[&str]) -> (Vec<Option<String>>, Option<String>) {
    if raws.is_empty() { return (Vec::new(), None); }
    let fail = |why: String| (vec![None; raws.len()], Some(why));
    let input: String = raws.iter().map(|r| format!("{r}\n")).collect();
    // Bounded (M58 final review): a c++filt that hangs degrades like a missing one.
    let out = match tool::run(&mut Command::new(tool), Some(input), tool::CXXFILT_SECS) {
        Ok(o) => o,
        Err(ToolError::Spawn(e)) => return fail(format!("{}: {e}", tool.display())),
        Err(ToolError::TimedOut) => return fail(format!("c++filt timed out after {} s", tool::CXXFILT_SECS)),
    };
    if !out.status.success() { return fail(format!("exit status {}", out.status)); }
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() != raws.len() { return fail("its output did not match its input".into()); }
    (raws.iter().zip(lines).map(|(r, l)| (l != *r && !l.is_empty()).then(|| l.to_string())).collect(), None)
}

/// Drop a C++ parameter list: `ns::f(int)` becomes `ns::f`, `ns::g(int) const` too.
fn strip_params(r: &str) -> &str {
    let t = r.strip_suffix(" const").unwrap_or(r);
    if !t.ends_with(')') { return r; }
    let mut depth = 0usize;
    for (i, c) in t.char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' => { depth -= 1; if depth == 0 { return &t[..i]; } }
            _ => {}
        }
    }
    r
}

/// A per-epoch lazy cache of readable names. Interior mutability: this is single-threaded
/// presentation code and `Exec`'s `&self` renderers need it.
pub struct Readable {
    cache: RefCell<HashMap<String, Option<String>>>,
    /// Raw C++ names of every image, not yet sent to `c++filt`; drained by one batch at first need.
    pending: RefCell<Vec<String>>,
}

impl Readable {
    pub fn new(syms: &Symbols) -> Readable {
        let pending = syms.iter_all().map(|(_, n)| n)
            .filter(|n| n.starts_with("__Z") && rust(&n[1..]).is_none())
            .map(str::to_string).collect();
        Readable { cache: RefCell::new(HashMap::new()), pending: RefCell::new(pending) }
    }

    fn flush(&self) {
        let mut pending = self.pending.borrow_mut();
        if pending.is_empty() { return; }
        pending.sort();
        pending.dedup();
        let refs: Vec<&str> = pending.iter().map(String::as_str).collect();
        let out = cxx_batch(&refs);
        let mut cache = self.cache.borrow_mut();
        for (raw, r) in pending.iter().zip(out) { cache.insert(raw.clone(), r); }
        pending.clear();
    }

    fn get(&self, raw: &str) -> Option<String> {
        if let Some(r) = self.cache.borrow().get(raw) { return r.clone(); }
        let s = raw.strip_prefix('_').unwrap_or(raw);
        let r = if let Some(r) = rust(s) { Some(r) }
            else if s.starts_with("_Z") {
                self.flush();
                let hit = self.cache.borrow().get(raw).cloned();
                match hit { Some(r) => r, None => cxx_batch(&[raw]).pop().flatten() }
            } else { None };
        self.cache.borrow_mut().insert(raw.to_string(), r.clone());
        r
    }

    /// The readable name, or the raw one when there is none.
    pub fn name(&self, raw: &str) -> String { self.get(raw).unwrap_or_else(|| raw.to_string()) }

    /// Addresses for a user's name, in order: (1) exact raw, (2) exact readable, (3) readable with a
    /// C++ parameter list stripped, (4) trailing `::` path segments. Each step only if the previous
    /// found nothing. Steps 2 to 4 search the executable only.
    pub fn lookup(&self, syms: &Symbols, query: &str) -> Vec<u64> {
        let hits = syms.addrs_of(query);
        if !hits.is_empty() { return hits; }
        let Some(exe) = syms.exe() else { return Vec::new() };
        let named: Vec<(u64, String)> = exe.iter()
            .filter_map(|(a, n)| self.get(n).map(|r| (a, r))).collect();
        let suffix = format!("::{query}");
        let steps: [&dyn Fn(&str) -> bool; 3] = [
            &|r| r == query,
            &|r| strip_params(r) == query,
            &|r| { let b = strip_params(r); b.ends_with(&suffix) },
        ];
        for step in steps {
            let mut out: Vec<u64> = named.iter().filter(|(_, r)| step(r)).map(|(a, _)| *a).collect();
            out.sort_unstable();
            out.dedup();
            if !out.is_empty() { return out; }
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn rust_v0() {
        assert_eq!(readable("__RNvCsfLfy6EI15iL_7___rustc10rust_panic").as_deref(), Some("__rustc::rust_panic"));
    }
    #[test] fn rust_legacy() {
        assert_eq!(readable("__ZN8rt_probe5tests16fails_on_purpose17h64099bb73bd71502E").as_deref(),
                   Some("rt_probe::tests::fails_on_purpose"));
    }
    #[test] fn cxx() {
        assert_eq!(readable("__ZNSt3__16vectorIiNS_9allocatorIiEEE9push_backERKi").as_deref(),
                   Some("std::__1::vector<int, std::__1::allocator<int>>::push_back(int const&)"));
    }
    #[test] fn c_and_swift_untouched() {
        assert_eq!(readable("_main"), None);
        assert_eq!(readable("_$s4main3FooV"), None);
    }
    #[test] fn batch_is_aligned() {
        let r = cxx_batch(&["__ZNSt3__16vectorIiNS_9allocatorIiEEE9push_backERKi", "_main"]);
        assert_eq!(r.len(), 2);
        assert!(r[0].is_some());
        assert_eq!(r[1], None);
    }

    #[test] fn a_missing_tool_degrades_and_says_why_once() {
        let missing = Path::new("/nonexistent/c++filt");
        let said = AtomicBool::new(false);
        let mut lines: Vec<String> = Vec::new();
        for _ in 0..2 {
            let (out, why) = cxx_batch_with(missing, &["__ZN2ns1fEi", "_main"]);
            assert_eq!(out, vec![None, None]);
            assert!(why.as_deref().is_some_and(|w| w.contains("/nonexistent/c++filt")), "{why:?}");
            notice_once(&said, why, &mut |m| lines.push(m.to_string()));
        }
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("retrace: c++filt failed (") && lines[0].ends_with("C++ names are shown mangled"));
        // A working tool reports no reason.
        assert_eq!(cxx_batch_with(Path::new("/usr/bin/c++filt"), &["_main"]).1, None);
    }

    fn syms() -> Symbols {
        Symbols::from_names(&[
            (0x100, "_main"),
            (0x200, "__RNvCsfLfy6EI15iL_7___rustc10rust_panic"),
            (0x300, "__ZN6ledger5tests1f17h0123456789abcdefE"),
            (0x400, "__ZN5other5tests1f17h0123456789abcdefE"),
            (0x500, "__ZN2ns1fEi"),
            (0x600, "__ZN6ledger5tests4solo17h0123456789abcdefE"),
        ])
    }
    #[test] fn step1_raw_exact() {
        let s = syms();
        assert_eq!(Readable::new(&s).lookup(&s, "_main"), vec![0x100]);
        assert_eq!(Readable::new(&s).lookup(&s, "__ZN2ns1fEi"), vec![0x500]);
    }
    #[test] fn step2_readable_exact_without_hash() {
        let s = syms();
        assert_eq!(Readable::new(&s).lookup(&s, "ledger::tests::solo"), vec![0x600]);
    }
    #[test] fn step3_params_stripped() {
        let s = syms();
        assert_eq!(Readable::new(&s).lookup(&s, "ns::f"), vec![0x500]);
    }
    #[test] fn step4_suffix() {
        let s = syms();
        assert_eq!(Readable::new(&s).lookup(&s, "rust_panic"), vec![0x200]);
        assert_eq!(Readable::new(&s).lookup(&s, "tests::solo"), vec![0x600]);
    }
    #[test] fn step4_ambiguous_suffix_reports_all() {
        let s = syms();
        assert_eq!(Readable::new(&s).lookup(&s, "tests::f"), vec![0x300, 0x400]);
    }
    #[test] fn earlier_step_wins() {
        let s = Symbols::from_names(&[(0x1, "__ZN1a1fE"), (0x2, "__ZN3pkg1a1fE")]);
        assert_eq!(Readable::new(&s).lookup(&s, "a::f"), vec![0x1]); // step 2 beats step 4
    }
    #[test] fn unknown_is_empty() {
        let s = syms();
        assert!(Readable::new(&s).lookup(&s, "nope").is_empty());
    }
}
