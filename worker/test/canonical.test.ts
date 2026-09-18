// Golden CID tests — the same literals as crates/ctx-core/tests/golden.rs,
// produced by the independent Python reference (tests/golden/reference.py).
// If one fails, fix the implementation; never edit the literal.

import { describe, expect, it } from "vitest";
import { canonicalContent, canonicalString, cid, docCid, normalizeText, type Kind } from "../src/canonical.js";

function golden(
  kind: Kind,
  text: string,
  why: string | null,
  refs: string[],
  entities: string[],
  canonical: string,
  expected: string,
) {
  expect(canonicalString(canonicalContent(kind, text, why, refs, entities))).toBe(canonical);
  expect(cid(kind, text, why, refs, entities)).toBe(expected);
}

describe("golden", () => {
  it("minimal", () =>
    golden(
      "fact",
      "hello",
      null,
      [],
      [],
      '{"entities":[],"kind":"fact","refs":[],"text":"hello","why":null}\n',
      "b3:ef5c7ba58908aa7aa4d1cf7f2d3011d40beeede4d51537e621ec5ed9f3350d0f",
    ));

  it("full", () =>
    golden(
      "decision",
      "Use SQLite as the index",
      "Postgres needs a server; the index is a disposable cache.",
      ["docs/spec.md", "crates/ctx-store-sqlite/src/lib.rs:L10"],
      ["storage", "sqlite"],
      '{"entities":["sqlite","storage"],"kind":"decision",' +
        '"refs":["crates/ctx-store-sqlite/src/lib.rs:L10","docs/spec.md"],' +
        '"text":"Use SQLite as the index",' +
        '"why":"Postgres needs a server; the index is a disposable cache."}\n',
      "b3:1e9044b2e035f6cd8b08eb9046555ee65a011bdebe6d7ab1ec4681dbc9c99daa",
    ));

  it("multiline", () =>
    golden(
      "constraint",
      "line one\n  indented two\nline three",
      null,
      [],
      [],
      '{"entities":[],"kind":"constraint","refs":[],"text":"line one\\n  indented two\\nline three","why":null}\n',
      "b3:938d7c14cf862cd0dc4f598a0fd9c593b7045e4cc99d162f4604ecc4f143f624",
    ));

  it("smart punctuation", () =>
    golden(
      "rejected",
      "“Covenants” — rejected; it’s too slow",
      null,
      [],
      [],
      '{"entities":[],"kind":"rejected","refs":[],"text":"“Covenants” — rejected; it’s too slow","why":null}\n',
      "b3:a7ee6724b5ff15af1c72ed85d9727c08acc093146566eb8e095fcc5b29236aff",
    ));

  it("nbsp interior", () =>
    golden(
      "fact",
      "10 ms budget",
      null,
      [],
      [],
      '{"entities":[],"kind":"fact","refs":[],"text":"10 ms budget","why":null}\n',
      "b3:336c585fb432305a64f27263b8223abf02f6fa63a23d2c7b63cc23b23e8484ab",
    ));

  it("escapes", () =>
    golden(
      "claim",
      'tab\there "quoted" back\\slash  del',
      null,
      [],
      [],
      '{"entities":[],"kind":"claim","refs":[],"text":"tab\\there \\"quoted\\" back\\\\slash \\u0001 del","why":null}\n',
      "b3:64e242c6c9e7d951cf2ac4a2fb19d3d0a02e5121c42de7512f5db30356568476",
    ));

  it("astral", () =>
    golden(
      "question",
      "Ship the \u{1F680} before Q4?",
      null,
      [],
      ["\u{1F680}"],
      '{"entities":["\u{1F680}"],"kind":"question","refs":[],"text":"Ship the \u{1F680} before Q4?","why":null}\n',
      "b3:c22f360fa43f4c365c333f2af3880284e526336e7dfce4fa7987e76a3db6c572",
    ));

  it("cafe", () =>
    golden(
      "fact",
      "café",
      null,
      [],
      ["Café"],
      '{"entities":["café"],"kind":"fact","refs":[],"text":"café","why":null}\n',
      "b3:b1b9c2bc5e3b2f15cd796a704c9c0552918c6830b3058465a77575ee81bad49d",
    ));
});

const textCid = (t: string) => cid("fact", t);

describe("equivalences", () => {
  it("CRLF, LF and CR are equivalent", () => {
    const lf = textCid("a\nb\nc");
    expect(textCid("a\r\nb\r\nc")).toBe(lf);
    expect(textCid("a\rb\rc")).toBe(lf);
    expect(textCid("a\r\nb\nc\r\n")).toBe(lf);
  });

  it("trailing whitespace and newlines are insignificant; leading is kept", () => {
    const base = textCid("a\nb");
    expect(textCid("a  \t\nb")).toBe(base);
    expect(textCid("a\nb\n\n\n")).toBe(base);
    expect(textCid("a  　\nb \n \n")).toBe(base);
    expect(textCid("  a\nb")).not.toBe(base);
  });

  it("trailing NBSP stripped, interior NBSP kept", () => {
    expect(textCid("10 ms ")).toBe(textCid("10 ms"));
    expect(textCid("10 ms")).not.toBe(textCid("10 ms"));
  });

  it("U+FEFF is not whitespace (unlike JS trimEnd)", () => {
    expect(textCid("x﻿")).not.toBe(textCid("x"));
  });

  it("NFD and NFC are equivalent", () => {
    expect(textCid("café")).toBe(textCid("café"));
    expect(cid("fact", "x", null, [], ["CAFÉ"])).toBe(cid("fact", "x", null, [], ["café"]));
  });

  it("smart punctuation is not folded", () => {
    expect(textCid("“x”")).not.toBe(textCid('"x"'));
  });

  it("ref/entity order, case and duplicates are insignificant", () => {
    expect(cid("fact", "x", null, ["b.rs", "a.rs"], ["Liquid", "settlement"])).toBe(
      cid("fact", "x", null, ["a.rs", "b.rs", " a.rs"], ["settlement", "liquid", "LIQUID"]),
    );
  });

  it("sorts by code point, not UTF-16 units", () => {
    // U+FFFD < U+1F680 by code point, but JS default sort puts the surrogate
    // pair (0xD83D...) first. U+FFFD is stable under NFC and case mapping.
    const c = canonicalContent("fact", "x", null, [], ["\u{1F680}", "\u{FFFD}"]);
    expect(c.entities).toEqual(["\u{FFFD}", "\u{1F680}"]);
    expect(["\u{FFFD}", "\u{1F680}"].sort()).toEqual(["\u{1F680}", "\u{FFFD}"]);
  });

  it("empty why is no why", () => {
    expect(cid("fact", "x", " \n")).toBe(cid("fact", "x"));
  });
});

describe("doc cid", () => {
  it("golden: matches the independent Python BLAKE3", () => {
    // python: blake3("# Spec\n\nBuild it.\n".encode()).hexdigest()
    expect(docCid("# Spec\n\nBuild it.")).toBe(
      "b3:85dc6e887b400e1986ea0a09195d1913f4528a3a83fd66339d96b6716543b9c5",
    );
  });

  it("is computed over the normalised body", () => {
    expect(docCid(normalizeText("# Spec  \r\n\r\nBuild it.\r\n\r\n"))).toBe(docCid("# Spec\n\nBuild it."));
  });
});

