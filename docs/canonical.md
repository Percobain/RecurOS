# Canonical form, content addresses and Merkle roots

This document is normative. Every implementation that computes a ContextOS
content address must follow it byte for byte. Today there are three: the
Rust core (`crates/ctx-core/src/canonical.rs`, `jcs.rs`, `merkle.rs`), the
Cloudflare Worker (`worker/src/canonical.ts`), and an independent Python
reference used only to produce test expectations
(`tests/golden/reference.py`).

Each rule below is followed by the reason it exists. The reasons matter
because this is the one part of ContextOS where a subtle mistake corrupts
data instead of crashing. If two implementations disagree by a single byte,
the same claim gets two different addresses: deduplication silently stops
working, and Merkle roots computed on two machines never match.

## 1. What a content address is for

Every claim carries a `cid` (content identifier) of the form

```
b3:<64 lowercase hex digits>
```

It is the BLAKE3-256 hash of the claim's canonical bytes. It gives us four
things for free:

1. **Deduplication.** Saving the same statement twice, from the CLI on one
   machine and from ChatGPT through the Worker on another, produces the same
   `cid`, so the second save is recognised as a duplicate.
2. **Integrity.** A reader recomputes the `cid` from the stored content. If
   it does not match, the line was edited by hand or corrupted, and the
   reader skips it with a warning.
3. **Idempotent merges.** Merging the same content twice cannot create two
   claims that look different.
4. **Pack identity.** A pack is identified by a Merkle root over the `cid`s
   of the claims in it (section 8), which lets two machines tell whether
   they are looking at the same knowledge.

A `cid` is not the claim's identity. Identity is the `id` (a ULID, see
`protocol.md`). Two claims can share a `cid` (the same statement saved on two
branches) while having different ids, histories and statuses.

## 2. What is hashed, and what is not

Only five fields are part of the content:

| Field | Included because |
|---|---|
| `kind` | "Use Postgres" as a `decision` and as a `rejected` approach mean opposite things. |
| `text` | It is the statement itself. |
| `why` | The reason is part of what was claimed. The same decision for a different reason is different knowledge. |
| `refs` | Pointers to code, docs or URLs change what the claim is about. |
| `entities` | Topic tags are chosen by the author and affect retrieval. |

Everything else is deliberately left out:

| Field | Excluded because |
|---|---|
| `id`, `t_tx`, `t_valid` | Two people recording the same fact at different times are recording the same fact. |
| `branch` | The same statement on `research` and on `code` is the same content. Deduplication is applied per branch by the store, not by the hash. |
| `status`, `supersedes` | These describe what happened to a claim later. Claims are immutable; including mutable state would make the address change over time. |
| `confidence`, `src`, `tokens`, counters | Metadata about the claim, not the claim. |

## 3. Normalising free text (`text`, `why`)

Free text goes through four steps, in this order.

### 3.1 Unicode NFC

The string is converted to Unicode Normalization Form C.

*Why:* the same visible text can be encoded in more than one way. "café"
can be the single code point U+00E9 or "e" followed by the combining accent
U+0301. macOS file APIs, some keyboards and some web pages produce the
decomposed form, others the composed one. NFC picks one representation, so
text copied from anywhere hashes the same.

*Why NFC and not NFKC:* NFKC also folds "compatibility" characters, for
example the ligature "ﬁ" to "fi", superscript digits to plain digits, and
full-width letters to ASCII. That changes meaning in technical text (`x²` is
not `x2`) and changes what the author wrote. NFC only unifies encodings of
the same character.

### 3.2 Line endings to LF

Every `\r\n` becomes `\n`, then every remaining lone `\r` becomes `\n`.

*Why:* Windows tools write CRLF, old Mac text uses CR, everything else uses
LF. A claim typed on Windows and the same claim pasted on Linux must hash the
same. `\r\n` is replaced first so that it becomes one newline rather than
two.

### 3.3 Strip trailing whitespace from every line

The text is split on `\n`, trailing whitespace is removed from each line,
and the lines are joined again with `\n`.

"Whitespace" means exactly these code points (the Unicode `White_Space`
property):

```
U+0009 U+000A U+000B U+000C U+000D U+0020 U+0085 U+00A0 U+1680
U+2000 U+2001 U+2002 U+2003 U+2004 U+2005 U+2006 U+2007 U+2008 U+2009 U+200A
U+2028 U+2029 U+202F U+205F U+3000
```

*Why strip at all:* trailing spaces are invisible, editors add and remove
them freely, and chat interfaces often append them when copying. They never
carry meaning in a claim.

*Why an explicit list:* language runtimes disagree about what whitespace is.
JavaScript's `trimEnd` removes U+FEFF (the byte order mark) but not U+0085
(next line). Rust's `char::is_whitespace` follows whatever Unicode version the
compiler ships with. A hard-coded list cannot drift between languages or
toolchain versions. Implementations must use this list and must not call
their language's built-in trim.

*Why no-break space is included:* chat interfaces and word processors often
turn a trailing space into U+00A0 when copying. Only trailing no-break spaces
are removed; one inside a line (as in "10 ms") is content and is kept.

### 3.4 Strip trailing newlines

All `\n` at the end of the string are removed.

*Why:* "decided X" and "decided X\n" are the same claim. Blank trailing lines
come from editors and shell heredocs, not from the author's intent. The
serialised document still ends in exactly one newline (section 6), which is
where that convention belongs.

### 3.5 What is deliberately not changed

- **Leading whitespace is kept.** Indentation can be meaningful, for example
  in a code snippet or a nested list inside a claim.
- **Smart quotes, dashes and ellipses are kept.** “quoted” is not folded to
  "quoted". Folding would make two different texts share an address and
  would make rendered packs show something the author did not write. If a
  user saves the same sentence once with straight quotes and once with curly
  quotes, they get two claims; `ctx refine` reports such near-duplicates for
  a human to merge.
- **Case is kept** in `text` and `why`.
- **Internal runs of spaces are kept.** Collapsing them could damage code or
  tables.

An empty `why` after normalisation is treated exactly like an absent `why`.
An empty `text` after normalisation is an error: a claim must say something.

## 4. Normalising identifiers (`refs`, `entities`)

Each ref and each entity is normalised like free text (section 3) and then
has its **leading** whitespace removed as well. A path, a URL or a tag never
starts with meaningful spaces.

Entities are additionally lowercased, **before** the normalisation above, so
the order is: lowercase, then NFC, then line endings and whitespace.

*Why lowercase entities:* tags are for grouping and retrieval. `Settlement`,
`settlement` and `SETTLEMENT` are one topic.

*Why lowercase first:* case mapping can produce text that is no longer in
NFC. Applying NFC last guarantees the stored tag is normalised. Lowercasing
uses the Unicode default case mapping with no locale, which is what both
Rust's `to_lowercase` and JavaScript's `toLowerCase` do. (Locale-aware
mapping, such as Turkish dotless i, would make the hash depend on the
machine's language settings.)

*Why refs are not lowercased:* file paths are case-sensitive on Linux and
URLs can be case-sensitive after the host.

After normalisation, empty values are dropped, then each list is
**deduplicated and sorted by Unicode code point**.

*Why sort:* the order in which someone typed tags or refs carries no meaning,
and different clients will send them in different orders.

*Why code point order:* it is identical to sorting the UTF-8 bytes, which is
what Rust's default string ordering does. JavaScript's default
`Array.prototype.sort` compares UTF-16 code units instead, which orders
characters above U+FFFF (emoji, many CJK extension characters) differently.
The Worker therefore sorts with an explicit code point comparison.

## 5. The canonical object

The normalised fields are placed in a JSON object with exactly these five
keys:

```json
{"entities": [...], "kind": "decision", "refs": [...], "text": "...", "why": "..." }
```

- `kind` is one of `fact`, `decision`, `rejected`, `constraint`, `question`,
  `claim`. These spellings are frozen: renaming a kind would change the
  address of every claim of that kind.
- `why` is always present. When there is no reason it is JSON `null`.

*Why always include `why`:* a fixed key set means there is exactly one way
to serialise a claim with no reason. Omitting the key in some
implementations and writing `null` in others would produce two addresses for
the same claim.

`entities` and `refs` are always present, as empty arrays when there are
none, for the same reason.

## 6. Serialisation: RFC 8785 (JCS)

The object is serialised with the JSON Canonicalization Scheme, RFC 8785:

- Object keys are sorted by their UTF-16 code units. (All five keys are
  ASCII, so this is plain alphabetical order: `entities`, `kind`, `refs`,
  `text`, `why`.)
- No whitespace between tokens.
- Strings are escaped exactly as ECMAScript `JSON.stringify` does: `"` and
  `\` are escaped, the control characters U+0008, U+0009, U+000A, U+000C and
  U+000D use their short forms (`\b \t \n \f \r`), other characters below
  U+0020 are written as `\u00xx` with lowercase hex, and everything else,
  including U+007F, U+2028, U+2029 and all non-ASCII text, is written
  literally as UTF-8.

A single `\n` is appended after the closing brace, and the result is encoded
as UTF-8. These bytes are the canonical form.

*Why JCS:* ordinary JSON has many valid spellings of the same value (key
order, spacing, and whether "é" is written as the character itself or as a
six-character backslash-u escape). JCS defines exactly
one, and it is a published standard that other languages can implement
without reading our code.

*Why our own encoder:* the output is hashed, so we must control every byte,
and the encoder is only about eighty lines. The Rust encoder refuses numbers
outright. Number formatting is by far the hardest part of RFC 8785, and the
canonical object contains no numbers, so refusing them removes the largest
source of cross-language disagreement.

*Why the trailing newline:* it makes the canonical form a well-formed text
file line, convenient when inspecting it with command-line tools. It is part
of the hashed bytes, so every implementation must add it.

## 7. The hash

```
cid = "b3:" + lowercase_hex(BLAKE3-256(canonical_bytes))
```

*Why BLAKE3:* it is fast (hashing is a negligible part of verifying ten
thousand claims during a reindex), it produces 256-bit output with no known
weaknesses, it has mature implementations in Rust, JavaScript and Python,
and its internal tree structure fits the Merkle roots we build on top.

*Why hexadecimal and not base64:* hex is unambiguous, case-stable after
lowercasing, safe in file names and URLs, and readable by people and
models. ContextOS never puts base64 in anything a model reads, because
models cannot reliably decode it and it tokenises poorly.

*Why the `b3:` prefix:* it names the algorithm. If the hash function ever has
to change, new addresses can use a new prefix and old ones stay valid and
distinguishable.

### Short tags

Rendered packs show the first four hex digits as a tag, for example
`[c:7f2a]`. Tags are for humans and models to refer back to a claim. They
are not unique: commands that accept them (`ctx show c:7f2a`) resolve the
prefix against the index and ask for more characters if it matches more than
one claim.

## 8. Document addresses

Documents (the `doc` log record, used for specs) have their own address:

```
doc_cid = "b3:" + lowercase_hex(BLAKE3-256(utf8(normalised_body) + "\n"))
```

The body is normalised exactly like claim text (section 3). There is no JSON
wrapper, because a document has one content field.

*Why documents are addressed separately from claims:* a document's address
answers "is this the same text as the current version?", which is how saving
an unchanged spec twice becomes a no-op, and how `SPEC.md` detects that it
was edited by hand (see `protocol.md`). The title and name are not part of
the address: renaming or retitling a spec does not make its text different.

## 9. Merkle roots

A pack, or the state of a branch, is identified by a Merkle root over a set
of claim `cid`s:

1. Take the set of `cid` strings, remove duplicates, and sort them
   (byte order).
2. Each leaf is `BLAKE3(0x00 || utf8(cid))`.
3. Each interior node is `BLAKE3(0x01 || left || right)`, pairing nodes left
   to right. If a level has an odd number of nodes, the last one is carried
   up to the next level unchanged.
4. The root is written as `b3:` + lowercase hex. The root of an empty set is
   `b3:` + hex of BLAKE3 of zero bytes.

*Why a tree and not one hash of everything:* a single hash only says that
two sets differ. A tree lets two parties compare subtree hashes and find
which claims differ by exchanging a logarithmic number of hashes, so a
browser extension could fetch a three-claim difference instead of a whole
pack.

*Why sort first:* the tree's shape must depend only on which claims are in
the set, never on the order they were read, so two machines build identical
trees independently.

*Why the 0x00 and 0x01 prefixes:* domain separation. Without them an
attacker could present an interior node's input as if it were a leaf (a
second-preimage attack on Merkle trees, as described for Certificate
Transparency in RFC 6962).

*Why promote the odd node instead of duplicating it:* duplicating the last
node lets two different sets produce the same root (a known weakness of
Bitcoin's original tree). Promotion does not.

Pack footers show the first eight hex digits of the root, and `ctx verify`
accepts a prefix.

## 10. Conformance cases

These canonical forms and addresses were produced by the independent Python
reference and are asserted as literal strings in
`crates/ctx-core/tests/golden.rs` and `worker/test/canonical.test.ts`. CI
recomputes them with the Python reference on every push, so the literals
cannot drift.

| Case | Input | `cid` |
|---|---|---|
| minimal | fact, text `hello` | `b3:ef5c7ba58908aa7aa4d1cf7f2d3011d40beeede4d51537e621ec5ed9f3350d0f` |
| full | decision, why, two refs, two tags | `b3:1e9044b2e035f6cd8b08eb9046555ee65a011bdebe6d7ab1ec4681dbc9c99daa` |
| multiline | constraint, three lines, one indented | `b3:938d7c14cf862cd0dc4f598a0fd9c593b7045e4cc99d162f4604ecc4f143f624` |
| smart punctuation | rejected, curly quotes and a dash kept as written | `b3:a7ee6724b5ff15af1c72ed85d9727c08acc093146566eb8e095fcc5b29236aff` |
| interior no-break space | fact, `10` U+00A0 `ms budget` | `b3:336c585fb432305a64f27263b8223abf02f6fa63a23d2c7b63cc23b23e8484ab` |
| escapes | claim, tab, quotes, backslash, U+0001, U+007F | `b3:64e242c6c9e7d951cf2ac4a2fb19d3d0a02e5121c42de7512f5db30356568476` |
| astral | question, an emoji in text and as a tag | `b3:c22f360fa43f4c365c333f2af3880284e526336e7dfce4fa7987e76a3db6c572` |
| café | fact, composed é, tag `Café` | `b3:b1b9c2bc5e3b2f15cd796a704c9c0552918c6830b3058465a77575ee81bad49d` |
| document | body `# Spec` LF LF `Build it.` | `b3:85dc6e887b400e1986ea0a09195d1913f4528a3a83fd66339d96b6716543b9c5` |

The equivalence tests next to the golden cases check the rules rather than
fixed values: CRLF, CR and LF give the same address; trailing spaces,
trailing no-break spaces and trailing blank lines do not change it; composed
and decomposed accents are equal; tag order, tag case and duplicate refs do
not matter; an empty `why` equals no `why`; and the kind always matters.

## 11. Writing another implementation

A checklist, based on the mistakes the three existing implementations had
to avoid:

1. Use the whitespace list in section 3.3, never the language's trim.
2. Replace `\r\n` before replacing lone `\r`.
3. Lowercase entities before NFC, with locale-independent case mapping.
4. Sort refs and entities by code point, not by UTF-16 unit.
5. Always emit all five keys; `why` is `null` when absent.
6. Escape strings exactly as `JSON.stringify`, including lowercase hex in
   `\u00xx` and literal U+2028 and U+2029.
7. Append one `\n` before hashing.
8. Reproduce every case in section 10 before trusting the implementation.

## 12. Changing these rules

Any change to sections 3 to 9 changes the address of existing claims, so it
is a breaking change to every store. It would require a new `cid` prefix
(for example `b3v2:`), a migration that rewrites nothing in the log (the log
is append-only) but recomputes addresses in the index, and a period in which
readers accept both prefixes. Golden literals must never be edited to make a
failing test pass; a failure means the implementation changed, not the
expectation.
