import Link from "next/link";

export const metadata = { title: "Getting started" };

export default function Docs() {
  return (
    <>
      <h1>Getting started</h1>
      <p className="lede">
        RecurOS is one program, called <code>ctx</code>. No Docker, no database to run, no
        account to create. Everything below works offline except <code>ctx sync</code>.
      </p>

      <h2>Install</h2>
      <p>One command. A single binary, with nothing to install before it.</p>
      <p>
        <strong>macOS and Linux</strong>
      </p>
      <pre>
        <code>
          <span className="prompt">$ </span>curl -fsSL https://recuros.vercel.app/install.sh | sh
        </code>
      </pre>
      <p>
        <strong>Windows</strong> (PowerShell)
      </p>
      <pre>
        <code>
          <span className="prompt">&gt; </span>irm https://recuros.vercel.app/install.ps1 | iex
        </code>
      </pre>
      <p>
        Or build it yourself, which is the only route that needs{" "}
        <a href="https://rustup.rs">Rust</a>:{" "}
        <code>cargo install --git https://github.com/Percobain/RecurOS ctx-cli --locked</code>
      </p>
      <p className="note">
        Every binary is built by GitHub Actions from the tag and published beside a{" "}
        <code>SHA256SUMS</code> file. See <Link href="/releases">Releases</Link>.
      </p>

      <h2>Wire a project</h2>
      <p>
        From inside any project folder. This is idempotent: run it again whenever you add a new
        agent.
      </p>
      <pre>
        <code>
          <span className="prompt">$ </span>cd my-project{"\n"}
          <span className="prompt">$ </span>ctx init
        </code>
      </pre>
      <p>It writes four things, and only these:</p>
      <ul>
        <li>
          <code>.ctx/config.yaml</code>: which context branch this repo uses. Commit it.
        </li>
        <li>
          <code>.ctx/commands.md</code>: the command list, so your agent can drive{" "}
          <code>ctx</code> without you learning it.
        </li>
        <li>
          <code>AGENTS.md</code>: the compiled context, inside markers. Anything you wrote in
          that file yourself is kept.
        </li>
        <li>
          Agent configs it finds: <code>CLAUDE.md</code>, <code>.mcp.json</code>,{" "}
          <code>.cursor/mcp.json</code>, and a Claude Code session hook. Only RecurOS&rsquo;s own
          entries are touched.
        </li>
      </ul>
      <p>
        Restart your coding agent in that folder. From then on you talk to it normally, and it
        runs <code>ctx</code> for you.
      </p>

      <h2>Record something</h2>
      <p>Say it in plain words to Claude Code, Cursor or Codex:</p>
      <pre>
        <code>
          Save that as a decision: we use SQLite, not Postgres,{"\n"}
          because there is no server to run.
        </code>
      </pre>
      <p>Or do it yourself:</p>
      <pre>
        <code>
          <span className="prompt">$ </span>ctx save &quot;Use SQLite, not Postgres&quot; -k
          decision \{"\n"}
          {"      "}-w &quot;no server to run&quot; -r src/db.rs{"\n"}
          <span className="ok">saved [c:7f2a] decision &rarr; my-project/code</span>
        </code>
      </pre>
      <p>
        A claim is one of six kinds: <code>decision</code>, <code>constraint</code>,{" "}
        <code>rejected</code>, <code>fact</code>, <code>question</code>, <code>claim</code>. The{" "}
        <code>-w</code> is the reason it holds, which is the part that stops a future agent from
        undoing it. The <code>-r</code> is what it is about, and it is also how retrieval finds
        it later.
      </p>
      <p className="note">
        <strong>Nothing is saved unless you ask.</strong> Agents that write to the store on their
        own fill it with progress notes within a day, which is why that is off by design.
      </p>

      <h2>See what your agents see</h2>
      <pre>
        <code>
          <span className="prompt">$ </span>ctx pack{"          "}
          <span className="cmt"># the compiled context, exactly as given</span>
          {"\n"}
          <span className="prompt">$ </span>ctx pack --task &quot;fixing the sync
          conflict&quot;{"\n"}
          <span className="prompt">$ </span>ctx log{"           "}
          <span className="cmt"># everything recorded, newest last</span>
          {"\n"}
          <span className="prompt">$ </span>ctx list{"          "}
          <span className="cmt"># every project and how big it is</span>
        </code>
      </pre>

      <h2>Housekeeping</h2>
      <pre>
        <code>
          <span className="prompt">$ </span>ctx rename old-name new-name{"\n"}
          <span className="prompt">$ </span>ctx delete c:7f2a{"           "}
          <span className="cmt"># one claim</span>
          {"\n"}
          <span className="prompt">$ </span>ctx delete my-project --cloud{"  "}
          <span className="cmt"># everywhere, including the chats</span>
        </code>
      </pre>
      <p>
        Deleting hides a claim from every surface and syncs that everywhere, but keeps it in the
        log: <code>ctx log --all</code> still shows it. An append-only log is what makes two
        machines able to merge without a conflict, so nothing is ever truly erased.
      </p>

      <p style={{ marginTop: 40 }}>
        Next: <Link href="/docs/existing-project">filling a project that already exists</Link>,
        or <Link href="/docs/chats">connecting ChatGPT and claude.ai</Link>.
      </p>
    </>
  );
}
