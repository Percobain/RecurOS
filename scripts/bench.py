"""Measure what RecurOS costs, on this repository, right now.

Every number on the website comes from this script. Run it from the repo
root with `ctx` on your PATH:

    python scripts/bench.py

It writes web/data/benchmarks.json. Token counts use the same chars/4
estimate the packer itself uses, so the site and the tool never disagree.
Timings are the best of several runs: the floor is the honest number,
because everything above it is scheduler noise. Process startup is measured
separately and subtracted, since it is the cost of running a command at all
and not the cost of the work.
"""

import json
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "web" / "data" / "benchmarks.json"
CTX = os.environ.get("CTX_BIN", "ctx")
RUNS = 9


def tokens(text: str) -> int:
    return (len(text) + 3) // 4


def run(*args: str) -> str:
    proc = subprocess.run(
        [CTX, *args], capture_output=True, text=True, cwd=ROOT, encoding="utf-8"
    )
    if proc.returncode != 0:
        sys.exit(f"ctx {' '.join(args)} failed:\n{proc.stderr}")
    return proc.stdout


def floor_ms(args: list[str]) -> float:
    best = float("inf")
    for _ in range(RUNS):
        start = time.perf_counter()
        subprocess.run([CTX, *args], capture_output=True, cwd=ROOT)
        best = min(best, (time.perf_counter() - start) * 1000)
    return best


def main() -> None:
    data: dict = {}

    # What an agent is handed.
    pack = run("pack")
    data["pack_tokens"] = tokens(pack)
    data["pack_claims"] = pack.count("\n- ")

    # What it would otherwise have to read to learn the same things.
    prose = []
    for name in ["README.md", "AGENTS.md"]:
        path = ROOT / name
        if path.exists():
            prose.append({"file": name, "tokens": tokens(path.read_text(encoding="utf-8"))})
    for path in sorted((ROOT / "docs").glob("*.md")):
        prose.append(
            {"file": f"docs/{path.name}", "tokens": tokens(path.read_text(encoding="utf-8"))}
        )
    data["prose"] = prose
    data["prose_tokens"] = sum(p["tokens"] for p in prose)

    code_tokens, code_files = 0, 0
    for path in ROOT.glob("crates/*/src/*.rs"):
        code_tokens += tokens(path.read_text(encoding="utf-8"))
        code_files += 1
    data["code_tokens"] = code_tokens
    data["code_files"] = code_files

    # One real question, answered both ways.
    question = "why do we pull with rebase instead of fast forward only"
    top = run("search", question, "-n", "1").strip().splitlines()
    data["question"] = question
    data["answer_tokens"] = tokens(top[0]) if top else 0
    source = ROOT / "crates/ctx-git/src/sync.rs"
    data["answer_source"] = "crates/ctx-git/src/sync.rs"
    data["answer_source_tokens"] = tokens(source.read_text(encoding="utf-8"))

    # Speed, with the cost of merely starting a process taken out.
    startup = floor_ms(["--version"])
    data["ms_startup"] = round(startup, 1)
    data["ms_pack"] = round(max(floor_ms(["pack"]) - startup, 0.0), 1)
    data["ms_search"] = round(max(floor_ms(["search", "sync conflict rebase"]) - startup, 0.0), 1)

    reindex = run("reindex").strip()
    data["reindex"] = reindex
    words = reindex.split()
    data["claims_indexed"] = int(words[1]) if words and words[0] == "reindexed" else None
    data["ms_reindex"] = int(words[-1].removesuffix("ms")) if reindex.endswith("ms") else None

    home = Path(os.environ.get("CTX_HOME") or Path.home() / "ctx")
    data["log_bytes"] = sum(p.stat().st_size for p in home.glob("log/*/*.jsonl"))

    data["measured_at"] = time.strftime("%Y-%m-%d")
    names = {"win32": "Windows", "darwin": "macOS", "linux": "Linux"}
    data["platform"] = names.get(sys.platform, sys.platform)

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(data, indent=2))
    print(f"\nwrote {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
