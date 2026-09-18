//! RFC 8785 JSON Canonicalization Scheme, restricted to the value shapes the
//! canonical claim form actually uses.
//!
//! Written by hand rather than pulled from a crate because the output is hashed:
//! we need to own every byte, and the Cloudflare Worker (TypeScript) must be able
//! to reproduce it exactly. Numbers are deliberately unsupported — JCS number
//! formatting is the hard part of the RFC and the canonical form has no numbers,
//! so refusing them removes a whole class of cross-language divergence.

use serde_json::Value;

/// Serialise `value` per RFC 8785. Returns `None` if the value contains a number.
pub fn to_string(value: &Value) -> Option<String> {
    let mut out = String::new();
    write_value(&mut out, value)?;
    Some(out)
}

fn write_value(out: &mut String, value: &Value) -> Option<()> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(_) => return None,
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(out, item)?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            // RFC 8785 §3.2.3: sort keys by their UTF-16 code units.
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(out, key);
                out.push(':');
                write_value(out, &map[key])?;
            }
            out.push('}');
        }
    }
    Some(())
}

/// String escaping per RFC 8785 §3.2.2.2 (identical to ECMAScript
/// `JSON.stringify`): only `"`, backslash and C0 controls are escaped;
/// everything else — including U+2028/U+2029 and all non-ASCII — is literal.
fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str(r#"\""#),
            '\\' => out.push_str(r"\\"),
            '\u{08}' => out.push_str(r"\b"),
            '\u{0C}' => out.push_str(r"\f"),
            '\n' => out.push_str(r"\n"),
            '\r' => out.push_str(r"\r"),
            '\t' => out.push_str(r"\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!(r"\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sorts_keys_by_utf16_code_units() {
        // U+10000 encodes as a surrogate pair starting 0xD800, which sorts
        // *before* U+FB33 in UTF-16 even though it is the larger code point.
        let v = json!({"\u{FB33}": "a", "\u{10000}": "b", "a": "c"});
        assert_eq!(
            to_string(&v).unwrap(),
            "{\"a\":\"c\",\"\u{10000}\":\"b\",\"\u{FB33}\":\"a\"}"
        );
    }

    #[test]
    fn escapes_like_json_stringify() {
        let v = Value::String("q\"b\\\u{08}\u{0C}\n\r\t\u{01}\u{1F}\u{7F}\u{2028}é".into());
        assert_eq!(
            to_string(&v).unwrap(),
            // Built from a backslash char so the expectation is unambiguous.
            "\"q_\"b___b_f_n_r_t_u0001_u001f".replace('_', &char::from(92u8).to_string())
                + "\u{7F}\u{2028}é\""
        );
    }

    #[test]
    fn rejects_numbers() {
        assert!(to_string(&json!({"a": 1})).is_none());
    }

    #[test]
    fn nested_arrays_and_null() {
        let v = json!({"b": [], "a": ["x", null, true]});
        assert_eq!(to_string(&v).unwrap(), r#"{"a":["x",null,true],"b":[]}"#);
    }
}
