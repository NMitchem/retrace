//! M58 B1: a minimal JSON writer for `retrace debug --json`. Compact, keys in insertion order,
//! strings escaped per RFC 8259. The caller makes non-UTF-8 lossy before it gets here.

#[allow(dead_code)] // Int is used by later M58 tasks
#[derive(Debug, Clone, PartialEq)]
pub enum J {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(&'static str, J)>),
}

/// An address is a lowercase hex string in JSON.
pub fn hex(v: u64) -> J { J::Str(format!("{v:#x}")) }

fn esc(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

impl J {
    pub fn render(&self) -> String {
        let mut s = String::new();
        self.write(&mut s);
        s
    }

    fn write(&self, o: &mut String) {
        match self {
            J::Null => o.push_str("null"),
            J::Bool(b) => o.push_str(if *b { "true" } else { "false" }),
            J::Int(i) => o.push_str(&i.to_string()),
            J::UInt(u) => o.push_str(&u.to_string()),
            J::Str(s) => esc(s, o),
            J::Arr(v) => {
                o.push('[');
                for (i, x) in v.iter().enumerate() {
                    if i > 0 { o.push(','); }
                    x.write(o);
                }
                o.push(']');
            }
            J::Obj(v) => {
                o.push('{');
                for (i, (k, x)) in v.iter().enumerate() {
                    if i > 0 { o.push(','); }
                    esc(k, o);
                    o.push(':');
                    x.write(o);
                }
                o.push('}');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_quotes_backslashes_and_controls() {
        assert_eq!(J::Str("a\"b\\c\n\u{1}".into()).render(), r#""a\"b\\c\n\u0001""#);
    }

    #[test]
    fn objects_keep_insertion_order() {
        let o = J::Obj(vec![("z", J::Int(-1)), ("a", J::Arr(vec![J::Null, J::Bool(true), J::UInt(7)]))]);
        assert_eq!(o.render(), r#"{"z":-1,"a":[null,true,7]}"#);
    }

    #[test]
    fn hex_is_lowercase_0x() {
        assert_eq!(hex(0xabcdef).render(), "\"0xabcdef\"");
        assert_eq!(hex(0).render(), "\"0x0\"");
    }
}
