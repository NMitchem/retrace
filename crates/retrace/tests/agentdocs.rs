//! M58 A3: the agent-facing docs are read as source. Every command they show exists, the skill's
//! frontmatter is valid, the paste-in snippet is the README's, and the plugin manifests agree.

use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"))
}

/// The text inside code spans and fenced blocks.
fn code_text(md: &str) -> String {
    let mut out = String::new();
    let mut fenced = false;
    for line in md.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            out.push_str(line);
            out.push('\n');
        } else {
            for (i, part) in line.split('`').enumerate() {
                if i % 2 == 1 {
                    out.push_str(part);
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// Every `"word"` that begins a match arm (`"word" =>`, or `Some("word")` when `arrow` is false).
fn quoted_arms(src: &str, arrow: bool) -> Vec<String> {
    let mut v = Vec::new();
    for line in src.lines() {
        let t = line.trim_start();
        let mut rest = if arrow {
            t
        } else {
            match t.strip_prefix("Some(") {
                Some(r) => r,
                None => continue,
            }
        };
        while let Some(r) = rest.strip_prefix('"') {
            let Some(end) = r.find('"') else { break };
            v.push(r[..end].to_string());
            rest = r[end + 1..].trim_start();
            rest = rest.strip_prefix('|').map(str::trim_start).unwrap_or(rest);
        }
    }
    v
}

fn subcommands() -> Vec<String> {
    let main = read("crates/retrace/src/main.rs");
    let start = main.find("fn run(a: &[String]) -> i32").expect("run()");
    let body = &main[start..];
    let end = body.find("\n}\n").expect("end of run()");
    quoted_arms(&body[..end], false)
}

fn debugger_verbs() -> Vec<String> {
    let dbg = read("crates/retrace/src/debug.rs");
    let start = dbg.find("fn parse_one(").expect("parse_one");
    let body = &dbg[start..];
    let end = body.find("\n}\n").expect("end of parse_one");
    quoted_arms(&body[..end], true)
}

const DOCS: [&str; 3] = ["README.md", "plugin/skills/retrace/SKILL.md", "docs/agents-snippet.md"];

#[test]
fn every_command_the_docs_show_exists() {
    let subs = subcommands();
    let verbs = debugger_verbs();
    assert!(subs.contains(&"record".to_string()) && subs.contains(&"lldb".to_string()), "{subs:?}");
    assert!(verbs.contains(&"reverse-continue".to_string()), "{verbs:?}");
    for doc in DOCS {
        let code = code_text(&read(doc));
        let mut i = 0;
        while let Some(p) = code[i..].find("retrace ") {
            let at = i + p;
            i = at + 8;
            // Only the command name itself, not a path such as `target/retrace record` or a word
            // ending in "retrace".
            if at > 0 && !code[..at].ends_with(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '(') {
                continue;
            }
            let word: String = code[i..].chars().take_while(|c| c.is_ascii_lowercase() || *c == '-').collect();
            if word.is_empty() || word.starts_with('-') {
                continue;
            }
            let after = code[i + word.len()..].chars().next();
            if after.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
                continue;
            }
            assert!(subs.contains(&word), "{doc}: `retrace {word}` is not a subcommand ({subs:?})");
        }
        let mut j = 0;
        while let Some(p) = code[j..].find("--script '") {
            let s = j + p + 10;
            let e = code[s..].find('\'').unwrap_or_else(|| panic!("{doc}: unterminated --script")) + s;
            for seg in code[s..e].split(';') {
                let Some(verb) = seg.split_whitespace().next() else { continue };
                if verb.starts_with('<') {
                    continue; // a placeholder such as '<commands>'
                }
                assert!(verbs.contains(&verb.to_string()), "{doc}: `{verb}` is not a debugger command ({verbs:?})");
            }
            j = e;
        }
    }
}

#[test]
fn skill_frontmatter_is_valid() {
    let s = read("plugin/skills/retrace/SKILL.md");
    assert!(s.starts_with("---\n"), "frontmatter must open the file");
    let end = s[4..].find("\n---").expect("frontmatter must close") + 4;
    let fm = &s[4..end];
    assert!(fm.lines().any(|l| l.trim() == "name: retrace"), "name: retrace");
    let d = fm.lines().find_map(|l| l.strip_prefix("description:")).expect("description").trim();
    let d = d.trim_matches('"');
    assert!(d.chars().count() < 1536, "description is {} chars", d.chars().count());
    let first = d.split(". ").next().unwrap();
    assert!(first.contains("Use when"), "the first sentence must carry the trigger: {first}");
    assert!(s.lines().count() < 250, "SKILL.md stays under 250 lines");
}

#[test]
fn snippet_matches_readme() {
    let snippet = read("docs/agents-snippet.md");
    let body = snippet.trim();
    assert!(!body.is_empty() && body.lines().count() <= 6, "the snippet is 3 to 6 lines");
    assert!(read("README.md").contains(body), "README.md must embed docs/agents-snippet.md verbatim");
}

#[test]
fn plugin_manifests_agree() {
    let m: serde_json::Value = serde_json::from_str(&read(".claude-plugin/marketplace.json")).expect("marketplace.json");
    let p: serde_json::Value = serde_json::from_str(&read("plugin/.claude-plugin/plugin.json")).expect("plugin.json");
    assert_eq!(m["name"], "retrace");
    assert_eq!(p["name"], "retrace");
    assert_eq!(m["plugins"][0]["name"], p["name"]);
    assert_eq!(m["plugins"][0]["source"], "./plugin");
    assert_eq!(p["author"]["name"], "Noah Mitchem");
}
