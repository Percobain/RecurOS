# ContextOS browser extension

Chrome (MV3) extension for chat surfaces that can't use a custom MCP connector:
Gemini, free-tier ChatGPT, Perplexity, DeepSeek, and claude.ai/ChatGPT if you
don't want to use the Worker.

It does three things:

- **ctx → browser:** inserts a compiled context pack (plain markdown) into the
  chat composer.
- **browser → ctx (claims):** puts a **Save to ctx** button under any
  ```` ```ctx-claims ```` code block a model writes, and saves those claims to
  your local store.
- **browser → ctx (spec):** puts a **Save spec to ctx** button under any
  ````` ````ctx-spec ````` block, and saves it as the branch's spec document.

It talks only to `ctx daemon` on `127.0.0.1`. Nothing is sent anywhere else.

## Install

1. Start the local daemon (leave it running):
   ```sh
   ctx daemon
   ```
2. Open `chrome://extensions`, turn on **Developer mode**, click
   **Load unpacked**, and select this `extension/` folder.
3. Print your token:
   ```sh
   ctx daemon token
   ```
   Open the extension's **Options**, paste the token, and click
   **Test connection**. You should see `Connected` and your branches.

## From idea to code

This is the flow the extension is built for:

1. **Start the idea.** `ctx new my-idea` creates the project and makes
   `my-idea/research` the active branch, so everything you save from chats
   lands there.
2. **Research in any chat.** Click the toolbar icon and **Insert context**
   so the model knows what you already decided. When something is worth
   keeping, type `ctx save` and click **Save to ctx** under the block the
   model writes.
3. **Write the spec.** When the research is done, type `ctx spec`. The model
   writes the complete spec in one block fenced with four backticks and the
   tag `ctx-spec` (four, so code examples inside the spec survive). Click
   **Save spec to ctx**. Saving again after edits stores a new version;
   saving the same text twice shows `Spec unchanged (already saved)`.
4. **Build it.** In a terminal:
   ```sh
   ctx build my-idea
   cd my-idea && claude
   ```
   `ctx build` creates the repo, writes the spec to `SPEC.md`, puts the
   research conclusions in `AGENTS.md`, and wires your coding agents.

The popup shows the selected branch's documents under the branch picker
(for example `spec: My Idea`), so you can see whether a spec was saved.

## Use

**Give a chat your context.** Click the ContextOS toolbar icon, pick a branch,
optionally type a task (this narrows the pack to what's relevant), and click
**Insert context**. Or press **Alt+Shift+C** in the composer to insert the pack
for your default branch. **Copy** puts the pack on the clipboard instead.

**Save what you learned.** The inserted pack ends with an instruction to the
model. When you type `ctx save`, the model answers with one block like this:

````
```ctx-claims
[{"kind":"decision","text":"Use a federation peg for settlement","why":"covenants confirm too slowly"}]
```
````

Click **Save to ctx** under the block. The button shows what happened, for
example `Saved 2 · 1 duplicate`. Nothing is saved until you click.

Kinds: `fact decision rejected constraint question claim`.

**Save a spec.** When you type `ctx spec`, the model answers with the whole
spec as markdown inside a `ctx-spec` block. Click **Save spec to ctx**. The
button reads the block when you click, so wait until the answer has finished
streaming.

## When the extension breaks

Chat sites change their layouts all the time. The clipboard does the same job
with no extension at all, and it's the real protocol:

```sh
ctx pack --for dossier --clip    # copy a pack, paste it into any chat
ctx save --paste                 # copy the model's ctx-claims block, then run this
ctx spec save --paste            # copy the model's ctx-spec block, then run this
```

If the extension breaks, you lose two keystrokes, not your workflow.

## Troubleshooting

| Message | Fix |
|---|---|
| `Cannot reach ctxd … is ctxd running?` | Run `ctx daemon`. |
| `ctxd rejected the token` | Run `ctx daemon token` and paste the token again in Options. |
| `Could not find a chat composer` | Click into the message box first, or use **Copy**. |
| No **Save to ctx** button | The block has to be a JSON array of objects with `kind` and `text`. Wait for the answer to finish streaming. |
| No **Save spec to ctx** button | The block has to be fenced with the language tag `ctx-spec`. Ask the model to use exactly that tag. |
| `ctxd returned 404` when saving a spec | Your `ctx` is older than the extension. Update it. |

## How it works

- The content script never calls localhost. Chat sites' CSP blocks that, and
  the error looks like CORS but isn't. It messages the background service
  worker, which makes the HTTP call with your token.
- The service worker keeps no state. MV3 kills it after about 30s idle, so
  every request reads its settings from `chrome.storage.local` again.
- There's no native messaging, so no registry keys or per-browser manifests.
- Capture only looks at code blocks, because markdown fences survive UI
  redesigns and chat DOM selectors don't. The extension never reads your
  conversation.
- Specs go to `POST /v1/docs` as plain markdown (never base64), claims to
  `POST /v1/claims`. A `ctx-spec` block is never parsed as claims.

Icons are generated by `node tools/make-icons.js`.
