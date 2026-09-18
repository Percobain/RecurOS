//! Canonical byte form of a claim's content, and its content address (CID).
//!
//! This is the one place in ContextOS where a subtle bug corrupts data instead
//! of crashing: if two machines (or the Rust binary and the TypeScript Worker)
//! disagree on a single byte, dedup silently breaks and Merkle roots never
//! match. So every rule here is explicit, and `docs/canonical.md` is the
//! normative description other implementations must follow.
//!
//! Pipeline (spec §5.3):
//! 1. take `{kind, text, why, refs, entities}`
//! 2. NFC-normalise every string
//! 3. line endings → LF (`\r\n` and lone `\r`)
//! 4. strip trailing whitespace from every line
//! 5. strip trailing newlines from each string; the *document* ends in exactly one `\n`
//! 6. serialise with RFC 8785 (JCS)
//! 7. UTF-8 encode
//!
//! `cid = "b3:" + lowercase_hex(blake3(canonical_bytes))`

use serde_json::{Map, Value};
use unicode_normalization::UnicodeNormalization;

use crate::claim::Kind;
use crate::jcs;

/// Characters with the Unicode `White_Space` property, spelled out rather than
/// delegated to `char::is_whitespace` so the set can never drift with a
/// toolchain's Unicode version, and so other implementations can copy it
/// verbatim. Note: JavaScript's `trimEnd` uses a *different* set (it adds
/// U+FEFF and omits U+0085) — the Worker must use this list, not `trimEnd`.
pub const WHITESPACE: &[char] = &[
    '\u{0009}', '\u{000A}', '\u{000B}', '\u{000C}', '\u{000D}', '\u{0020}', '\u{0085}', '\u{00A0}',
    '\u{1680}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}',
    '\u{2007}', '\u{2008}', '\u{2009}', '\u{200A}', '\u{2028}', '\u{2029}', '\u{202F}', '\u{205F}',
    '\u{3000}',
];

/// The content-bearing fields of a claim, already normalised. Constructing one
/// through [`CanonicalContent::new`] is the only way to get normalised fields,
/// so a `Claim` built from it stores exactly what was hashed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalContent {
    pub kind: Kind,
    pub text: String,
    pub why: Option<String>,
    pub refs: Vec<String>,
    pub entities: Vec<String>,
}

impl CanonicalContent {
    /// Normalise raw user input. Empty `why` becomes `None`; empty refs and
    /// entities are dropped; refs and entities are deduplicated and sorted by
    /// code point (equivalently, by UTF-8 bytes — *not* JS's UTF-16 sort).
    pub fn new(
        kind: Kind,
        text: &str,
        why: Option<&str>,
        refs: &[impl AsRef<str>],
        entities: &[impl AsRef<str>],
    ) -> Self {
        let why = why.map(normalize_text).filter(|w| !w.is_empty());

        let mut refs: Vec<String> = refs
            .iter()
            .map(|r| normalize_token(r.as_ref()))
            .filter(|r| !r.is_empty())
            .collect();
        refs.sort();
        refs.dedup();

        // Lowercase first, then normalise: case mapping can emit non-NFC
        // sequences, so NFC must be the last transformation. `to_lowercase` is
        // Unicode default case mapping, matching JS `toLowerCase` (no locale).
        let mut entities: Vec<String> = entities
            .iter()
            .map(|e| normalize_token(&e.as_ref().to_lowercase()))
            .filter(|e| !e.is_empty())
            .collect();
        entities.sort();
        entities.dedup();

        CanonicalContent {
            kind,
            text: normalize_text(text),
            why,
            refs,
            entities,
        }
    }

    /// The exact bytes the CID is computed over.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut obj = Map::new();
        obj.insert("kind".into(), Value::String(self.kind.as_str().into()));
        obj.insert("text".into(), Value::String(self.text.clone()));
        obj.insert(
            "why".into(),
            self.why.clone().map(Value::String).unwrap_or(Value::Null),
        );
        obj.insert("refs".into(), strings(&self.refs));
        obj.insert("entities".into(), strings(&self.entities));

        // The canonical object contains only strings, arrays and null, so JCS
        // cannot fail here; `expect` documents an internal invariant, not input.
        let mut s = jcs::to_string(&Value::Object(obj)).expect("canonical form has no numbers");
        s.push('\n');
        s.into_bytes()
    }

    pub fn cid(&self) -> String {
        format!("b3:{}", blake3::hash(&self.to_bytes()).to_hex())
    }
}

fn strings(v: &[String]) -> Value {
    Value::Array(v.iter().cloned().map(Value::String).collect())
}

/// Normalise free text (`text`, `why`): NFC, LF line endings, no trailing
/// whitespace on any line, no trailing newlines. Leading whitespace is kept —
/// indentation can be meaningful (code snippets).
pub fn normalize_text(s: &str) -> String {
    let nfc: String = s.nfc().collect();
    let lf = nfc.replace("\r\n", "\n").replace('\r', "\n");
    let stripped: Vec<&str> = lf
        .split('\n')
        .map(|line| line.trim_end_matches(WHITESPACE))
        .collect();
    stripped.join("\n").trim_end_matches('\n').to_owned()
}

/// Normalise an identifier-like string (`refs`, `entities`): as
/// [`normalize_text`], plus leading whitespace is also removed since it is
/// never meaningful in a path, URL or tag.
pub fn normalize_token(s: &str) -> String {
    normalize_text(s).trim_start_matches(WHITESPACE).to_owned()
}

/// Canonical bytes for raw (un-normalised) input. Convenience wrapper.
pub fn canonical_bytes(
    kind: Kind,
    text: &str,
    why: Option<&str>,
    refs: &[impl AsRef<str>],
    entities: &[impl AsRef<str>],
) -> Vec<u8> {
    CanonicalContent::new(kind, text, why, refs, entities).to_bytes()
}

/// CID for raw (un-normalised) input. Convenience wrapper.
pub fn cid(
    kind: Kind,
    text: &str,
    why: Option<&str>,
    refs: &[impl AsRef<str>],
    entities: &[impl AsRef<str>],
) -> String {
    CanonicalContent::new(kind, text, why, refs, entities).cid()
}
