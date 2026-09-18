# ContextOS Worker: claude.ai & ChatGPT connector

A tiny, stateless [MCP](https://modelcontextprotocol.io) server on Cloudflare
Workers. It gives claude.ai and ChatGPT the same five `ctx_*` tools your local
agents have, reading and writing your ContextOS store through a **private
GitHub repo**. No extension needed, and it fits in the free tier.

```
claude.ai / ChatGPT ──MCP──▶ Worker ──GitHub API──▶ your private ctx repo ◀──git── your laptop (ctx sync)
```

- **Reads** compiled packs from `packs/` (published by `ctx sync` on your
  machine) and searches the log.
- **Writes** go to `log/cloud/<YYYY-MM>.jsonl`, a file only the Worker writes.
  Your laptop picks them up on its next `ctx sync` or session start.

## What you need

- ContextOS installed locally, with some claims saved (`ctx save ...`)
- A GitHub account
- A Cloudflare account (free)
- Node.js 18+

## Setup (about 10 minutes)

### 1. Put `~/ctx` in a private GitHub repo

Create an **empty, private** repo on GitHub (e.g. `ctx-store`), then:

```sh
git -C ~/ctx remote add origin git@github.com:<you>/ctx-store.git
ctx sync            # compiles packs/, commits and pushes
```

On Windows, `~/ctx` is `%USERPROFILE%\ctx`.

### 2. Create a fine-grained GitHub token

GitHub → Settings → Developer settings → Personal access tokens →
**Fine-grained tokens** → Generate new token:

- **Repository access:** Only select repositories → `ctx-store`
- **Permissions → Repository → Contents:** Read and write
- Nothing else.

Copy the token (`github_pat_...`).

### 3. Configure and deploy the Worker

```sh
cd worker
npm install
npx wrangler login
```

Edit `wrangler.toml` and set your repo:

```toml
[vars]
GITHUB_REPO = "<you>/ctx-store"
GITHUB_BRANCH = "main"
```

Set the two secrets:

```sh
npx wrangler secret put GITHUB_TOKEN     # paste the github_pat_... token
openssl rand -hex 24                     # generate a secret, copy it
npx wrangler secret put CTX_SECRET       # paste that secret
```

(No `openssl`? Any long random string works, e.g. from a password manager.
Use letters and digits only.)

Deploy:

```sh
npx wrangler deploy
```

Wrangler prints your Worker URL, e.g. `https://contextos.<you>.workers.dev`.

Your connector URL is:

```
https://contextos.<you>.workers.dev/mcp/<CTX_SECRET>
```

> **This URL is the password.** Anyone with it can read and write your
> context. Keep it private; to revoke it, run
> `npx wrangler secret put CTX_SECRET` with a new value.

### 4. Add it as a connector

**claude.ai:** Settings → Connectors → **Add custom connector** → paste the
URL → Add. In a chat, enable it from the tools menu.

**ChatGPT:** Settings → Apps & Connectors → Advanced → enable **Developer
mode**, then **Create** a connector with the URL (no authentication).

### 5. Try it

In a new chat:

> Use ctx_index, then load the pack for `myproject/research`.

> We decided to drop the covenant approach because confirmations are too slow.
> ctx save that as a rejected claim on myproject/research.

Back on your laptop, `ctx sync` then `ctx log` shows the claim with `src: cloud`.

## The idea-to-build flow

This is the path the Worker is built for: an idea starts in a chat and ends
in a coding agent, without copying anything by hand.

1. **Start the idea on your laptop:** `ctx new <idea>`, then `ctx sync`. This
   creates `<idea>/research` and `<idea>/code` and makes research the active
   branch. The Worker reads `refs/active`, so from now on chat tools save to
   `<idea>/research` without you naming a branch.
2. **Research in claude.ai or ChatGPT** with the connector enabled. Ask the
   model to `ctx save` decisions, facts, rejected ideas and open questions as
   they come up.
3. **Say "ctx spec"** when the research is done. The model writes the full
   spec and saves it with `ctx_append`: a one-line decision as the claim, and
   the complete markdown in `doc`. It lands in `log/cloud/` as a `doc` record
   plus a claim that refers to it (`doc:spec`).
4. **Build:** on your laptop, `ctx build <idea>` pulls the spec, creates a
   project folder with `SPEC.md`, `AGENTS.md` and the agent wiring, and then
   you run `claude` in it.

Any chat can read the spec back with `ctx_pack(doc: "spec")`.

## Tools

| Tool | What it does |
|---|---|
| `ctx_index()` | Overview of all projects and branches (`packs/index.md`), with the current branch |
| `ctx_pack(branch?, task?, budget?, doc?)` | The compiled pack for a branch; with `task`, adds matching claims; with `doc`, returns that document (e.g. `spec`) instead |
| `ctx_search(query, branch?)` | Keyword search over all claims |
| `ctx_append(kind, text, branch?, why?, refs?, doc?, doc_name?)` | Save a claim; with `doc`, also save a document (default name `spec`) in the same commit |
| `ctx_propose(target_branch, kind, text, why?)` | Propose a claim; review locally with `ctx review` |

`kind` is one of `fact`, `decision`, `rejected`, `constraint`, `question`, `claim`.
When `branch` is omitted, tools use the branch in `refs/active` (set on your
laptop with `ctx use` or `ctx new`), or `default` if there is none.

## Cost and limits

- **The Workers Free plan never bills you.** It allows 100,000 requests per
  day. Past that, requests simply fail until the daily reset; nothing is
  charged. Billing only exists if you move the account to Workers Paid.
- **This Worker caps itself below that anyway.** A Rate Limiting binding in
  `wrangler.toml` allows 62 requests per 60 seconds, which is at most
  62 x 1,440 = 89,280 requests per day. Over the cap, the connector returns
  HTTP 429 with a JSON-RPC error explaining the guard, and it recovers within
  a minute.
- **To change the cap**, edit `limit` (and `period`, which must be 10 or 60
  seconds) in the `[[ratelimits]]` block of `wrangler.toml` and redeploy.
  The counter is kept per Cloudflare location, so it is a close guard rather
  than an exact global count; one person's traffic comes from one location
  in practice.
- **It is bring-your-own-account by design.** Every user deploys this Worker
  to their own free Cloudflare account and points it at their own private
  GitHub repo. Nobody shares a quota, and there is no ContextOS server whose
  bill could grow.

## Troubleshooting

- **"No compiled pack" / "No index yet"**: run `ctx sync` on your machine. Packs
  are compiled locally and pushed. The Worker never runs the packer.
- **Tool calls fail with 401/403 from GitHub**: the token is expired or lacks
  *Contents: Read and write* on that exact repo.
- **404 on the connector URL**: the path secret doesn't match `CTX_SECRET`.
- **Local `ctx sync` fails after a cloud save**: it shouldn't. The Worker only
  ever writes `log/cloud/`, so there is nothing to conflict with. If it does,
  please file a bug.

## Development

```sh
npm test            # golden CID tests (same literals as the Rust suite) + router tests
npm run typecheck
npx wrangler dev    # local server; put secrets in .dev.vars
```

`src/canonical.ts` must stay byte-identical to
[`docs/canonical.md`](../docs/canonical.md). The golden tests enforce this.
