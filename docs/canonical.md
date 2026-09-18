# Canonical form and CID (normative)

Every implementation that computes a claim CID (the Rust core, the Cloudflare
Worker, anything else) must follow this document byte for byte. The reference
implementations are `crates/ctx-core/src/canonical.rs` and the independent
`tests/golden/reference.py`; the golden cases in `crates/ctx-core/tests/golden.rs`
are the conformance suite.

## Inputs

`kind`, `text`, `why` (optional), `refs` (list), `entities` (list). Nothing else
— not the id, branch, timestamps, status or counters — is part of the content
address. The same content on two branches has the same CID.

## Whitespace set

"Whitespace" means exactly the Unicode `White_Space` characters:

```
U+0009–U+000D, U+0020, U+0085, U+00A0, U+1680, U+2000–U+200A,
U+2028, U+2029, U+202F, U+205F, U+3000
```

JavaScript's `trim`/`trimEnd` use a different set (they include U+FEFF and
exclude U+0085). Do not use them.

## Normalising free text (`text`, `why`)

1. Unicode NFC.
2. Replace `\r\n` with `\n`, then any remaining `\r` with `\n`.
3. Split on `\n`; strip trailing whitespace from every line; rejoin with `\n`.
4. Strip all trailing `\n`.

Leading whitespace is preserved (indentation is content). A `why` that is empty
after normalisation is treated as absent.

No other folding is applied. In particular smart quotes, em-dashes and interior
non-breaking spaces are **not** mapped to ASCII: they are part of the author's
text and NFC leaves them alone.

## Normalising identifiers (`refs`, `entities`)

As free text, then also strip leading whitespace. Entities are lowercased
(Unicode default case mapping, no locale) **before** normalising, so NFC is the
last transformation. Empty values are dropped. Each list is deduplicated and
sorted by Unicode code point (equivalently, by UTF-8 bytes — note this differs
from JavaScript's default `Array.prototype.sort`, which compares UTF-16 units).

## Serialisation

Build the object

```json
{"entities": [...], "kind": "<kind>", "refs": [...], "text": "...", "why": "..." | null}
```

where `kind` is one of `fact decision rejected constraint question claim`, and
serialise it with RFC 8785 (JCS): keys sorted by UTF-16 code units, no
insignificant whitespace, strings escaped exactly as ECMAScript
`JSON.stringify` does (`\"`, `\\`, `\b \f \n \r \t`, other C0 controls as
lowercase `\u00xx`, everything else literal). `why` is always present, as
`null` when absent. Append a single `\n`. Encode as UTF-8.

## CID

```
cid = "b3:" + lowercase_hex(BLAKE3-256(canonical_bytes))
```

Nothing in this pipeline is base64-encoded.
