export const metadata = { title: "What we used, and why" };

type Row = { choice: string; instead: string; why: string };

const STACK: Row[] = [
  {
    choice: "Rust, one binary",
    instead: "Node or Python",
    why: "It has to start fast enough to run on every prompt, be installable without a runtime, and hold a file lock correctly. A daemon exists, but it is the same binary: ctx daemon, not a second thing to install.",
  },
  {
    choice: "Git as the sync layer",
    instead: "A hosted database",
    why: "Every user already has a private git remote and knows how to audit one. It gives history, distribution and access control for free, and it means there is no RecurOS server to trust or pay for.",
  },
  {
    choice: "Append-only JSONL",
    instead: "Editing rows",
    why: "One writer per file per machine means two laptops never touch the same bytes, so a pull is a rebase that cannot conflict. It is also readable with cat when something goes wrong.",
  },
  {
    choice: "BLAKE3 over RFC 8785",
    instead: "UUIDs, or a hash of the raw JSON",
    why: "Canonical JSON makes the id depend on the content and nothing else: not key order, not whitespace, not which language wrote it. The same claim saved from Rust, TypeScript and Python gets the same id.",
  },
  {
    choice: "SQLite + FTS5",
    instead: "A vector database",
    why: "The corpus is hundreds of short curated sentences, not millions of chunks. BM25 over that is fast, exact, explainable and has no model to download. Embeddings buy recall on large fuzzy corpora, which this is not.",
  },
  {
    choice: "MCP, five tools",
    instead: "A bespoke plugin per tool",
    why: "One protocol reaches Claude Code, Cursor, Codex, ChatGPT and claude.ai. The schemas are held under 450 tokens in total by a test, because they are paid for on every single turn whether used or not.",
  },
  {
    choice: "A Cloudflare Worker you deploy",
    instead: "A service we run",
    why: "Browser chats cannot run a local binary, so they need an HTTP endpoint. Making it yours keeps the promise that no third party holds your context, and the free tier covers it with a hard request cap.",
  },
  {
    choice: "AGENTS.md on disk",
    instead: "Injecting context into every prompt",
    why: "A plain file is the floor: it works for a teammate who does not have RecurOS, it survives every hook bug, and you can read it. Injection was tried and rejected, because it makes the context invisible and unreviewable.",
  },
];

const REJECTED: Row[] = [
  {
    choice: "Agents saving on their own",
    instead: "",
    why: "Tried it. Within a day the store fills with progress notes and restatements of what the code already says, and then nobody trusts it. Saving is explicit, plus one batched call at the end of a session.",
  },
  {
    choice: "Injecting context into every prompt",
    instead: "",
    why: "It spends tokens on turns that do not need them and hides what the model was told. A compiled file you can open beats a hidden preamble.",
  },
  {
    choice: "Indexing the source tree",
    instead: "",
    why: "Your agent can already read your code, and a second, staler copy of it is not a memory. RecurOS keeps only what the code cannot say: why, what must not break, what was abandoned.",
  },
  {
    choice: "Fast-forward-only pulls",
    instead: "",
    why: "Local records are new lines in a file only this machine writes. Rebasing them onto someone else's work is always correct; refusing to is just a failed sync.",
  },
  {
    choice: "A seventh claim kind for specs",
    instead: "",
    why: "A spec is a long document with versions, not a one-sentence claim. It became its own record type instead of distorting the claim model.",
  },
];

export default function Decisions() {
  return (
    <>
      <h1>What we used, and why</h1>
      <p className="lede">
        These are the actual entries in RecurOS&rsquo;s own store. It is built using itself, so
        the reasoning below is the same text its agents are given.
      </p>

      <h2>The stack</h2>
      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Chose</th>
              <th>Over</th>
              <th style={{ minWidth: 380 }}>Because</th>
            </tr>
          </thead>
          <tbody>
            {STACK.map((r) => (
              <tr key={r.choice}>
                <td>{r.choice}</td>
                <td style={{ color: "var(--dim)" }}>{r.instead}</td>
                <td className="wrap-cell">{r.why}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <h2>Rejected, and not up for discussion</h2>
      <p>
        A store of decisions is only worth having if it also remembers the ones that were turned
        down. These are recorded as <code>rejected</code> claims, which means every agent reads
        them and stops proposing them.
      </p>
      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Not doing</th>
              <th style={{ minWidth: 420 }}>Because</th>
            </tr>
          </thead>
          <tbody>
            {REJECTED.map((r) => (
              <tr key={r.choice}>
                <td>{r.choice}</td>
                <td className="wrap-cell">{r.why}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <h2>Trade-offs we know about</h2>
      <ul>
        <li>
          <strong>Retrieval is lexical.</strong> Ask in words nobody used and the strict pass
          finds nothing; graph expansion and a relevance floor soften it, but a semantic layer
          would do better. It is optional-by-design work, not a rewrite.
        </li>
        <li>
          <strong>Someone has to curate.</strong> A store nobody prunes decays. That is a real
          cost, and it is the cost of a memory you can trust rather than a summary you cannot.
        </li>
        <li>
          <strong>Rust is a build step.</strong> Prebuilt binaries remove it; until the first
          tagged release, installing means compiling.
        </li>
      </ul>
    </>
  );
}
