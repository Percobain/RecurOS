// Canonical form and CID — a byte-exact port of docs/canonical.md.
//
// If this disagrees with crates/ctx-core/src/canonical.rs by a single byte,
// claims written from claude.ai get a different address than the same claim
// written locally and dedup silently breaks. The golden tests in
// test/canonical.test.ts pin the same literal CIDs as the Rust suite.

import { blake3 } from "@noble/hashes/blake3.js";
import { bytesToHex } from "@noble/hashes/utils.js";

export const KINDS = ["fact", "decision", "rejected", "constraint", "question", "claim"] as const;
export type Kind = (typeof KINDS)[number];

export function isKind(k: unknown): k is Kind {
  return typeof k === "string" && (KINDS as readonly string[]).includes(k);
}

// Unicode White_Space, spelled out. NOT String.prototype.trim's set, which
// adds U+FEFF and omits U+0085.
const WHITESPACE = new Set<number>([
  0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x20, 0x85, 0xa0, 0x1680, 0x2000, 0x2001, 0x2002, 0x2003,
  0x2004, 0x2005, 0x2006, 0x2007, 0x2008, 0x2009, 0x200a, 0x2028, 0x2029, 0x202f, 0x205f,
  0x3000,
]);

// Every whitespace char is in the BMP, so checking UTF-16 units is exact:
// a surrogate half is never whitespace.
function isWs(s: string, i: number): boolean {
  return WHITESPACE.has(s.charCodeAt(i));
}

function trimEndWs(s: string): string {
  let end = s.length;
  while (end > 0 && isWs(s, end - 1)) end--;
  return s.slice(0, end);
}

function trimStartWs(s: string): string {
  let start = 0;
  while (start < s.length && isWs(s, start)) start++;
  return s.slice(start);
}

/** NFC, LF line endings, no trailing whitespace per line, no trailing newlines. */
export function normalizeText(s: string): string {
  const lf = s.normalize("NFC").replace(/\r\n/g, "\n").replace(/\r/g, "\n");
  const joined = lf.split("\n").map(trimEndWs).join("\n");
  let end = joined.length;
  while (end > 0 && joined.charCodeAt(end - 1) === 0x0a) end--;
  return joined.slice(0, end);
}

/** As normalizeText, plus leading whitespace removed (paths, URLs, tags). */
export function normalizeToken(s: string): string {
  return trimStartWs(normalizeText(s));
}

/** Compare by Unicode code point (== UTF-8 byte order), not UTF-16 units. */
export function compareCodePoints(a: string, b: string): number {
  const ia = a[Symbol.iterator]();
  const ib = b[Symbol.iterator]();
  for (;;) {
    const x = ia.next();
    const y = ib.next();
    if (x.done) return y.done ? 0 : -1;
    if (y.done) return 1;
    const cx = x.value.codePointAt(0)!;
    const cy = y.value.codePointAt(0)!;
    if (cx !== cy) return cx < cy ? -1 : 1;
  }
}

function sortedUnique(values: string[]): string[] {
  const out = values.filter((v) => v !== "").sort(compareCodePoints);
  return out.filter((v, i) => i === 0 || v !== out[i - 1]);
}

export interface CanonicalContent {
  kind: Kind;
  text: string;
  why: string | null;
  refs: string[];
  entities: string[];
}

export function canonicalContent(
  kind: Kind,
  text: string,
  why: string | null | undefined,
  refs: readonly string[] = [],
  entities: readonly string[] = [],
): CanonicalContent {
  const w = why == null ? null : normalizeText(why);
  return {
    kind,
    text: normalizeText(text),
    why: w === "" ? null : w,
    refs: sortedUnique(refs.map(normalizeToken)),
    // Lowercase first, then normalise: NFC must be the last transformation.
    entities: sortedUnique(entities.map((e) => normalizeToken(e.toLowerCase()))),
  };
}

// JSON.stringify escapes strings exactly as RFC 8785 requires (", \, \b \f
// \n \r \t, other C0 as lowercase \u00xx, everything else literal). Keys are
// written by hand in JCS (UTF-16) order; all are ASCII.
export function canonicalString(c: CanonicalContent): string {
  const arr = (xs: string[]) => "[" + xs.map((x) => JSON.stringify(x)).join(",") + "]";
  return (
    "{" +
    `"entities":${arr(c.entities)},` +
    `"kind":${JSON.stringify(c.kind)},` +
    `"refs":${arr(c.refs)},` +
    `"text":${JSON.stringify(c.text)},` +
    `"why":${c.why === null ? "null" : JSON.stringify(c.why)}` +
    "}\n"
  );
}

export function canonicalBytes(c: CanonicalContent): Uint8Array {
  return new TextEncoder().encode(canonicalString(c));
}

export function cidOf(c: CanonicalContent): string {
  return "b3:" + bytesToHex(blake3(canonicalBytes(c)));
}

/** CID for raw (un-normalised) input. */
export function cid(
  kind: Kind,
  text: string,
  why?: string | null,
  refs: readonly string[] = [],
  entities: readonly string[] = [],
): string {
  return cidOf(canonicalContent(kind, text, why, refs, entities));
}

/**
 * Content address of a document body that is already normalised with
 * normalizeText: "b3:" + hex(BLAKE3(utf8(body) + "\n")). Mirrors Rust
 * `ctx_core::record::doc_cid`.
 */
export function docCid(normalizedBody: string): string {
  return "b3:" + bytesToHex(blake3(new TextEncoder().encode(normalizedBody + "\n")));
}

/** True if nothing but whitespace remains — such claims are rejected. */
export function isBlank(s: string): boolean {
  return trimStartWs(trimEndWs(s)) === "";
}
