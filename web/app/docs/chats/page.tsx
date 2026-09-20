export const metadata = { title: "ChatGPT & claude.ai" };

export default function Chats() {
  return (
    <>
      <h1>ChatGPT and claude.ai</h1>
      <p className="lede">
        Optional, and one-time. Claude Code, Cursor and Codex work with no cloud at all, because
        they can run <code>ctx</code> themselves. Browser chats cannot, so they need a mailbox:
        a small Cloudflare Worker you deploy to your own account, reading the same private
        GitHub repository your store syncs to.
      </p>

      <h2>Why a Worker and not a service</h2>
      <p>
        Because there is no RecurOS company to host one, and you should not have to trust one.
        The Worker is stateless: it holds no data, it reads and writes the GitHub repository you
        point it at, and the only secret it has is the one you set. If it disappears, your
        context is still on your laptop and in your own git history.
      </p>

      <h2>Step 1: a private repo for the store</h2>
      <p>
        Create an empty <strong>private</strong> repository on GitHub, then point your store at
        it:
      </p>
      <pre>
        <code>
          <span className="prompt">$ </span>git -C ~/ctx remote add origin
          git@github.com:you/ctx-store.git{"\n"}
          <span className="prompt">$ </span>ctx sync
        </code>
      </pre>

      <h2>Step 2: a token the Worker can use</h2>
      <p>
        A GitHub fine-grained personal access token, scoped to <em>only</em> that one repository,
        with <strong>Contents: read and write</strong>. Nothing else.
      </p>

      <h2>Step 3: deploy the Worker</h2>
      <pre>
        <code>
          <span className="prompt">$ </span>cd worker{"\n"}
          <span className="prompt">$ </span>npx wrangler deploy{"\n"}
          <span className="prompt">$ </span>npx wrangler secret put GITHUB_TOKEN{"\n"}
          <span className="prompt">$ </span>npx wrangler secret put CTX_SECRET
        </code>
      </pre>
      <p>
        Deploy first, then set the secrets: a Worker has to exist before it can have any. Edit{" "}
        <code>GITHUB_REPO</code> in <code>wrangler.toml</code> to your repo. The{" "}
        <code>CTX_SECRET</code> is a long random string that becomes part of your connector URL,
        which is what keeps it yours.
      </p>
      <p className="note">
        <strong>It will not bill you.</strong> The config caps the Worker at 62 requests per 60
        seconds, so at most 89,280 a day, under Cloudflare&rsquo;s free tier of 100,000. Over
        the cap, requests fail rather than charge.
      </p>

      <h2>Step 4: add the connector</h2>
      <p>
        Your URL is <code>https://recuros.&lt;you&gt;.workers.dev/mcp/&lt;CTX_SECRET&gt;</code>.
      </p>
      <ul>
        <li>
          <strong>claude.ai</strong>: Settings &rarr; Connectors &rarr; Add custom connector.
          Name it RecurOS, paste the URL. Turn it on with the tools icon in a new chat.
        </li>
        <li>
          <strong>ChatGPT</strong>: Settings &rarr; Apps &amp; Connectors (developer mode) &rarr;
          Create. Same URL, authentication <em>None</em>, since the secret is already in the path.
        </li>
      </ul>

      <h2>What the chats can do</h2>
      <p>
        Exactly five tools, kept under 450 tokens of schema in total so they cost almost nothing
        to have switched on:
      </p>
      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Tool</th>
              <th>What it does</th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <td className="num">ctx_index</td>
              <td className="wrap-cell">The ~250-token overview: which projects exist.</td>
            </tr>
            <tr>
              <td className="num">ctx_pack</td>
              <td className="wrap-cell">
                Compiled context for one project, or a document such as a spec.
              </td>
            </tr>
            <tr>
              <td className="num">ctx_search</td>
              <td className="wrap-cell">Find claims by words, files or tags.</td>
            </tr>
            <tr>
              <td className="num">ctx_append</td>
              <td className="wrap-cell">Record a claim. Only when you ask for it.</td>
            </tr>
            <tr>
              <td className="num">ctx_propose</td>
              <td className="wrap-cell">
                Suggest a claim for another project, for you to accept or reject.
              </td>
            </tr>
          </tbody>
        </table>
      </div>

      <h2>The flow this is for</h2>
      <ol>
        <li>Have an idea in ChatGPT. Say &ldquo;save this idea to RecurOS.&rdquo;</li>
        <li>
          Research it in claude.ai the next day, from a different machine. It already has the
          idea.
        </li>
        <li>Ask for a spec. Say &ldquo;save that spec.&rdquo;</li>
        <li>
          On your laptop: <code>ctx build my-idea</code>. A repo appears with{" "}
          <code>.ctx/SPEC.md</code>, <code>AGENTS.md</code> and your coding agent already wired
          to the same context.
        </li>
      </ol>
    </>
  );
}
