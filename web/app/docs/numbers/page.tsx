import { bench, fmt, kb, ratio } from "@/lib/numbers";

export const metadata = { title: "Numbers" };

export default function Numbers() {
  return (
    <>
      <h1>Numbers</h1>
      <p className="lede">
        Every figure on this site is measured by{" "}
        <a href="https://github.com/Percobain/RecurOS/blob/main/scripts/bench.py">
          <code>scripts/bench.py</code>
        </a>{" "}
        against RecurOS&rsquo;s own store, and written to{" "}
        <code>web/data/benchmarks.json</code>. Last run {bench.measured_at} on{" "}
        {bench.platform}. Nothing here is estimated by hand.
      </p>

      <h2>Context size</h2>
      <p>
        Token counts use the same chars&nbsp;/&nbsp;4 estimate the packer itself uses, so the
        site and the tool never disagree. It is an approximation of a real tokeniser, applied
        identically to both sides of every comparison.
      </p>
      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th>What an agent reads</th>
              <th>Tokens</th>
              <th>Relative</th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <td>Compiled context ({bench.pack_claims} claims, AGENTS.md)</td>
              <td className="num">{fmt(bench.pack_tokens)}</td>
              <td className="num">1&times;</td>
            </tr>
            {bench.prose.map((p) => (
              <tr key={p.file}>
                <td>
                  <code>{p.file}</code>
                </td>
                <td className="num">{fmt(p.tokens)}</td>
                <td className="num" style={{ color: "var(--dim)" }}>
                  {ratio(p.tokens, bench.pack_tokens)}&times;
                </td>
              </tr>
            ))}
            <tr>
              <td>
                <strong>All prose together</strong>
              </td>
              <td className="num">
                <strong>{fmt(bench.prose_tokens)}</strong>
              </td>
              <td className="num">
                <strong>{ratio(bench.prose_tokens, bench.pack_tokens)}&times;</strong>
              </td>
            </tr>
            <tr>
              <td>
                Rust source ({bench.code_files} files under <code>crates/*/src</code>)
              </td>
              <td className="num">{fmt(bench.code_tokens)}</td>
              <td className="num" style={{ color: "var(--dim)" }}>
                {ratio(bench.code_tokens, bench.pack_tokens)}&times;
              </td>
            </tr>
          </tbody>
        </table>
      </div>
      <p className="note">
        <strong>What this does and does not say.</strong> It is not a claim that an agent reads
        all {fmt(bench.prose_tokens)} tokens on every turn — a good agent greps first. It is the
        size of the haystack it is grepping, versus the size of the answer when the answer was
        written down on purpose. The compiled context is also{" "}
        <em>all</em> of it: there is nothing else to go and find.
      </p>

      <h2>One real question</h2>
      <p>
        &ldquo;{bench.question}?&rdquo; — a question with a genuine answer in this codebase,
        asked both ways.
      </p>
      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Route</th>
              <th>Tokens</th>
              <th>Answers &ldquo;why&rdquo;</th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <td>
                <code>ctx search</code>, top hit
              </td>
              <td className="num">{bench.answer_tokens}</td>
              <td style={{ color: "var(--good)" }}>yes</td>
            </tr>
            <tr>
              <td>
                Read <code>{bench.answer_source}</code>
              </td>
              <td className="num">{fmt(bench.answer_source_tokens)}</td>
              <td style={{ color: "var(--warn)" }}>no — the code shows what, not why</td>
            </tr>
          </tbody>
        </table>
      </div>
      <p>
        {ratio(bench.answer_source_tokens, bench.answer_tokens)}&times; fewer tokens is the
        smaller half of the point. The file can tell you that the code calls{" "}
        <code>git pull --rebase</code>. It cannot tell you that fast-forward-only was tried and
        rejected, which is the thing that stops the next agent from switching it back.
      </p>

      <h2>Speed</h2>
      <p>
        Best of {9} runs, which is the honest floor: everything above it is scheduler noise.
        Process startup is measured separately and subtracted, since it is the cost of running
        any command at all, not the cost of the work.
      </p>
      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Operation</th>
              <th>Time</th>
              <th>Notes</th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <td>Compile the context</td>
              <td className="num">{bench.ms_pack} ms</td>
              <td className="wrap-cell">
                {bench.pack_claims} claims selected under a 700-token budget
              </td>
            </tr>
            <tr>
              <td>Search</td>
              <td className="num">{bench.ms_search} ms</td>
              <td className="wrap-cell">
                two FTS5 passes, reciprocal-rank fused, plus one hop of graph expansion
              </td>
            </tr>
            <tr>
              <td>Rebuild the whole index</td>
              <td className="num">{bench.ms_reindex} ms</td>
              <td className="wrap-cell">
                {bench.claims_indexed} records replayed from the log, from scratch
              </td>
            </tr>
            <tr>
              <td>Process startup</td>
              <td className="num">{bench.ms_startup} ms</td>
              <td className="wrap-cell">
                <code>ctx --version</code>: the floor for any single command on Windows
              </td>
            </tr>
          </tbody>
        </table>
      </div>
      <p>
        Startup dominates, which is the right problem to have: the work itself is single-digit
        milliseconds, and a long-lived MCP server pays the startup once rather than per call.
      </p>

      <h2>Size on disk</h2>
      <p>
        The entire store — every claim, every status change, every document version ever written
        — is {kb(bench.log_bytes)} of JSONL across {bench.claims_indexed} records. The SQLite
        index beside it is disposable and rebuilt in {bench.ms_reindex} ms, so it is not really
        storage at all.
      </p>

      <h2>Reproducing this</h2>
      <pre>
        <code>
          <span className="prompt">$ </span>git clone
          https://github.com/Percobain/RecurOS && cd RecurOS{"\n"}
          <span className="prompt">$ </span>cargo install --path crates/ctx-cli --locked{"\n"}
          <span className="prompt">$ </span>python scripts/bench.py
        </code>
      </pre>
      <p>
        Against your own store the absolute numbers will differ — that is the point of
        publishing the script rather than a screenshot.
      </p>
    </>
  );
}
