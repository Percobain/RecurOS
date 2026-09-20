import Link from "next/link";
import { bench, fmt } from "@/lib/numbers";

export const metadata = { title: "How it works" };

export default function HowItWorks() {
  return (
    <>
      <h1>How it works</h1>
      <p className="lede">
        Four pieces: a record that cannot change, a log that cannot conflict, an index that can
        be thrown away, and a compiler that decides what fits.
      </p>

      <h2>The claim</h2>
      <p>
        Everything in RecurOS is one record type. A claim has a kind (<code>decision</code>,{" "}
        <code>constraint</code>, <code>rejected</code>, <code>fact</code>, <code>question</code>,{" "}
        <code>claim</code>), the sentence itself, the reason it holds, the files it is about,
        tags, a confidence, and the branch it was made on.
      </p>
      <p>
        There are exactly six kinds and there will not be a seventh. Every kind multiplies the
        renderer and the weighting logic, so new distinctions go in tags instead.
      </p>
      <p>
        A claim is immutable. You do not edit a decision; you record the new one as{" "}
        <code>--supersedes</code> the old, and the old one becomes history rather than a lie.
        That is also why &ldquo;delete&rdquo; is a status flip: two machines can both flip a
        status and still agree, but they cannot both rewrite a line.
      </p>

      <h2>Content addressing</h2>
      <p>
        A claim&rsquo;s id is BLAKE3 over its canonical form: RFC 8785 JSON, NFC-normalised
        text, an explicit whitespace table, keys sorted by code point. The same claim written on
        two machines produces the same id, byte for byte, which is what makes merging a
        non-event.
      </p>
      <p>
        This is the one place a subtle bug corrupts data instead of crashing, so the canonical
        form is frozen and its test vectors are asserted as literals produced by an independent
        Python implementation, not by the Rust code under test.
      </p>

      <h2>The log</h2>
      <p>
        An append-only JSONL file per machine per month, inside a git repository you own. One
        writer per file, locked and fsynced, torn last lines tolerated. Two machines never touch
        the same file, so a pull is a rebase that cannot conflict.
      </p>
      <p>
        Pulls use <code>git pull --rebase</code>, never fast-forward-only: your local records
        are new lines in your own file, and rebasing them on top of someone else&rsquo;s is
        always correct. Fast-forward-only would simply refuse.
      </p>

      <h2>The index</h2>
      <p>
        SQLite with FTS5, treated as a disposable cache: every row is recoverable from the log,
        so a schema change drops everything and re-ingests. Rebuilding {bench.claims_indexed}{" "}
        claims takes {bench.ms_reindex} ms, and the whole log on disk is{" "}
        {Math.round(bench.log_bytes / 1024)} KB.
      </p>
      <p>
        Retrieval is hybrid. One pass demands every meaningful word, one accepts any of them,
        and the two rankings are fused by reciprocal rank. The strict pass alone returns nothing
        the moment a word is missing; the loose pass alone lets a claim sharing one common word
        outrank the claim that answers the question. Then it expands one hop out from the best
        hits, following supersession and shared file references. A decision and the constraint
        it satisfies often share no vocabulary at all, only a path.
      </p>

      <h2>The compiler</h2>
      <p>
        Given a branch, an optional task, and a token budget, a cost-aware lazy-greedy pass
        maximises a submodular objective:
      </p>
      <pre>
        <code>
          F(S) = α · Σ rel(c,q) · w(c){"        "}
          <span className="cmt">relevance × weight</span>
          {"\n"}
          {"     "}+ β · Σ max cov(c,e){"        "}
          <span className="cmt">coverage of the topics</span>
          {"\n"}
          {"     "}− γ · Σ sim(c,c&apos;){"         "}
          <span className="cmt">redundancy</span>
        </code>
      </pre>
      <p>
        <code>w(c)</code> multiplies kind, confidence, recency (constraints never decay),
        usefulness votes and status. Relevance halves every eight retrieval ranks, and coverage
        is scaled by relevance too, because otherwise a claim carrying five tags outscores what the
        relevance term can award, and asking a question changes nothing about the answer.
      </p>
      <p>
        The output is deterministic and the budget is a hard guarantee: if even the fixed text
        does not fit, it emits a compact form rather than overflowing. Compiling this
        repository&rsquo;s {bench.pack_claims} claims takes {bench.ms_pack} ms and produces{" "}
        {fmt(bench.pack_tokens)} tokens.
      </p>

      <h2>Branches</h2>
      <p>
        Context is organised as <code>project/lane</code>, usually{" "}
        <code>project/research</code> and <code>project/code</code>. Research holds everything
        loose: ideas, open questions, beliefs. Code inherits only the decisions, constraints and
        rejected options, so the noise of exploring never reaches your coding agent.
      </p>
      <p>
        Inheritance narrows at each hop and lateral edges are non-transitive, which keeps a
        project from quietly absorbing another&rsquo;s context. The graph is checked for cycles
        on every load, because an undetected cycle is an infinite compile.
      </p>

      <p style={{ marginTop: 40 }}>
        Next: <Link href="/docs/decisions">what we used, and why</Link>.
      </p>
    </>
  );
}
