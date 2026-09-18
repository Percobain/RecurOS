# ContextOS protocol

This document describes everything that crosses a process or machine
boundary in ContextOS: the files on disk, the records in the log, how
machines stay in sync, how branches and packs work, and the interfaces that
the MCP server, the local daemon, the browser extension and the Cloudflare
Worker speak. It is normative for anyone writing another client. Each rule
is followed by the reason for it, because most of them exist to prevent a
specific failure.

How content addresses are computed (the `cid` fields below) is specified
separately in [canonical.md](canonical.md).

## Contents

1. Design principles
2. The store on disk
3. Identifiers
4. The log and its records
5. Reading, indexing and convergence
6. Synchronisation
7. Branches and routing
8. Packs
9. Files ContextOS writes into your repositories
10. The idea-to-build flow
11. MCP server
12. Local daemon (HTTP)
13. Chat protocols: `ctx-claims` and `ctx-spec`
14. Cloudflare Worker
15. Security model
16. Compatibility and versioning

## 1. Design principles

These principles explain most of the decisions in the rest of the document.

1. **The log is the only source of truth.** Everything else (the SQLite
   index, compiled packs, `AGENTS.md`, `SPEC.md`) is derived and can be
   deleted and rebuilt. This is what makes a lost cache or a lost machine
   harmless.
2. **Records are immutable and append-only.** Nothing is edited or deleted.
   Corrections, status changes and new versions are new records. This gives
   history, audit, rollback and conflict-free merging at once.
3. **One writer per file.** Each machine appends only to its own log files.
   Git can then combine machines' work without ever producing a merge
   conflict.
4. **No model in the write path or the merge path.** Saving and merging
   never call an LLM, so they are instant, free, offline-capable and
   deterministic. Humans decide what is worth keeping.
5. **Routing is declared, never guessed.** Which branch a repository or chat
   writes to comes from a file or a command, not from a model's judgement,
   because a small silent misfiling rate poisons a knowledge store.
6. **Everything works without a network or a remote.** Git sync, the Worker
   and the extension are additions on top of a fully local tool.
7. **No base64 in anything a model reads.** Models cannot reliably decode
   it and it wastes tokens. Everything is plain markdown or JSON.
8. **Correctness never depends on hooks firing.** Agent hooks are
   best-effort; the committed `AGENTS.md` alone always carries the context.

## 2. The store on disk

The store is a directory, `~/ctx` by default (`%USERPROFILE%\ctx` on
Windows), overridable with the `CTX_HOME` environment variable. It is a git
repository.

```
~/ctx/
  log/
    <machine-id>/<YYYY-MM>.jsonl   append-only; written only by that machine
    cloud/<YYYY-MM>.jsonl          written only by the Cloudflare Worker
  refs/
    branches.yaml                  branch definitions (section 7)
    active                         the branch chat surfaces write to
    weights.yaml                   optional packer weight overrides
    eval.yaml                      optional `ctx eval` questions
  packs/
    index.md                       overview of all branches
    <project>/<branch>.md          a dossier per branch, for the Worker
  .cache/ctx.db                    SQLite index (derived, disposable)
  .machine                         this machine's id
  .gitignore                       always contains .cache/ and .machine
  .gitattributes
```

Why each piece is shaped this way:

- **Per-machine directories** are the whole sync design (section 6).
- **Monthly files** keep any single file small enough to diff and to fetch
  through the GitHub API, and make "what happened in September" cheap to
  read.
- **`cloud/`** is the Worker's own shard, so browser-side writes obey the
  one-writer rule too.
- **`packs/` is committed** so the Worker can serve compiled context without
  reimplementing the packer in TypeScript. `ctx sync` regenerates it before
  every commit and only rewrites files whose content changed, so an
  unchanged store produces no commit.
- **`.cache/` is ignored** because it is derived; committing it would create
  conflicts and bloat.
- **`.machine` is ignored**, and `ctx` re-adds that line if it is ever
  missing. The id names the directory this machine writes to. If it were
  committed, a second machine would pull it and start writing into the first
  machine's shard, breaking the one-writer rule.
- **`.gitattributes`** sets `* text=auto eol=lf` so logs keep LF line
  endings on Windows, and `log/**/*.jsonl merge=union` as a safety net: if
  two clones of the same machine ever do touch one file, git keeps both
  sides' lines instead of conflicting.

### The machine id

On first use `ctx` derives the id from the hostname (lowercased, runs of
non-alphanumeric characters turned into `-`) and stores it in `.machine`.
Later hostname changes do not affect it, so a renamed laptop keeps writing
to the same shard. `cloud` is reserved for the Worker; a host literally
named "cloud" becomes `cloud-host`.

## 3. Identifiers

| Identifier | Form | Used for |
|---|---|---|
| Record id | ULID, 26 characters, e.g. `01M2STVE9QK4...` | Identity of every record |
| `cid` | `b3:` + 64 hex | Content address of a claim or document ([canonical.md](canonical.md)) |
| Short tag | `c:` + first 4 hex of the cid, e.g. `c:7f2a` | Referring to a claim in packs and commands |
| Merkle root | `b3:` + 64 hex (footers show 8) | Identity of a set of claims |
| Branch | `project/name`, e.g. `sovereign/code` | Which context a claim belongs to |

*Why ULIDs:* they sort by creation time, need no coordination between
machines (80 random bits per millisecond), and are readable. Ordering by id
is ordering by time, which the log, the index and the packer's tie-breaking
all rely on.

Commands that take an id accept any unique prefix of the ULID or of the
cid (`01M2ST`, `c:7f2a`, `b3:7f2a`), case-insensitively.

## 4. The log and its records

### 4.1 Line format

Each shard file is UTF-8 JSON Lines: one record per line, each line ending
in `\n`. Every record is an object with a `rec` field naming its type.

Writers must:

- take an exclusive OS file lock for the duration of the append, so two
  local processes (for example the CLI and the MCP server) cannot interleave
  bytes;
- if the file does not end in `\n` (a previous writer crashed mid-line),
  write a `\n` first, so the new record starts on its own line instead of
  being glued to garbage;
- write the whole line, then `fsync`.

Readers must:

- skip a final line without a trailing `\n` (a torn write) with a warning,
  never fail, and not consume it, so it is re-examined once it is completed
  or terminated;
- skip lines that are not valid JSON, and records whose `rec` they do not
  know, with a warning;
- recompute and verify the `cid` of claims and documents, skipping any that
  do not match (the line was edited by hand or corrupted).

*Why tagged records:* the log is permanent, so new record types must be
addable without a format break. Old readers skip what they do not
understand.

*Why skip rather than fail:* a single bad line must never make the whole
store unreadable. Warnings make the problem visible without blocking work.

Timestamps are RFC 3339 in UTC. Optional fields are omitted when empty.

### 4.2 `claim`

The only kind of knowledge record. Example:

```json
{"rec":"claim","id":"01M2STVE...","cid":"b3:32a2...","kind":"decision",
 "branch":"contextos/code","text":"Pull with git pull --rebase, not --ff-only",
 "why":"two machines that each commit always diverge ...",
 "refs":["crates/ctx-git/src/sync.rs"],"entities":["git","sync"],
 "status":"active","confidence":"medium","src":"cli",
 "t_valid":"2026-09-18T09:12:03.412Z","t_tx":"2026-09-18T09:12:03.412Z","tokens":11}
```

| Field | Meaning and reason |
|---|---|
| `id` | ULID. Identity. |
| `cid` | Content address. Must verify. |
| `kind` | One of six: `fact` (true about the world), `decision` (chosen), `rejected` (ruled out, with why), `constraint` (must hold), `question` (open), `claim` (a belief or bet). Exactly six, because every kind multiplies renderer and weighting logic; new distinctions belong in `entities`. `rejected` and `constraint` exist separately because they carry the knowledge people rarely write down and most often re-derive. |
| `branch` | Which context branch it belongs to (section 7). |
| `text` | What. Always rendered when the claim is packed. |
| `why` | Why. Optional. The first thing dropped when a pack runs short of budget, because agents mostly need the what and people need the why. |
| `refs` | Pointers: file paths (`src/x.rs:L88`), `repo@sha`, URLs, and two reserved prefixes: `ctx:<id>` (this claim was merged from that claim) and `doc:<name>` (this claim summarises that document). |
| `entities` | Lowercase topic tags, used for retrieval and for topic coverage in packs. |
| `supersedes` | Optional id of a claim this one replaces. Implies that claim becomes `superseded` (section 4.3). |
| `status` | The status the claim was written with: `active`, or `proposed` for cross-branch proposals. Its effective status can move later through status records. |
| `confidence` | `high`, `medium` (default) or `low`. Feeds the packer's weight. |
| `src` | Which surface wrote it: `cli`, `mcp`, `cloud`, `claude.ai`, `chatgpt`, `clipboard`, `merge`, and so on. |
| `helpful`, `harmful` | Optional per-machine counters (section 4.4). |
| `last_used` | Reserved. |
| `t_valid` | When it became true in the world. |
| `t_tx` | When it was recorded. |
| `tokens` | `ceil(characters(text) / 4)`, precomputed so the packer never tokenises inside its selection loop. |

*Why two timestamps:* bitemporal records answer both "what was true in
August" (`t_valid`) and "what did we believe in August" (`t_tx`). They
differ when something is recorded late, and the difference matters the
first time a decision is reversed.

*Why a character estimate for tokens:* the target models (Claude, GPT, Qwen,
Gemini) all use different tokenisers, so an exact count for one is only an
estimate for the others. Budgets are enforced against this same estimate,
so packing is self-consistent.

*Why stored fields are already normalised:* the writer stores exactly the
text it hashed, so what is on disk is what the `cid` covers.

### 4.3 `status`

Moves a claim to a new status. This is how claims are archived, how
proposals are accepted or rejected, and how deletion works (it never
removes anything).

```json
{"rec":"status","id":"01M2...","claim":"01M1...","to":"rejected",
 "reason":"too slow to confirm","src":"review","t_tx":"2026-09-18T10:00:00Z"}
```

The effective status of a claim is the maximum, in this order, of its own
`status` and every status record that targets it:

```
proposed < active < superseded < rejected < archived
```

A claim whose id appears in another claim's `supersedes` counts as having a
`superseded` record.

*Why a maximum:* taking the maximum is commutative, associative and
idempotent. It does not matter in which order machines' shards are read, or
whether a status record arrives before the claim it refers to: every reader
ends with the same status. This is what makes status changes merge without
conflicts. The cost is that a status can never go back down (an archived
claim cannot be un-archived); the answer is to record the claim again,
which is also more honest history.

*Why `reason` is kept:* the reason a proposal was rejected is often the
most valuable knowledge in the store. `ctx show` displays it.

### 4.4 `counter`

Feedback on how useful a claim was, recorded with `ctx rate <id> up|down`.

```json
{"rec":"counter","id":"01M2...","claim":"01M1...","machine":"laptop",
 "helpful":3,"harmful":0,"t_tx":"..."}
```

Each record carries one machine's **absolute** counts. Readers keep, per
machine, the maximum seen for each field, and the claim's totals are the
sums over machines.

*Why absolute per-machine values:* this is a grow-only counter (a G-counter
CRDT). Two machines rating concurrently never overwrite each other, and
reading the same record twice cannot double-count it.

### 4.5 `doc`

A long-form document attached to a branch. Today this is used for the spec
that ends a research phase.

```json
{"rec":"doc","id":"01M2...","branch":"habit/research","name":"spec",
 "title":"Habit Tracker v1","body":"# Habit Tracker v1\n\n## Goals\n...",
 "cid":"b3:3a40...","src":"claude.ai","t_tx":"..."}
```

| Field | Meaning |
|---|---|
| `name` | Short identifier, lowercase letters, digits and `-`, up to 64 characters. `spec` is the one `ctx build` hands to coding agents. |
| `title` | Given by the writer, otherwise the first non-empty line of the body without leading `#`, up to 120 characters, otherwise the name. |
| `body` | Markdown, normalised like claim text. |
| `cid` | Document address ([canonical.md](canonical.md), section 8). |

The **current version** of a document is the record with the greatest `id`
for its `(branch, name)` pair. Older versions stay in the log.

Writers must not write a new version whose `cid` equals the current
version's `cid`: saving an unchanged spec is a no-op.

*Why documents are not claims:* a claim is one short, atomic statement that
the packer can select or drop on its own. A spec is read whole and is often
thousands of tokens. Making it a seventh claim kind would break the packer's
assumptions and the rule of exactly six kinds. As a separate record type it
still lives in the log (so it syncs, versions and survives like everything
else) without pretending to be a claim.

*Why the body is in the log, not a separate file:* a file would be a second
source of truth that git could conflict on. In the log it inherits the
one-writer rule and append-only history for free.

## 5. Reading, indexing and convergence

The index (`.cache/ctx.db`, SQLite with FTS5 full-text search) is built by
reading every shard. For each shard it remembers the byte offset it has read
up to, so each command only reads new bytes: your own last append, or other
machines' shards that arrived with a `git pull`. When nothing changed this
costs a few file-size checks.

If a shard is ever shorter than its recorded offset (history rewritten
behind ContextOS's back), the index is discarded and rebuilt. `ctx reindex`
does the same on demand; 10,000 claims take about one second.

Records are keyed by `id`, so reading a line twice (for example after a
union merge duplicated it) is harmless.

The index stores each claim's effective status (section 4.3) and counter
totals (section 4.4), recomputed whenever a related record arrives, in any
order. Search uses FTS5 with Porter stemming, diacritics removed, and BM25
ranking that weights tags highest and `why` lowest. User queries are split
into words and each word is quoted, so FTS5 operators typed by a user can
never cause an error.

If the index's schema version differs from the binary's, it is dropped and
rebuilt. Upgrading ContextOS never needs a migration, because nothing in the
index is original data.

## 6. Synchronisation

### 6.1 Why conflicts cannot happen

Each machine writes only to `log/<its-id>/`, and the Worker only to
`log/cloud/`. When two machines both append and then sync, git sees two
commits that touch different files. Combining them never requires a merge
decision.

### 6.2 Pull with rebase

Syncing commits local changes, then runs `git pull --rebase --autostash`,
then pushes.

*Why rebase and not fast-forward only:* once two machines have each made a
commit, their histories have diverged, even though they touched different
files. A fast-forward-only pull would fail on every such round. Because the
changes are always to disjoint files, rebasing one machine's commits on top
of the other's cannot conflict. If a rebase ever does conflict, some writer
touched a file it does not own: ContextOS aborts the rebase (nothing is
lost) and reports it as the bug it is.

A pull from a remote with no commits yet is treated as "nothing to pull",
not as an error, so the first push to a new repository works.

`git` is run as a subprocess with `GIT_TERMINAL_PROMPT=0` (so a hook can
never hang waiting for a password). This reuses the user's existing
credential helpers and SSH configuration and avoids a C library dependency.
If the user has no git identity configured, commits use `ContextOS
<ctx@localhost>` rather than failing.

### 6.3 When sync happens

| Trigger | What happens |
|---|---|
| `ctx save` | Nothing on the network. Saving is always instant and offline. |
| `ctx sync` | Regenerate `packs/`, commit, pull (rebase), push. With no remote configured it only commits. |
| MCP `ctx_append` | 30 seconds after the last write (so a burst becomes one commit), the MCP server syncs. Pending syncs are flushed when the agent session ends. |
| Session start (Claude Code hook) | Pull with a 1.5 second limit; failure is reported and ignored so a session always starts. |
| `ctx build` | Pull with a 10 second limit, so research done in the browser a minute ago is included. |
| `ctx daemon` | Full sync every 5 minutes on a background thread, when a remote is configured. |
| Worker write | Committed directly to the remote through the GitHub API. |

With no remote everything still works locally; this is a normal state, not
an error.

## 7. Branches and routing

### 7.1 What a branch is

A branch is a named context inside a project, such as `sovereign/research`
and `sovereign/code`. It is a field on each claim plus an entry in
`refs/branches.yaml`. It is **not** a git branch: the store has one git
branch, `main`.

*Why not git branches:* the point is to work on research and on code at the
same time. Git branches are exclusive (you are on one at a time), and
switching would hide one context from the other.

The branch `default` (no project) holds claims saved before any project
exists. It accepts every kind and inherits nothing.

### 7.2 `refs/branches.yaml`

```yaml
sovereign:                 # project
  research:                # branch: sovereign/research
    holds: [fact, question, claim, rejected, decision, constraint]
    budget: 1200
    compile: dossier
  code:
    parent: research
    holds: [decision, constraint, rejected, question, fact]
    inherits:
      research: [decision, constraint, rejected]
    budget: 700
    compile: agents-md
    binds: [claude-code, codex, cursor, gemini]
    pins: []               # claim ids always included in packs
    archived: false
```

| Key | Meaning and reason |
|---|---|
| `holds` | Kinds this branch accepts. Saving another kind is refused, with a suggestion of a sibling that holds it (`try --to sovereign/research`). This keeps branches clean without anyone policing them. Empty means all kinds. |
| `parent` | Ancestor edge. |
| `inherits` | Kinds visible from other branches. The parent's entry scopes the ancestor edge; any other key is a lateral edge. Names without `/` are in the same project. |
| `budget` | Token budget when compiling this branch's own `compile` projection. |
| `compile` | Default projection: `agents-md`, `dossier`, `prose` or `markdown`. |
| `binds` | Agents that default to this branch (informational). |
| `pins` | Claims always included in packs, charged to the budget. |
| `archived` | Hidden from indexes and packs; its claims are archived. |

The file is validated on every load and save: names must be valid, every
edge must point at a defined branch, and the graph must be acyclic
(checked with Kahn's algorithm). It is written atomically (temporary file,
then rename), so a crash cannot leave half a file.

*Why validate cycles:* an undetected cycle would make pool resolution loop
forever.

### 7.3 What a branch can see (its pool)

```
pool(b) = own(b)
        ∪ for each ancestor p: own(p) limited to kinds allowed at every hop
        ∪ for each lateral s:  own(s) limited to inherits[s]
```

- **Ancestor edges are transitive, and narrow at each hop.** If `c`
  inherits `[decision, fact]` from `b`, and `b` inherits `[decision]` from
  `a`, then `c` sees only decisions from `a`.
- **Lateral edges are not transitive.** If `c` looks at `b` and `b` looks at
  `a`, `c` does not see `a`.

*Why scoped inheritance:* `code` needs research's conclusions, not its forty
open questions. Wholesale inheritance is how context bloats.

*Why laterals do not chain:* otherwise every branch would eventually see
every other branch and inheritance would stop meaning anything.

### 7.4 Templates

`ctx branch new <name> --template <t>` and `ctx new <idea>` create branches
from four templates:

| Template | Holds | Inherits from parent | Budget | Compiles to |
|---|---|---|---|---|
| research | every kind | decision, fact | 1200 | dossier |
| code | decision, constraint, rejected, question, fact | decision, constraint, rejected | 700 | agents-md |
| gtm | claim, decision, rejected, question | decision, claim, constraint | 900 | prose |
| writing | claim, fact, decision, rejected, question | decision, fact, claim | 1500 | dossier |

*Why research holds constraints:* most constraints (API limits, budgets,
legal requirements) are discovered during research, and `code` inherits
them from there.

*Why code inherits rejected approaches:* the builder is exactly who needs
to know what was already ruled out, so it does not propose it again.

### 7.5 Routing: which branch a write goes to

In order of precedence:

1. An explicit branch (`ctx save --to research`, a tool's `branch`
   argument). A bare name is resolved in the current project.
2. The `.ctx.yaml` file found in the working directory or the nearest
   parent directory:

   ```yaml
   project: sovereign
   branch: code
   ```

   It is committed to the code repository, so every clone and every agent
   in it routes the same way.
3. `refs/active`, set by `ctx use <branch>` or `ctx new <idea>`. This is
   what chat surfaces use, since they have no working directory.
4. `default`.

*Why never ask a model:* it costs tokens, it is unpredictable, and a small
silent misfiling rate poisons the store. A declared binding is free and
always right.

### 7.6 Proposals, review, merge and archive

- **Proposals.** A session on one branch that learns something belonging to
  another writes a claim with `status: proposed` on the target branch
  (`ctx_propose`). Proposed claims are never packed. `ctx review accept`
  appends a status record to `active`; `ctx review reject --reason`
  appends one to `rejected`, and the reason is kept forever.
- **Merge.** `ctx branch merge src --into dst` appends a copy of each
  active claim from `src` that `dst` holds, with a `ctx:<source id>` ref.
  The originals stay active where they were written, which keeps merges
  reversible and auditable, and they sync like any other write. Merging
  twice copies nothing new.
- **Archive.** `ctx branch archive` appends an `archived` status record for
  each claim on the branch and marks it archived in `branches.yaml`. Nothing
  is deleted.

## 8. Packs

A pack is a branch compiled for one reader under a token budget. Packing is
pure and deterministic: the same inputs always produce byte-identical
output.

### 8.1 Projections and budgets

| Projection | Reader | Default budget | Reasons (`why`) |
|---|---|---|---|
| `agents-md` | coding agents, written to `AGENTS.md` | 700 | only with leftover budget |
| `markdown` | local models and pipes (`ctx pack \| ollama run ...`) | 1500 | only with leftover budget |
| `dossier` | claude.ai, ChatGPT, any chat | 4000 | always |
| `prose` (`handoff`) | a human teammate | unlimited | always |

A branch's own `budget` applies when compiling its own `compile`
projection. The dossiers published to `packs/` for the Worker always use
the dossier budget, because in a chat the window, not the price, is the
constraint.

*Why agents get so little:* the pack is loaded into every agent session
and is part of the cached prompt prefix; a small, dense pack keeps that
cheap and leaves room for the actual work.

### 8.2 How claims are chosen

1. **Pool.** The branch's pool (section 7.3), limited to `active` and
   `superseded` claims.
2. **Task.** If a task is given (`--task`, or a tool's `task` argument), the
   index is searched for it and only the best 200 matches in the pool are
   kept, scored with Reciprocal Rank Fusion (`1 / (60 + rank)`). RRF needs
   no calibration between different rankers, so semantic search can be
   added later as a second ranker. If the search finds nothing, the whole
   pool is used, so an unhelpful task never produces an empty pack.
3. **Weight.** Each claim gets
   `w = τ(kind) · κ(confidence) · ρ(recency) · η(usefulness) · σ(status)`:
   - τ: constraint 1.0, decision 1.0, rejected 0.9, fact 0.8, question 0.7,
     claim 0.6;
   - κ: high 1.0, medium 0.8, low 0.5;
   - ρ: halves every 180 days of age, measured against the newest claim in
     the pool rather than the clock (so packs are reproducible), and never
     decays for constraints;
   - η: `(1 + helpful) / (1 + helpful + 2 · harmful)`;
   - σ: active 1.0, superseded 0.15 (kept small but non-zero because "we
     used to think X, then changed to Y" is half the value of a handoff),
     anything else 0.
4. **Selection.** Claims are chosen to maximise
   `relevance · weight + 0.6 · topic coverage − 0.4 · redundancy` within the
   budget, where topic coverage rewards covering more entity tags and
   redundancy penalises word overlap with claims already chosen. The
   objective is submodular (each addition helps less than the last), which
   allows lazy greedy selection by gain per token with a known quality
   guarantee, fast enough to run at every session start (well under a
   millisecond for 200 candidates). Ties are broken by id.
5. **Reasons.** For agent projections, claims are first packed without
   `why`; if more than 15% of the budget is left, reasons are added to the
   highest-weight claims until it runs out. This gives agents the most
   claims and people the reasoning on what matters most. Dossiers and
   handoffs include reasons from the start.
6. **Unlimited budgets** (handoffs) include every claim with positive
   weight, because redundancy exists to spend a tight window well, not to
   hide a superseded claim behind the one that replaced it.

All constants live in one structure and can be overridden per store in
`refs/weights.yaml`. `ctx eval` measures a set of questions with known
answers against packs at several budgets, so the weights can be tuned with
data instead of intuition.

### 8.3 The budget guarantee

Every claim's cost is computed from its exact rendered bullet. After
rendering, the whole output is measured; if it is over budget (section
headings are only estimated during selection), the least valuable detail is
removed first (a reason, then a whole claim) and the pack is rendered again.
If even the fixed text (header, rules, protocol instructions) does not fit,
a compact form without them is emitted. The footer's token total is
recomputed until it equals the estimate of the final text. A property test
checks all of this for random stores and budgets from 100 to 5000.

### 8.4 Layout

Claims are grouped into sections by kind (for example Constraints,
Decisions, "Rejected: do not propose these again", Facts, Open questions,
and "Changed over time" for superseded claims). Within a section they are
ordered by weight. Each bullet is:

```
- <text> (`ref`, `ref`) _(source branch, if inherited)_ [c:7f2a]
  - why: <reason>
```

Documents visible from the branch are listed (never inlined) at the top,
with their size and where to read them, for example ``read it in
`SPEC.md` ``. `agents-md` packs end with the operating rules below;
dossiers end with the chat protocols of section 13.

```
### Working with ctx
Active branch: <branch>. The context above is compiled; don't re-derive it.
Save only when I explicitly say so, plus one batched call at session end.
Use ctx_append(kind, text, why, refs). Don't log progress or summaries.
Cross-branch material: ctx_propose(target_branch, ...).
```

*Why agents save only when told:* told to save "whenever it decides
something", an agent logs "implemented the handler" forty times. The human
stays the one who decides what is knowledge.

### 8.5 The footer

Every pack ends with one line:

```
<!-- ctx/1 b=sovereign/code n=47 t=688/700 root=b3:4e91c2a7 gen=01JQ8F2M v=0.1.0 -->
```

| Field | Meaning |
|---|---|
| `ctx/1` | Footer format version |
| `b` | Branch compiled |
| `n` | Number of claims included |
| `t` | Estimated tokens of the whole pack / budget (`inf` for unlimited) |
| `root` | Merkle root of the included claims' cids (first 8 hex digits) |
| `gen` | Newest record id in the store when compiled (first 8 characters) |
| `v` | ContextOS version |

It makes any pack reproducible and debuggable: given the same store
generation and version, the same pack comes out, and `ctx verify <root>`
tells whether another machine has the same claims (`in-sync`), fewer
(`behind`), or ones this machine has not seen (`ahead-or-diverged`).

## 9. Files ContextOS writes into your repositories

`ctx init` (and `ctx build`) write these into a code repository. All of
them are safe to run repeatedly.

| File | Content | Commit it? |
|---|---|---|
| `.ctx.yaml` | project and branch binding (section 7.5) | yes |
| `AGENTS.md` | the `agents-md` pack, inside markers | yes |
| `SPEC.md` | the project's spec, if one exists | yes |
| `CLAUDE.md`, `GEMINI.md` | a line `@AGENTS.md` (an import) | yes |
| `.mcp.json`, `.cursor/mcp.json`, `.gemini/settings.json` | an `mcpServers.ctx` entry | yes |
| `~/.codex/config.toml` | a `[mcp_servers.ctx]` table | (user config) |
| `.claude/settings.local.json` | the SessionStart hook | no, it is per user |

### 9.1 `AGENTS.md`

ContextOS owns only the region between

```
<!-- ctx:begin (generated by ContextOS: edit with `ctx`, not by hand) -->
...
<!-- ctx:end -->
```

and replaces only that region. Everything outside it is the user's and is
never touched. The region is refreshed on every `ctx save` in the
repository, on `ctx sync`, and at session start.

*Why `AGENTS.md`:* it is read by most coding agents without any
configuration, so it works for a teammate who has never installed
ContextOS and keeps working if a hook breaks. It is the floor everything
else is built on.

### 9.2 `SPEC.md`

The current version of the project's `spec` document, found on the bound
branch, then on the branches it inherits from, then anywhere in the same
project. The first line is a marker:

```
<!-- ctx:doc name=spec cid=b3:... branch=habit/research (generated by ContextOS; ...) -->
```

When a newer version exists, the file is replaced only if its content still
matches the `cid` in the marker. If someone edited the file, or it has no
marker (a hand-written `SPEC.md`), it is left alone and reported.

*Why the marker:* it lets ContextOS tell "a file we wrote and nobody
touched" from "a file someone changed", so a new spec version from the
browser can update the repo automatically without ever overwriting a
person's edits.

### 9.3 Agent configuration

MCP entries run `ctx mcp`. When `ctx` is on the PATH, the plain command is
written, because these files are committed and must work on teammates'
machines; otherwise the absolute path is written (quoted if it contains
spaces) and the user is told to fix their PATH before committing.

Existing configuration is never modified: only a missing `ctx` entry is
added, invalid JSON is left untouched and reported, and a settings file that
already defines hooks is never merged into. The hook goes in
`.claude/settings.local.json`, which is not committed, so teammates without
ContextOS never get a failing hook.

The SessionStart hook (`ctx hook session-start`) does a bounded pull,
refreshes `SPEC.md` and `AGENTS.md`, and prints context only if it changed
after the agent already loaded `AGENTS.md`. It always exits successfully.
It never runs on every prompt, because changing the prompt on every turn
would invalidate prompt caching for the whole conversation, which costs far
more than the text saves.

## 10. The idea-to-build flow

The flow ContextOS is designed around: an idea starts in a chat, is
researched there, ends as a spec, and is built by a coding agent, without
the user ever copying context between tools.

| Step | Command or action | What is written |
|---|---|---|
| Start | `ctx new habit-tracker` | branches `habit-tracker/research` and `habit-tracker/code`; `refs/active` = research |
| Research | talk in claude.ai or ChatGPT with the Worker connector (or the extension, or the clipboard) | claims on the research branch |
| Spec | say "ctx spec" in the chat | a `doc` named `spec` on the research branch, plus a decision claim with a `doc:spec` ref |
| Build | `ctx build habit-tracker` | pulls, creates `./habit-tracker`, runs `git init`, writes `.ctx.yaml`, `SPEC.md`, `AGENTS.md` and agent configuration |
| Code | `claude` in that directory | the agent starts knowing the spec, the decisions, the constraints and what was rejected |
| Iterate | a new spec version from the chat | the next session start updates `SPEC.md` and tells the agent to re-read it |

`ctx map` draws a project as a Mermaid "metro map": each branch is a line,
claims are stations in the order they were recorded, colour shows the kind,
and dashed arrows show inheritance and supersession. It renders directly on
GitHub.

## 11. MCP server

`ctx mcp` serves the Model Context Protocol over stdio: newline-delimited
JSON-RPC 2.0 on stdin and stdout, diagnostics on stderr. Coding agents
start it; it runs in the repository's directory, so routing uses that
repository's `.ctx.yaml`.

It keeps no protocol state. It answers `initialize` (echoing the client's
protocol version), `ping`, `tools/list`, `tools/call`, and empty
`resources/list` and `prompts/list`. Notifications get no response. Batches
are supported. Nothing depends on `initialize` having happened, in line with
stateless MCP.

*Why hand-written rather than an SDK:* the surface is five tools and a few
methods; owning it keeps behaviour identical across client versions and the
binary small.

### 11.1 The five tools

Exactly five, because every tool schema is sent with every turn. Their
combined schema is kept under 450 tokens, and a test enforces this.

| Tool | Arguments | Result |
|---|---|---|
| `ctx_index` | none | The overview: current branch, branches with claim counts, and how to use the other tools (about 250 tokens). Anything else is discoverable from here. |
| `ctx_pack` | `branch?`, `task?`, `budget?` (100 to 200,000), `doc?` | The compiled pack in the branch's own projection. With `doc`, the full text of that document instead (for example `doc: "spec"`), or the list of available documents. |
| `ctx_search` | `query`, `branch?` | Up to 20 matching claims with kind, branch, status, reason and short tag. |
| `ctx_append` | `kind`, `text`, `branch?`, `why?`, `refs?`, `doc?`, `doc_name?` | Records a claim (only when the user asks). With `doc`, also saves that markdown as a document (default name `spec`) and adds a `doc:<name>` ref to the claim. |
| `ctx_propose` | `target_branch`, `kind`, `text`, `why?` | Records a proposed claim on another branch for `ctx review`. |

Errors are returned inside the tool result with `isError: true`, so the
model can read them and correct itself (for example an unknown kind, or a
kind the branch does not hold, with the suggested branch).

Every call first reads any new log bytes, so it sees claims written by other
processes since the previous call. After a write, the repository's
`AGENTS.md` is refreshed and a sync is scheduled (section 6.3).

## 12. Local daemon (HTTP)

`ctx daemon` serves an HTTP API for the browser extension on
`127.0.0.1:7777`, loopback only.

### 12.1 Authentication

Every request except `GET /v1/health` must carry
`Authorization: Bearer <token>`. The token is 64 random hex characters in
`~/.ctx/token` (created with mode 0600 on Unix), printed by
`ctx daemon token`, and compared in constant time. It is never written to
logs.

Requests that carry an `Origin` header starting with `http://` or
`https://` are refused with 403, even with a valid token.

*Why the Origin check:* any website can make a browser send requests to
`localhost`. The extension's background service worker sends a
`chrome-extension://` origin, so refusing web origins means no web page can
even probe the API.

### 12.2 Endpoints

| Method and path | Request | Response |
|---|---|---|
| `GET /v1/health` | | `{"ok":true,"version":"0.1.0"}` |
| `GET /v1/branches` | | `{"active":"p/b","branches":[{"name":"p/b","claims":12}]}` |
| `GET /v1/pack` | query: `branch?`, `task?`, `budget?`, `for?` (`dossier` default, `agents-md`, `markdown`, `prose`, `index`) | the pack as `text/markdown` |
| `GET /v1/search` | query: `q`, `branch?` | `{"claims":[<claim>...]}` (up to 50) |
| `POST /v1/claims` | `{"branch?","src?","claims":[{"kind","text","why?","refs?","entities?"}]}`, at most 200 claims | `{"saved":[{"id","cid"}],"duplicates":[{"id","cid"}],"errors":["..."]}` |
| `POST /v1/docs` | `{"branch?","name?","title?","body","src?"}`, body at most 2 MB | `{"id","cid","name","branch","title","duplicate":bool}` |
| `GET /v1/docs` | query: `branch?` | `{"docs":[{"name","title","branch","tokens","t_tx"}]}` |

`branch` defaults as in section 7.5. `src` is reduced to letters, digits,
`.`, `-` and `_` (40 characters) and defaults to `browser`. Invalid claims
in a batch are reported individually; valid ones are still saved.

## 13. Chat protocols: `ctx-claims` and `ctx-spec`

Chat surfaces without the Worker connector hand knowledge back through
markdown code fences. Every dossier ends with instructions that teach the
model both.

### 13.1 `ctx-claims`

When the user says "ctx save", the model replies with exactly one fenced
block:

````
```ctx-claims
[{"kind":"decision","text":"Use a federation peg","why":"covenants confirm too slowly","refs":["docs/settlement.md"]}]
```
````

`kind` is one of the six kinds; `why`, `refs` and `entities` are optional.
Consumers: the extension's "Save to ctx" button (sends to
`POST /v1/claims`) and `ctx save --paste` (reads the clipboard). A bare JSON
array without the fence is also accepted. If several blocks are present the
last one wins, since it is the model's final answer. Invalid JSON is an
error, never a partial save.

### 13.2 `ctx-spec`

When the user says "ctx spec", the model writes the whole spec as markdown
in one block fenced with **four** backticks:

`````
````ctx-spec
# Habit Tracker v1

## Data model
```ts
type Habit = { id: string }
```
````
`````

Consumers: the extension's "Save spec to ctx" button (sends to
`POST /v1/docs`) and `ctx spec save --paste`. Any fence of three or more
backticks or tildes tagged `ctx-spec` is accepted, and it ends at the next
line consisting of the same fence. Text without such a fence is taken as the
spec as-is.

*Why four backticks:* specs contain code blocks. A three-backtick outer
fence would be closed by the first inner code block; a longer outer fence
cannot be.

*Why fences instead of reading the page:* chat page layouts change
constantly, so selectors break. Code fences are plain markdown, survive
every redesign, and are copied faithfully to the clipboard. The clipboard
is the ground-truth path: if the extension breaks, the user loses two
keystrokes, not the workflow.

## 14. Cloudflare Worker

The Worker gives claude.ai and ChatGPT the same five tools, as a custom MCP
connector, with no extension. Each user deploys it to their own Cloudflare
account and points it at their own private GitHub repository holding the
store, so there is no shared service and nothing to bill on anyone else's
behalf.

### 14.1 Endpoint

`POST /mcp/<CTX_SECRET>`: stateless JSON-RPC 2.0 with JSON responses (no
server-sent events, no sessions). Any instance can serve any request, which
keeps it inside the free plan. Other paths return 404; `GET` returns 405.

*Why the secret in the path:* chat connectors accept a URL, and a long
random path segment is the simplest credential they can all carry. The URL
must be kept private like a password.

Configuration: `GITHUB_TOKEN` (a fine-grained token with contents
read/write on the one store repository), `GITHUB_REPO` (`owner/name`),
`GITHUB_BRANCH` (default `main`), `CTX_SECRET`.

### 14.2 Tool behaviour

| Tool | Behaviour |
|---|---|
| `ctx_index` | Returns `packs/index.md`, with the current branch from `refs/active`. |
| `ctx_pack` | Returns `packs/<branch>.md`; with `task`, appends matching claims from the log; with `doc`, returns that document (branch first, then any branch of the same project). The packer is not reimplemented: packs are compiled on a laptop by `ctx sync`. |
| `ctx_search` | Keyword search over all `log/**/*.jsonl`. |
| `ctx_append` | Builds a claim (and, with `doc`, a document) with the canonical form ported to TypeScript, and appends the line or lines to `log/cloud/<YYYY-MM>.jsonl` in one GitHub Contents API update, retrying on a version conflict. An unchanged document is not written again. |
| `ctx_propose` | As `ctx_append`, with `status: "proposed"`. |

When a tool's `branch` is omitted the Worker uses `refs/active`, then
`default`, so after `ctx new <idea>` and a sync, chat research lands in the
idea's research branch with no further setup.

Writes appear on laptops at the next `ctx sync`, `ctx build` or session
start.

### 14.3 Request limit and cost

The Cloudflare Workers Free plan allows 100,000 requests per day and never
bills: requests beyond the limit fail until the next day. To stay safely
below it, the Worker uses the Workers rate limiting binding to allow 62
requests per minute (at most 89,280 per day) and answers requests beyond
that with HTTP 429 and a JSON-RPC error explaining the limit. The limit
lives in `wrangler.toml`. The binding counts per Cloudflare location, so it
is a close guard rather than an exact global count, which is sufficient for
one user's traffic.

## 15. Security model

- **Local by default.** Nothing leaves the machine unless the user adds a
  git remote or deploys the Worker. There is no ContextOS service and no
  telemetry.
- **The store is private data.** The remote should be a private repository.
  The Worker's token should be fine-grained to that one repository.
- **The daemon** listens on loopback only, requires the token, and refuses
  web origins.
- **Integrity.** Readers verify every claim and document against its
  content address, so a tampered line is detected and skipped.
- **Hooks and configs** only add ContextOS's own entries and never modify
  or merge into anything else.

## 16. Compatibility and versioning

- **Log records** are permanent. New fields must be optional, and new record
  types must use a new `rec` tag, so older readers skip them.
- **Canonical form and content addresses** are frozen
  ([canonical.md](canonical.md), section 12).
- **The index** is disposable and versioned; a version mismatch triggers a
  rebuild, never a migration.
- **The MCP tool set** stays at five; new capabilities are added as optional
  arguments.
- **The pack footer** carries a format version (`ctx/1`).
