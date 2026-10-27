//! The M53 scrub: no file under `docs/` holds a credential. Before it the Apple sweep ran every row in
//! the caller's whole environment, so the `env` and `printenv` rows' kept output, and the census
//! tables quoting it, committed a session token and an API key verbatim (M51's evidence, M53's t0
//! baseline and its census). `tools/apple-sweep.sh` now gives each row an allowlisted environment
//! (`SWEEP_ENV`); this scan is for whatever writes evidence some other way. A site is reported by
//! file, line and variable name or prefix, never by value, because gate logs are kept.
use std::path::{Path, PathBuf};

/// A variable is a credential's when its name, upper-cased, holds one of these or ends `_KEY`.
const NAME_WORDS: &[&str] =
    &["TOKEN", "SECRET", "PASSWORD", "PASSWD", "API_KEY", "APIKEY", "ACCESS_KEY", "PRIVATE_KEY", "CREDENTIAL"];

/// The shortest value of a credential-named variable that counts.
const MIN_VALUE: usize = 8;

/// Prefixes that begin a credential wherever it stands, but only with at least PREFIX_TAIL token
/// characters after them, so prose that names a prefix is not one.
const PREFIXES: &[&str] = &["ctx7sk", "ghp_", "gho_", "ghs_", "ghu_", "github_pat_", "sk-ant-", "xoxb-", "xoxp-", "AKIA"];
const PREFIX_TAIL: usize = 16;

fn ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `(line, what)` for every credential in `text`: a credential-named variable assigned a value of at
/// least MIN_VALUE characters that is not a placeholder (`<redacted>`, `$VAR`, `${…}`, `***`), named
/// by the variable; and every PREFIXES match with its tail, named by the prefix.
fn credential_sites(text: &str) -> Vec<(usize, String)> {
    let line = |at: usize| text[..at].matches('\n').count() + 1;
    let mut out = Vec::new();
    for (eq, _) in text.match_indices('=') {
        let mut start = text[..eq].char_indices().rev().take_while(|&(_, c)| ident_char(c)).last().map_or(eq, |(i, _)| i);
        // After an escaped newline (a TSV's `\n`), the escape's letter is not part of the name.
        if start < eq && text[..start].ends_with('\\') {
            start += 1;
        }
        let name = text[start..eq].to_ascii_uppercase();
        if !(NAME_WORDS.iter().any(|w| name.contains(w)) || name.ends_with("_KEY")) {
            continue;
        }
        // A value ends where a shell word, a quoted string or an escaped newline (a TSV's `\n`) would.
        let value: String =
            text[eq + 1..].chars().take_while(|&c| !c.is_whitespace() && !"\"'|\\,;)".contains(c)).collect();
        if value.chars().count() >= MIN_VALUE && !value.starts_with(['<', '$', '*', '{', '%']) {
            out.push((line(eq), text[start..eq].to_string()));
        }
    }
    for p in PREFIXES {
        for (at, _) in text.match_indices(p) {
            let tail = text[at + p.len()..].chars().take_while(|&c| ident_char(c) || c == '-').count();
            if tail >= PREFIX_TAIL {
                out.push((line(at), format!("{p}…")));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Every file under `dir`, recursively.
fn all_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            all_files(&p, out);
        } else {
            out.push(p);
        }
    }
}

#[test]
fn no_file_under_docs_holds_a_credential() {
    // `CARGO_MANIFEST_DIR` is `…/crates/retrace`; the repo root is two up.
    let docs = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().join("docs");
    let mut files = Vec::new();
    all_files(&docs, &mut files);
    files.sort();
    // The file the scrub found the first leak in: the scan must reach the sweep's kept evidence.
    let must = "sweep-evidence/2026-10-14-m51/apple-keep/env.nat.out";
    assert!(files.iter().any(|p| p.ends_with(must)), "the scan missed {must} under {}", docs.display());
    assert!(files.len() > 4000, "scanned only {} files under {} (4585 at the scrub)", files.len(), docs.display());
    let mut hits = Vec::new();
    for p in &files {
        let text = String::from_utf8_lossy(&std::fs::read(p).unwrap()).into_owned();
        hits.extend(credential_sites(&text).into_iter().map(|(l, what)| format!("{}:{l} {what}", p.display())));
    }
    assert!(hits.is_empty(),
        "credentials under docs/, by variable or prefix: replace each value with `<redacted>` before it is \
         committed (tools/apple-sweep.sh runs rows under SWEEP_ENV alone):\n{}",
        hits.join("\n"));
}

#[test]
fn the_detector_finds_a_credential_and_passes_a_placeholder() {
    // Every token here is built at run time, so this file holds no token-shaped literal of its own.
    let val = "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6".to_string();
    // An `env` capture, and the same variable quoted mid-row in a judged TSV after an escaped newline.
    let env = format!("PATH=/usr/bin\nCLAUDE_CODE_MESSAGING_TOKEN={val}\nHOME=/Users/x\n");
    assert_eq!(credential_sites(&env), [(2, "CLAUDE_CODE_MESSAGING_TOKEN".to_string())]);
    let tsv = format!("ROW\t/usr/bin/env\tFAIL\tPATH=/usr/bin\\nCLAUDE_CODE_MESSAGING_TOKEN={val}\\nHOME=/x\n");
    assert_eq!(credential_sites(&tsv), [(1, "CLAUDE_CODE_MESSAGING_TOKEN".to_string())]);
    // A key with a known prefix is found twice over, by its name and by its prefix; lower case and a
    // name that only ends `_KEY` count too.
    let key = format!("x\nCONTEXT7_API_KEY={}{val}\n", PREFIXES[0]);
    assert_eq!(credential_sites(&key), [(2, "CONTEXT7_API_KEY".to_string()), (2, format!("{}…", PREFIXES[0]))]);
    assert_eq!(credential_sites(&format!("password={val}")), [(1, "password".to_string())]);
    assert_eq!(credential_sites(&format!("STRIPE_KEY={val}")), [(1, "STRIPE_KEY".to_string())]);
    // A prefixed token quoted with no variable at all.
    let json = format!("{{\"auth\": \"{}{val}\"}}", PREFIXES[1]);
    assert_eq!(credential_sites(&json), [(1, format!("{}…", PREFIXES[1]))]);
    // Negatives: the placeholder the scrub writes, a shell reference, a short value, a prefix named
    // in prose, and the sweep's own variables, none of which is a credential's.
    assert!(credential_sites("CLAUDE_CODE_MESSAGING_TOKEN=<redacted>").is_empty());
    assert!(credential_sites("GITHUB_TOKEN=$GITHUB_TOKEN; API_KEY=${KEY:-}").is_empty());
    assert!(credential_sites("TOKEN=short").is_empty());
    assert!(credential_sites(&format!("a `{}` key was found", PREFIXES[0])).is_empty());
    assert!(credential_sites("RETRACE_SWEEP_KEEP=/private/tmp/keep/dir XPC_SERVICE_NAME=0 KEY=value1234").is_empty());
}
