import Link from "next/link";

export const metadata = { title: "Existing projects" };

export default function ExistingProject() {
  return (
    <>
      <h1>Filling a project that already exists</h1>
      <p className="lede">
        A new project starts empty and fills as you work. A project with two years of history
        does not — and re-typing two years of decisions is not something anybody will do.
      </p>

      <h2>The one command</h2>
      <pre>
        <code>
          <span className="prompt">$ </span>cd my-existing-project{"\n"}
          <span className="prompt">$ </span>ctx init{"\n"}
          <span className="prompt">$ </span>ctx onboard
        </code>
      </pre>
      <p>
        <code>ctx onboard</code> prints a prompt. Paste it into Claude Code, Cursor, Codex or
        Gemini CLI <em>in that same folder</em>. Add <code>--clip</code> to copy it instead.
      </p>

      <h2>Why a prompt, and not a reader</h2>
      <p>
        RecurOS could walk your repository itself and generate claims from it. It deliberately
        does not. Two reasons:
      </p>
      <ul>
        <li>
          The agent is already there, with the whole file tree, a language model, and your
          question in front of it. Shipping a worse version of that inside a Rust binary would
          be strictly worse and permanently out of date.
        </li>
        <li>
          Judgement is the whole job. The value of a claim is not that it is true, it is that
          someone decided it was worth keeping. A generated store reads like a summary, and
          nobody trusts a summary they did not ask for.
        </li>
      </ul>

      <h2>What the prompt asks for</h2>
      <p>
        It names the entry points that actually exist in your repo — the README, the build
        manifests, <code>docs/</code> — and then it is specific about what to keep:
      </p>
      <div className="note">
        Record the things the code cannot say for itself: why this database and not another,
        what must never break, what was tried and abandoned, which conventions are deliberate.
        Skip anything a reader can see by opening the file.
      </div>
      <p>And specific about what to refuse:</p>
      <ul>
        <li>One claim per fact, phrased so it is still true next month.</li>
        <li>No progress notes, task lists, or summaries of what the agent did.</li>
        <li>
          No guessing. Ten claims you are sure of beat thirty you inferred — a store you do not
          trust is worse than no store, because you have to check it anyway.
        </li>
      </ul>

      <h2>Then read it back</h2>
      <pre>
        <code>
          <span className="prompt">$ </span>ctx pack{"      "}
          <span className="cmt"># exactly what every AI will now be told</span>
          {"\n"}
          <span className="prompt">$ </span>ctx log -v{"   "}
          <span className="cmt"># each claim with its reason</span>
          {"\n"}
          <span className="prompt">$ </span>ctx delete c:xxxx{"  "}
          <span className="cmt"># anything you disagree with</span>
        </code>
      </pre>
      <p>
        This review step is the point. It takes a few minutes, it is the only time you will do
        it, and what survives is a context you actually endorse.
      </p>

      <h2>Bringing in a project already under another name</h2>
      <p>
        If you already have context under a different project name, <code>ctx rename</code>{" "}
        carries all of it over — every claim with its reason, references, tags, confidence,
        original date and its place in the supersession chain, plus the current version of every
        document.
      </p>
      <pre>
        <code>
          <span className="prompt">$ </span>ctx rename old-name new-name{"\n"}
          <span className="ok">
            renamed old-name to new-name: 63 claims, 1 document across 2 branches
          </span>
        </code>
      </pre>
      <p>
        A claim records the branch it was made on, so a claim cannot change branch: renaming
        means re-recording each one under the new name and retiring the old. The old name stays
        readable in <code>ctx log --all</code>.
      </p>

      <p style={{ marginTop: 40 }}>
        Next: <Link href="/docs/chats">connect ChatGPT and claude.ai</Link>.
      </p>
    </>
  );
}
