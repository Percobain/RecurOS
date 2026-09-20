import Link from "next/link";
import { bench, fmt, ratio } from "@/lib/numbers";

export default function Home() {
  const factor = ratio(bench.prose_tokens, bench.pack_tokens);
  const questionFactor = ratio(bench.answer_source_tokens, bench.answer_tokens);

  return (
    <>
      <div className="hero">
        <div className="wrap">
          <p className="eyebrow">Local first · git synced · no account</p>
          <h1>One memory for every AI tool you use.</h1>
          <p className="lede">
            You decide something in ChatGPT. You refine it in claude.ai. Then you open Claude
            Code and it knows none of it. RecurOS records the decision once and{" "}
            <strong>compiles it into the context every one of them reads</strong>: the
            constraints, the rejected options, and the reason each was chosen.
          </p>
          <div className="cta">
            <Link className="btn primary" href="/docs">
              Get started
            </Link>
            <a className="btn" href="https://github.com/Percobain/RecurOS">
              GitHub
            </a>
          </div>
          <pre>
            <code>
              <span className="prompt">$ </span>cargo install --git
              https://github.com/Percobain/RecurOS ctx-cli{"\n"}
              <span className="prompt">$ </span>cd my-project && ctx init{"\n"}
              <span className="prompt">$ </span>ctx onboard{"  "}
              <span className="cmt"># fills this project from its own code</span>
            </code>
          </pre>
        </div>
      </div>

      <section>
        <div className="wrap">
          <h2>Curated context, not a bigger haystack</h2>
          <p className="section-lede">
            Most tools help an agent search more of your repository. RecurOS does the opposite:
            it keeps the handful of things that stay true and never needed searching for. These
            are this repository&rsquo;s own numbers, measured by{" "}
            <code>scripts/bench.py</code> on {bench.measured_at}.
          </p>
          <div className="versus">
            <div className="card">
              <h4>Prose an agent would read</h4>
              <div className="stat">{fmt(bench.prose_tokens)}</div>
              <div className="bar dim">
                <span style={{ width: "100%" }} />
              </div>
              <p>
                tokens across {bench.prose.length} files: the README, AGENTS.md and{" "}
                <code>docs/</code>. Most of it is true, and almost none of it is the answer to
                the question at hand.
              </p>
            </div>
            <div className="card">
              <h4>What RecurOS compiles</h4>
              <div className="stat">{fmt(bench.pack_tokens)}</div>
              <div className="bar">
                <span style={{ width: `${(bench.pack_tokens / bench.prose_tokens) * 100}%` }} />
              </div>
              <p>
                tokens, {bench.pack_claims} claims, fitted to a {700}-token budget and written
                into AGENTS.md. <strong>{factor}&times; smaller</strong>, and every line of it
                is a decision somebody actually made.
              </p>
            </div>
            <div className="card">
              <h4>One question, both ways</h4>
              <div className="stat">
                {bench.answer_tokens} <span style={{ color: "var(--dim)" }}>/</span>{" "}
                {fmt(bench.answer_source_tokens)}
              </div>
              <div className="bar">
                <span
                  style={{
                    width: `${(bench.answer_tokens / bench.answer_source_tokens) * 100}%`,
                  }}
                />
              </div>
              <p>
                &ldquo;{bench.question}?&rdquo; is {bench.answer_tokens} tokens as a recorded
                decision, or {fmt(bench.answer_source_tokens)} tokens of{" "}
                <code>{bench.answer_source}</code> to read and infer from, and the file never
                says <em>why</em>.
              </p>
            </div>
          </div>
          <p style={{ marginTop: 28, fontSize: 14 }}>
            <Link href="/docs/numbers">Every number, and how it was measured &rarr;</Link>
          </p>
        </div>
      </section>

      <section>
        <div className="wrap narrow">
          <h2>How it works</h2>
          <ol className="steps">
            <li>
              <h3>You say &ldquo;save that&rdquo;</h3>
              <p>
                In any chat or coding agent. One sentence becomes a <em>claim</em>: a decision,
                a constraint, a rejected option, a fact, an open question, with the reason it
                holds and the files it is about.
              </p>
            </li>
            <li>
              <h3>It lands in an append-only log</h3>
              <p>
                A private git repository you own. Content addressed with BLAKE3 over canonical
                JSON, so the same claim has the same id on every machine and nothing is ever
                rewritten or lost.
              </p>
            </li>
            <li>
              <h3>Every tool reads the same compiled context</h3>
              <p>
                A budgeted compiler picks what fits: relevance to what you are doing, weighted
                by kind and recency, minus redundancy. {bench.ms_pack} ms to compile, written
                to AGENTS.md for Claude Code and served over MCP to ChatGPT and claude.ai.
              </p>
            </li>
          </ol>
        </div>
      </section>

      <section>
        <div className="wrap">
          <h2>Already have a project?</h2>
          <p className="section-lede">
            An existing codebase already holds most of its own context; it is just spread across
            the README, the docs, and people&rsquo;s heads. RecurOS does not read repositories;
            the agent already sitting in yours does, and it is the one that knows which of what
            it read is worth keeping.
          </p>
          <pre>
            <code>
              <span className="prompt">$ </span>cd my-existing-project{"\n"}
              <span className="prompt">$ </span>ctx init{"\n"}
              <span className="prompt">$ </span>ctx onboard{"\n"}
              {"\n"}
              <span className="cmt">
                # prints a prompt. Paste it into Claude Code, Cursor or Codex.
              </span>
              {"\n"}
              <span className="cmt">
                # It reads your repo and records why this database, what must
              </span>
              {"\n"}
              <span className="cmt">
                # never break, what was tried and abandoned. Then: ctx pack
              </span>
            </code>
          </pre>
          <p style={{ fontSize: 14 }}>
            <Link href="/docs">Full setup, including ChatGPT and claude.ai &rarr;</Link>
          </p>
        </div>
      </section>

      <section style={{ borderBottom: "none" }}>
        <div className="wrap narrow">
          <h2>What it is not</h2>
          <ul className="doc-list" style={{ color: "var(--muted)", paddingLeft: 22 }}>
            <li>
              <strong style={{ color: "var(--text)" }}>Not a code indexer.</strong> It does not
              chunk or embed your source. What the code says, the agent can already read.
            </li>
            <li>
              <strong style={{ color: "var(--text)" }}>Not automatic.</strong> Nothing is saved
              unless you ask. A store fills up with noise the moment an agent is allowed to
              write to it on its own.
            </li>
            <li>
              <strong style={{ color: "var(--text)" }}>Not a service.</strong> There is no
              RecurOS server and no account. The store is a git repository you own; the
              optional cloud bridge is a Cloudflare Worker you deploy yourself.
            </li>
          </ul>
        </div>
      </section>
    </>
  );
}
