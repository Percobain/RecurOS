//! Golden canonicalisation and CID tests (spec §5.3).
//!
//! These are the most important tests in the repo. Every expected value below
//! was produced by the independent Python implementation in
//! `tests/golden/reference.py`, not by this crate, and is asserted as a literal.
//! If one of these fails, do NOT update the literal to match: a changed CID
//! means every existing claim's address changed.

use ctx_core::{Kind, canonical_bytes, cid};

const NONE: &[&str] = &[];

fn assert_golden(
    kind: Kind,
    text: &str,
    why: Option<&str>,
    refs: &[&str],
    entities: &[&str],
    canonical: &str,
    expected_cid: &str,
) {
    let bytes = canonical_bytes(kind, text, why, refs, entities);
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        canonical,
        "canonical bytes"
    );
    assert_eq!(cid(kind, text, why, refs, entities), expected_cid, "cid");
}

#[test]
fn golden_minimal() {
    assert_golden(
        Kind::Fact,
        "hello",
        None,
        NONE,
        NONE,
        "{\"entities\":[],\"kind\":\"fact\",\"refs\":[],\"text\":\"hello\",\"why\":null}\n",
        "b3:ef5c7ba58908aa7aa4d1cf7f2d3011d40beeede4d51537e621ec5ed9f3350d0f",
    );
}

#[test]
fn golden_full() {
    assert_golden(
        Kind::Decision,
        "Use SQLite as the index",
        Some("Postgres needs a server; the index is a disposable cache."),
        &["docs/spec.md", "crates/ctx-store-sqlite/src/lib.rs:L10"],
        &["storage", "sqlite"],
        "{\"entities\":[\"sqlite\",\"storage\"],\"kind\":\"decision\",\
         \"refs\":[\"crates/ctx-store-sqlite/src/lib.rs:L10\",\"docs/spec.md\"],\
         \"text\":\"Use SQLite as the index\",\
         \"why\":\"Postgres needs a server; the index is a disposable cache.\"}\n",
        "b3:1e9044b2e035f6cd8b08eb9046555ee65a011bdebe6d7ab1ec4681dbc9c99daa",
    );
}

#[test]
fn golden_multiline() {
    assert_golden(
        Kind::Constraint,
        "line one\n  indented two\nline three",
        None,
        NONE,
        NONE,
        "{\"entities\":[],\"kind\":\"constraint\",\"refs\":[],\
         \"text\":\"line one\\n  indented two\\nline three\",\"why\":null}\n",
        "b3:938d7c14cf862cd0dc4f598a0fd9c593b7045e4cc99d162f4604ecc4f143f624",
    );
}

#[test]
fn golden_smart_punctuation() {
    // Smart quotes and em-dashes are content, not formatting: NFC leaves them
    // alone and so do we. They must hash identically on every platform.
    assert_golden(
        Kind::Rejected,
        "\u{201C}Covenants\u{201D} \u{2014} rejected; it\u{2019}s too slow",
        None,
        NONE,
        NONE,
        "{\"entities\":[],\"kind\":\"rejected\",\"refs\":[],\
         \"text\":\"\u{201C}Covenants\u{201D} \u{2014} rejected; it\u{2019}s too slow\",\"why\":null}\n",
        "b3:a7ee6724b5ff15af1c72ed85d9727c08acc093146566eb8e095fcc5b29236aff",
    );
}

#[test]
fn golden_nbsp_interior() {
    assert_golden(
        Kind::Fact,
        "10\u{00A0}ms budget",
        None,
        NONE,
        NONE,
        "{\"entities\":[],\"kind\":\"fact\",\"refs\":[],\"text\":\"10\u{00A0}ms budget\",\"why\":null}\n",
        "b3:336c585fb432305a64f27263b8223abf02f6fa63a23d2c7b63cc23b23e8484ab",
    );
}

#[test]
fn golden_escapes() {
    assert_golden(
        Kind::Claim,
        "tab\there \"quoted\" back\\slash \u{01} del\u{7F}",
        None,
        NONE,
        NONE,
        "{\"entities\":[],\"kind\":\"claim\",\"refs\":[],\
         \"text\":\"tab\\there \\\"quoted\\\" back\\\\slash \\u0001 del\u{7F}\",\"why\":null}\n",
        "b3:64e242c6c9e7d951cf2ac4a2fb19d3d0a02e5121c42de7512f5db30356568476",
    );
}

#[test]
fn golden_astral() {
    assert_golden(
        Kind::Question,
        "Ship the \u{1F680} before Q4?",
        None,
        NONE,
        &["\u{1F680}"],
        "{\"entities\":[\"\u{1F680}\"],\"kind\":\"question\",\"refs\":[],\
         \"text\":\"Ship the \u{1F680} before Q4?\",\"why\":null}\n",
        "b3:c22f360fa43f4c365c333f2af3880284e526336e7dfce4fa7987e76a3db6c572",
    );
}

#[test]
fn golden_cafe() {
    assert_golden(
        Kind::Fact,
        "caf\u{E9}",
        None,
        NONE,
        &["Caf\u{E9}"],
        "{\"entities\":[\"caf\u{E9}\"],\"kind\":\"fact\",\"refs\":[],\"text\":\"caf\u{E9}\",\"why\":null}\n",
        "b3:b1b9c2bc5e3b2f15cd796a704c9c0552918c6830b3058465a77575ee81bad49d",
    );
}

// ---- Equivalences: different raw input, same CID -------------------------

fn text_cid(text: &str) -> String {
    cid(Kind::Fact, text, None, NONE, NONE)
}

#[test]
fn crlf_lf_and_cr_are_equivalent() {
    let lf = text_cid("a\nb\nc");
    assert_eq!(text_cid("a\r\nb\r\nc"), lf);
    assert_eq!(text_cid("a\rb\rc"), lf);
    assert_eq!(text_cid("a\r\nb\nc\r\n"), lf);
}

#[test]
fn trailing_whitespace_and_newlines_are_insignificant() {
    let base = text_cid("a\nb");
    assert_eq!(text_cid("a  \t\nb"), base);
    assert_eq!(text_cid("a\nb\n"), base);
    assert_eq!(text_cid("a\nb\n\n\n"), base);
    assert_eq!(text_cid("a \u{00A0}\u{3000}\nb \n \n"), base);
    // Leading whitespace is kept: indentation is content.
    assert_ne!(text_cid("  a\nb"), base);
}

#[test]
fn trailing_nbsp_is_stripped_but_interior_nbsp_is_kept() {
    assert_eq!(text_cid("10 ms\u{00A0}"), text_cid("10 ms"));
    assert_ne!(text_cid("10\u{00A0}ms"), text_cid("10 ms"));
}

#[test]
fn composed_and_decomposed_are_equivalent() {
    // U+00E9 vs U+0065 U+0301
    assert_eq!(text_cid("caf\u{E9}"), text_cid("cafe\u{301}"));
    assert_eq!(
        cid(Kind::Fact, "x", None, NONE, &["CAFE\u{301}"]),
        cid(Kind::Fact, "x", None, NONE, &["caf\u{E9}"]),
    );
}

#[test]
fn smart_punctuation_is_not_folded_to_ascii() {
    // Documented decision: NFC only. Folding would silently merge claims whose
    // text differs, and rendered packs must reproduce the author's text.
    assert_ne!(text_cid("\u{201C}x\u{201D}"), text_cid("\"x\""));
    assert_ne!(text_cid("a \u{2014} b"), text_cid("a - b"));
}

#[test]
fn ordering_and_duplicates_of_refs_and_entities_are_insignificant() {
    let a = cid(
        Kind::Fact,
        "x",
        None,
        &["b.rs", "a.rs"],
        &["Liquid", "settlement"],
    );
    let b = cid(
        Kind::Fact,
        "x",
        None,
        &["a.rs", "b.rs", " a.rs"],
        &["settlement", "liquid", "LIQUID"],
    );
    assert_eq!(a, b);
}

#[test]
fn field_order_in_serialised_input_is_insignificant() {
    // Two JSON documents with the same fields in different key order (as
    // different clients might send) produce the same CID.
    #[derive(serde::Deserialize)]
    struct Input {
        text: String,
        why: Option<String>,
        refs: Vec<String>,
    }
    let one: Input = serde_json::from_str(r#"{"text":"t","why":"w","refs":["r"]}"#).unwrap();
    let two: Input = serde_json::from_str(r#"{"refs":["r"],"why":"w","text":"t"}"#).unwrap();
    let c = |i: &Input| cid(Kind::Fact, &i.text, i.why.as_deref(), &i.refs, NONE);
    assert_eq!(c(&one), c(&two));
}

#[test]
fn empty_why_is_the_same_as_no_why() {
    assert_eq!(
        cid(Kind::Fact, "x", Some(" \n"), NONE, NONE),
        cid(Kind::Fact, "x", None, NONE, NONE)
    );
}

#[test]
fn kind_is_significant() {
    assert_ne!(
        cid(Kind::Decision, "x", None, NONE, NONE),
        cid(Kind::Rejected, "x", None, NONE, NONE)
    );
}
