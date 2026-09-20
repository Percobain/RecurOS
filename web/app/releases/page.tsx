import Link from "next/link";

export const metadata = { title: "Releases" };

type Release = {
  version: string;
  date: string;
  state?: "now" | "next";
  title: string;
  body: React.ReactNode;
};

const RELEASES: Release[] = [
  {
    version: "unreleased",
    date: "in progress",
    state: "now",
    title: "Retrieval, onboarding and a name",
    body: (
      <>
        <p>
          The work since the core landed. Install from <code>main</code> until the first tag:{" "}
          <code>cargo install --git https://github.com/Percobain/RecurOS ctx-cli</code>.
        </p>
        <ul>
          <li>
            <strong>Renamed to RecurOS.</strong> The CLI stays <code>ctx</code> — it is the
            command, not the brand, and it was already in every config file.
          </li>
          <li>
            <strong>Retrieval reads more than words.</strong> A claim&rsquo;s file references
            are indexed, so <code>lib.rs</code> finds the claims about that file. Function words
            are dropped and camelCase is split. A strict pass and a loose pass are fused by
            reciprocal rank, then expanded one hop along supersession and shared references.
          </li>
          <li>
            <strong>A task now changes the pack.</strong> Two bugs cancelled it out: relevance
            had no usable scale, and topic coverage could outbid it entirely. Asking about sync
            used to return the same context as asking nothing.
          </li>
          <li>
            <strong>
              <code>ctx onboard</code>
            </strong>{" "}
            — a prompt that fills an existing repository&rsquo;s context from its own code, run
            by the agent already sitting in it.
          </li>
          <li>
            <strong>
              <code>ctx rename</code> and <code>ctx list</code>
            </strong>{" "}
            — carry a project&rsquo;s whole context to a new name; see every project and what it
            costs.
          </li>
          <li>
            <code>ctx spec save FILE</code> takes the file literally instead of unwrapping an
            example fence inside it. <code>ctx search</code> no longer matches deleted claims.
            The AGENTS.md marker is matched by its stable prefix, so changing its wording
            replaces the region instead of appending a second one.
          </li>
        </ul>
      </>
    ),
  },
  {
    version: "v0.1.0",
    date: "planned",
    state: "next",
    title: "First tagged release",
    body: (
      <>
        <p>What has to be true before a version number means anything:</p>
        <ul>
          <li>
            Prebuilt binaries for macOS, Linux and Windows, so installing does not require Rust.
          </li>
          <li>
            An <code>npx</code> installer that fetches them, and the shell one-liners wired to
            real release assets.
          </li>
          <li>The browser extension exercised in an actual browser, not just built.</li>
          <li>
            A <code>ctx setup</code> that walks through the private repo, the token and the
            Worker instead of asking you to read four sections.
          </li>
        </ul>
      </>
    ),
  },
  {
    version: "core",
    date: "2026-09",
    title: "The parts everything else stands on",
    body: (
      <>
        <p>
          Built in dependency order, each piece landing with its own tests before the next one
          started.
        </p>
        <ul>
          <li>
            <strong>Content addressing.</strong> BLAKE3 over RFC 8785 canonical JSON, with test
            vectors asserted as literals produced by an independent Python implementation.
          </li>
          <li>
            <strong>The log and sync.</strong> Append-only JSONL, one writer per machine per
            month, <code>git pull --rebase</code>, and a two-machine test against a bare remote.
          </li>
          <li>
            <strong>The index.</strong> SQLite with FTS5, treated as a cache that can be dropped
            and replayed from the log at any time.
          </li>
          <li>
            <strong>The branch graph.</strong> Scoped inheritance that narrows per hop, lateral
            edges that do not compose, and a cycle check on every load.
          </li>
          <li>
            <strong>The compiler.</strong> Budgeted submodular selection with a hard budget
            guarantee and deterministic output.
          </li>
          <li>
            <strong>The surfaces.</strong> MCP server (five tools, under 450 tokens of schema),
            loopback daemon, Chrome extension, Cloudflare Worker, and agent wiring for Claude
            Code, Cursor, Codex and Gemini CLI.
          </li>
          <li>
            <strong>Specs as documents.</strong> A separate record type with versions, rather
            than a seventh kind of claim.
          </li>
        </ul>
      </>
    ),
  },
];

export default function Releases() {
  return (
    <div className="wrap narrow" style={{ padding: "56px 20px 96px" }}>
      <h1>Releases</h1>
      <p className="lede">
        RecurOS is pre-1.0 and honest about it: there is no tagged release yet, so this page
        tracks what is on <code>main</code> and what has to be true before a version number
        means something. The{" "}
        <a href="https://github.com/Percobain/RecurOS/commits/main">commit history</a> is the
        full record.
      </p>

      {RELEASES.map((r) => (
        <div className="release" key={r.version}>
          <div className="release-meta">
            <span className={r.state === "now" ? "tag now" : "tag"}>{r.version}</span>
            <div>{r.date}</div>
          </div>
          <div>
            <h3>{r.title}</h3>
            {r.body}
          </div>
        </div>
      ))}

      <p style={{ marginTop: 48, fontSize: 14 }}>
        Upgrading never needs a migration: the log is the source of truth and the index is
        rebuilt whenever its schema changes.{" "}
        <Link href="/docs/how-it-works">How that works &rarr;</Link>
      </p>
    </div>
  );
}
