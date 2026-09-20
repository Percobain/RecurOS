"""Independent reference implementation of the RecurOS canonical form.

This exists so the golden CIDs asserted in Rust are not derived from the Rust
code under test. It follows docs/canonical.md, using only Python's stdlib for
normalisation/JSON and the `blake3` package for hashing:

    python -m pip install blake3
    python tests/golden/reference.py

It prints each golden case's canonical bytes and CID.
"""

import json
import sys
import unicodedata

import blake3

WHITESPACE = "".join(
    chr(c)
    for c in [
        0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x20, 0x85, 0xA0, 0x1680,
        *range(0x2000, 0x200B), 0x2028, 0x2029, 0x202F, 0x205F, 0x3000,
    ]
)


def norm_text(s):
    s = unicodedata.normalize("NFC", s)
    s = s.replace("\r\n", "\n").replace("\r", "\n")
    s = "\n".join(line.rstrip(WHITESPACE) for line in s.split("\n"))
    return s.rstrip("\n")


def norm_token(s):
    return norm_text(s).lstrip(WHITESPACE)


def canonical(kind, text, why=None, refs=(), entities=()):
    why = norm_text(why) if why is not None else None
    if why == "":
        why = None
    refs = sorted({r for r in (norm_token(r) for r in refs) if r})
    ents = sorted({e for e in (norm_token(e.lower()) for e in entities) if e})
    obj = {
        "kind": kind,
        "text": norm_text(text),
        "why": why,
        "refs": refs,
        "entities": ents,
    }
    # Keys are ASCII, so code-point order == JCS UTF-16 order. Python's escaping
    # with ensure_ascii=False matches JCS for strings: short escapes for
    # \b\f\n\r\t, \u00xx (lowercase) for other C0 controls, all else literal.
    body = json.dumps(obj, sort_keys=True, ensure_ascii=False, separators=(",", ":"))
    return (body + "\n").encode("utf-8")


def cid(*args, **kwargs):
    return "b3:" + blake3.blake3(canonical(*args, **kwargs)).hexdigest()


# Keep in sync with crates/ctx-core/tests/golden.rs.
CASES = {
    "minimal": dict(kind="fact", text="hello"),
    "full": dict(
        kind="decision",
        text="Use SQLite as the index",
        why="Postgres needs a server; the index is a disposable cache.",
        refs=["docs/spec.md", "crates/ctx-store-sqlite/src/lib.rs:L10"],
        entities=["storage", "sqlite"],
    ),
    "multiline": dict(kind="constraint", text="line one\n  indented two\nline three"),
    "smart_punctuation": dict(
        kind="rejected", text="“Covenants” — rejected; it’s too slow"
    ),
    "nbsp_interior": dict(kind="fact", text="10 ms budget"),
    "escapes": dict(kind="claim", text='tab\there "quoted" back\\slash \x01 del\x7f'),
    "astral": dict(kind="question", text="Ship the \U0001F680 before Q4?", entities=["\U0001F680"]),
    "cafe": dict(kind="fact", text="café", entities=["Café"]),
}

def check(rust_test_path):
    """Assert each `fn golden_<name>` in the Rust test pins the CID we compute."""
    import re

    src = open(rust_test_path, encoding="utf-8").read()
    failures = 0
    for name, case in CASES.items():
        m = re.search(r"fn golden_" + name + r"\(\).*?\"(b3:[0-9a-f]{64})\"", src, re.S)
        expected = cid(**case)
        if not m:
            print(f"MISSING  golden_{name}: expected {expected}")
            failures += 1
        elif m.group(1) != expected:
            print(f"MISMATCH golden_{name}: rust {m.group(1)} != reference {expected}")
            failures += 1
        else:
            print(f"ok       golden_{name}")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    if len(sys.argv) == 3 and sys.argv[1] == "--check":
        check(sys.argv[2])
    for name, case in CASES.items():
        print(f"{name}")
        print(f"  canonical: {canonical(**case)!r}")
        print(f"  cid:       {cid(**case)}")
