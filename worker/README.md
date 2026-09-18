# ContextOS Worker — claude.ai & ChatGPT connector

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

## Tools

| Tool | What it does |
|---|---|
| `ctx_index()` | Overview of all projects and branches (`packs/index.md`) |
| `ctx_pack(branch, task?, budget?)` | The compiled pack for a branch; with `task`, adds matching claims |
| `ctx_search(query, branch?)` | Keyword search over all claims |
| `ctx_append(branch, kind, text, why?, refs?)` | Save a claim |
| `ctx_propose(target_branch, kind, text, why?)` | Propose a claim; review locally with `ctx review` |

`kind` is one of `fact`, `decision`, `rejected`, `constraint`, `question`, `claim`.

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
