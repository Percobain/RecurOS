import { afterEach, describe, expect, it, vi } from "vitest";
import { canonicalContent, cidOf } from "../src/canonical.js";
import worker, { type Env } from "../src/index.js";
import { base64ToUtf8, utf8ToBase64 } from "../src/github.js";
import { buildClaim, buildDoc, parseDocs, parseLog, search, validBranch } from "../src/tools.js";
import { ulid } from "../src/ulid.js";

const env: Env = { GITHUB_TOKEN: "t", GITHUB_REPO: "me/ctx", GITHUB_BRANCH: "main", CTX_SECRET: "s3cret" };

function rpc(body: unknown, path = "/mcp/s3cret", method = "POST"): Promise<Response> {
  return worker.fetch(
    new Request(`https://w.example${path}`, {
      method,
      body: method === "POST" ? JSON.stringify(body) : undefined,
    }),
    env,
  );
}

afterEach(() => vi.unstubAllGlobals());

describe("ulid", () => {
  it("is 26 Crockford chars and monotonic within a millisecond", () => {
    const a = ulid(1_700_000_000_000);
    const b = ulid(1_700_000_000_000);
    expect(a).toMatch(/^[0-9A-HJKMNP-TV-Z]{26}$/);
    expect(b > a).toBe(true);
    expect(a.slice(0, 10)).toBe(b.slice(0, 10));
  });
});

describe("buildClaim", () => {
  it("builds a protocol §2 record whose cid verifies", () => {
    const rec = buildClaim({
      branch: "sovereign/research",
      kind: "decision",
      text: "Use a federation peg  \r\n",
      why: "covenants are slow",
      refs: ["b", "a"],
      status: "active",
      now: new Date("2026-09-18T12:00:00Z"),
    });
    expect(Object.keys(rec)).toEqual([
      "rec", "id", "cid", "kind", "branch", "text", "why", "refs",
      "status", "confidence", "src", "t_valid", "t_tx", "tokens",
    ]);
    expect(rec.text).toBe("Use a federation peg");
    expect(rec.refs).toEqual(["a", "b"]);
    expect(rec.src).toBe("cloud");
    expect(rec.tokens).toBe(5);
    expect(rec.cid).toBe(cidOf(canonicalContent("decision", rec.text, rec.why, rec.refs, [])));
  });

  it("rejects bad kind, branch and blank text", () => {
    const base = { branch: "p/b", kind: "fact", text: "x", status: "active" as const };
    expect(() => buildClaim({ ...base, kind: "memo" })).toThrow(/kind/);
    expect(() => buildClaim({ ...base, branch: "P/B" })).toThrow(/branch/);
    expect(() => buildClaim({ ...base, text: " \n " })).toThrow(/empty/);
  });

  it("branch validation matches Rust BranchRef", () => {
    for (const ok of ["sovereign/code", "x-marketing_2", "default"]) expect(validBranch(ok)).toBe(true);
    for (const bad of ["", "/a", "a/", "a//b", "Code", "a b", "a.b"]) expect(validBranch(bad)).toBe(false);
  });
});

describe("log parsing and search", () => {
  const log = [
    '{"rec":"claim","id":"01A","cid":"b3:aaaa","kind":"decision","branch":"p/r","text":"Settlement uses a peg","status":"active"}',
    '{"rec":"claim","id":"01B","cid":"b3:bbbb","kind":"fact","branch":"p/r","text":"Old settlement idea","status":"active"}',
    '{"rec":"status","id":"01C","claim":"01B","to":"archived","src":"cli","t_tx":"x"}',
    '{"rec":"claim","id":"01D","cid":"b3:dddd","kind":"fact","branch":"p/c","text":"settlement in code","status":"active"}',
    '{"rec":"claim","id":"01E", torn',
  ].join("\n");

  it("skips bad lines, applies status transitions, filters by branch", () => {
    const claims = parseLog([log]);
    expect(claims.map((c) => c.id)).toEqual(["01A", "01B", "01D"]);
    expect(claims[1]!.status).toBe("archived");
    expect(search(claims, "settlement").map((c) => c.id)).toEqual(["01D", "01A"]);
    expect(search(claims, "settlement", "p/r").map((c) => c.id)).toEqual(["01A"]);
  });
});

describe("router", () => {
  it("404s unknown paths and wrong secrets; 405s GET", async () => {
    expect((await rpc({}, "/mcp/wrong")).status).toBe(404);
    expect((await rpc({}, "/")).status).toBe(404);
    expect((await rpc(undefined, "/mcp/s3cret", "GET")).status).toBe(405);
  });

  it("initialize echoes protocol version", async () => {
    const res = await rpc({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2026-07-28" } });
    const body = (await res.json()) as { result: { protocolVersion: string; serverInfo: { name: string } } };
    expect(body.result.protocolVersion).toBe("2026-07-28");
    expect(body.result.serverInfo.name).toBe("contextos");
  });

  it("notifications get 202 with no body", async () => {
    const res = await rpc({ jsonrpc: "2.0", method: "notifications/initialized" });
    expect(res.status).toBe(202);
    expect(await res.text()).toBe("");
  });

  it("lists exactly five tools", async () => {
    const res = await rpc({ jsonrpc: "2.0", id: 2, method: "tools/list" });
    const body = (await res.json()) as { result: { tools: { name: string }[] } };
    expect(body.result.tools.map((t) => t.name)).toEqual([
      "ctx_index", "ctx_pack", "ctx_search", "ctx_append", "ctx_propose",
    ]);
  });

  it("ctx_append writes a claim line to log/cloud via the Contents API", async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    vi.stubGlobal("fetch", async (url: string, init?: RequestInit) => {
      calls.push({ url, init });
      if (!init?.method || init.method === "GET") return new Response("{}", { status: 404 });
      return new Response("{}", { status: 201 });
    });
    const res = await rpc({
      jsonrpc: "2.0",
      id: 3,
      method: "tools/call",
      params: { name: "ctx_append", arguments: { branch: "p/research", kind: "fact", text: "hello" } },
    });
    const body = (await res.json()) as { result: { content: { text: string }[]; isError?: boolean } };
    expect(body.result.isError).toBeUndefined();
    expect(body.result.content[0]!.text).toMatch(/^Saved fact to p\/research \[c:ef5c\]/);
    const put = calls.find((c) => c.init?.method === "PUT")!;
    expect(put.url).toMatch(/\/repos\/me\/ctx\/contents\/log\/cloud\/\d{4}-\d{2}\.jsonl$/);
    const sent = JSON.parse(put.init!.body as string) as { content: string; sha?: string };
    expect(sent.sha).toBeUndefined();
    const line = JSON.parse(base64ToUtf8(sent.content).trimEnd());
    expect(line.cid).toBe("b3:ef5c7ba58908aa7aa4d1cf7f2d3011d40beeede4d51537e621ec5ed9f3350d0f");
  });

  it("tool errors are reported in-band", async () => {
    const res = await rpc({
      jsonrpc: "2.0",
      id: 4,
      method: "tools/call",
      params: { name: "ctx_append", arguments: { branch: "p/r", kind: "memo", text: "x" } },
    });
    const body = (await res.json()) as { result: { isError: boolean; content: { text: string }[] } };
    expect(body.result.isError).toBe(true);
    expect(body.result.content[0]!.text).toMatch(/invalid kind/);
  });
});

// ---- a tiny in-memory GitHub -------------------------------------------------

/** Serves `files` through the Contents, Trees and Blobs APIs and records PUTs. */
function fakeGitHub(files: Record<string, string>) {
  const puts: { path: string; text: string }[] = [];
  const sha = (path: string) => `sha-${path}`;
  vi.stubGlobal("fetch", async (url: string, init?: RequestInit) => {
    const u = new URL(url);
    const contents = u.pathname.match(/^\/repos\/me\/ctx\/contents\/(.+)$/);
    const blob = u.pathname.match(/^\/repos\/me\/ctx\/git\/blobs\/(.+)$/);
    if (init?.method === "PUT" && contents) {
      const path = decodeURIComponent(contents[1]!);
      const body = JSON.parse(init.body as string) as { content: string };
      const text = base64ToUtf8(body.content);
      puts.push({ path, text });
      files[path] = text;
      return new Response("{}", { status: 201 });
    }
    if (contents) {
      const path = decodeURIComponent(contents[1]!);
      if (!(path in files)) return new Response("{}", { status: 404 });
      return Response.json({ sha: sha(path), encoding: "base64", content: utf8ToBase64(files[path]!) });
    }
    if (u.pathname.startsWith("/repos/me/ctx/git/trees/")) {
      return Response.json({ tree: Object.keys(files).map((p) => ({ path: p, type: "blob", sha: sha(p) })) });
    }
    if (blob) {
      const path = decodeURIComponent(blob[1]!).slice("sha-".length);
      return Response.json({ content: utf8ToBase64(files[path] ?? "") });
    }
    return new Response("{}", { status: 404 });
  });
  return puts;
}

async function call(name: string, args: Record<string, unknown>, e: Env = env) {
  const res = await worker.fetch(
    new Request("https://w.example/mcp/s3cret", {
      method: "POST",
      body: JSON.stringify({ jsonrpc: "2.0", id: 9, method: "tools/call", params: { name, arguments: args } }),
    }),
    e,
  );
  const body = (await res.json()) as { result: { content: { text: string }[]; isError?: boolean } };
  return { status: res.status, text: body.result.content[0]!.text, isError: body.result.isError };
}

const SPEC = "# Tiny Todo\n\nA todo app.\n\n```sh\nnpm start\n```\n";

describe("documents", () => {
  it("buildDoc normalises, derives the title and verifies against the golden cid", () => {
    const d = buildDoc({ branch: "p/research", body: "# Spec  \r\n\r\nBuild it.\r\n", now: new Date("2026-09-18T12:00:00Z") });
    expect(Object.keys(d)).toEqual(["rec", "id", "branch", "name", "title", "body", "cid", "src", "t_tx"]);
    expect(d.name).toBe("spec");
    expect(d.title).toBe("Spec");
    expect(d.body).toBe("# Spec\n\nBuild it.");
    expect(d.cid).toBe("b3:85dc6e887b400e1986ea0a09195d1913f4528a3a83fd66339d96b6716543b9c5");
    expect(() => buildDoc({ branch: "p/r", name: "My Spec", body: "x" })).toThrow(/doc_name/);
    expect(() => buildDoc({ branch: "p/r", body: " \n " })).toThrow(/empty/);
  });

  it("parseDocs keeps the newest version per (branch, name) and drops bad cids", () => {
    const a = buildDoc({ branch: "p/research", body: "# v1", now: new Date(1_000) });
    const b = buildDoc({ branch: "p/research", body: "# v2", now: new Date(2_000) });
    const bad = { ...buildDoc({ branch: "p/research", name: "notes", body: "x" }), body: "tampered" };
    const docs = parseDocs([[a, b, bad].map((r) => JSON.stringify(r)).join("\n")]);
    expect(docs.map((d) => d.title)).toEqual(["v2"]);
  });

  it("ctx_append with doc writes the doc and the claim in one PUT, claim refs doc:spec", async () => {
    const puts = fakeGitHub({ "refs/active": "tiny/research\n" });
    const r = await call("ctx_append", { kind: "decision", text: "Build Tiny Todo per the spec", doc: SPEC });
    expect(r.isError).toBeUndefined();
    expect(r.text).toMatch(/Saved decision to tiny\/research/);
    expect(r.text).toMatch(/Saved document "spec" \("Tiny Todo"\)/);
    expect(puts).toHaveLength(1);
    expect(puts[0]!.path).toMatch(/^log\/cloud\/\d{4}-\d{2}\.jsonl$/);
    const lines = puts[0]!.text.trimEnd().split("\n").map((l) => JSON.parse(l));
    expect(lines.map((l) => l.rec)).toEqual(["doc", "claim"]);
    expect(lines[0].branch).toBe("tiny/research");
    expect(lines[0].body).toBe("# Tiny Todo\n\nA todo app.\n\n```sh\nnpm start\n```");
    expect(lines[1].refs).toEqual(["doc:spec"]);
    expect(lines[0].id < lines[1].id).toBe(true);
  });

  it("an identical doc is not written twice, but the claim still is", async () => {
    const files: Record<string, string> = {};
    const puts = fakeGitHub(files);
    await call("ctx_append", { branch: "tiny/research", kind: "decision", text: "Spec v1", doc: SPEC });
    const r = await call("ctx_append", { branch: "tiny/research", kind: "decision", text: "Spec v1 again", doc: SPEC });
    expect(r.text).toMatch(/unchanged, so it was not saved again/);
    expect(puts).toHaveLength(2);
    const second = puts[1]!.text.trimEnd().split("\n").map((l) => JSON.parse(l));
    // The second PUT holds the whole file: doc, claim, then only the new claim.
    expect(second.map((l) => l.rec)).toEqual(["doc", "claim", "claim"]);
  });

  it("ctx_pack doc reads the spec, falling back to another branch of the project", async () => {
    const doc = buildDoc({ branch: "tiny/research", body: SPEC });
    fakeGitHub({ "log/cloud/2026-09.jsonl": JSON.stringify(doc) + "\n", "refs/active": "tiny/code" });
    const r = await call("ctx_pack", { doc: "spec" });
    expect(r.text.startsWith("# Tiny Todo\n\n# Tiny Todo\n\nA todo app.")).toBe(true);
    const missing = await call("ctx_pack", { branch: "tiny/code", doc: "design" });
    expect(missing.text).toMatch(/No document named "design".*Available: spec\./);
  });
});

describe("active branch", () => {
  it("tools default to refs/active and ctx_index reports it", async () => {
    const puts = fakeGitHub({
      "refs/active": "idea/research\n",
      "packs/index.md": "# ContextOS index\n\nCurrent branch: `default`\n",
    });
    const idx = await call("ctx_index", {});
    expect(idx.text).toMatch(/Current branch: `idea\/research`/);
    expect(idx.text).not.toMatch(/`default`/);
    await call("ctx_append", { kind: "fact", text: "Users want offline mode" });
    expect(JSON.parse(puts[0]!.text.trimEnd()).branch).toBe("idea/research");
  });

  it("falls back to default without refs/active", async () => {
    const puts = fakeGitHub({});
    await call("ctx_append", { kind: "fact", text: "x" });
    expect(JSON.parse(puts[0]!.text.trimEnd()).branch).toBe("default");
  });
});

describe("cross-chat continuity (no laptop involved)", () => {
  it("an idea started in one chat is visible and continuable in another", async () => {
    const files: Record<string, string> = {};
    fakeGitHub(files);

    // ChatGPT: the user starts an idea by name; a bare name means <idea>/research.
    const saved = await call("ctx_append", {
      branch: "Notes App",
      kind: "decision",
      text: "Offline-first, sync later",
      why: "people take notes on the subway",
    });
    expect(saved.text).toMatch(/Saved decision to notes-app\/research/);
    await call("ctx_append", { branch: "notes-app", kind: "rejected", text: "Real-time collaboration in v1" });

    // claude.ai: the index is built from the log, so the new idea is listed
    // even though no laptop has compiled a pack for it.
    const idx = await call("ctx_index", {});
    expect(idx.text).toMatch(/`notes-app\/research`: 2 claims/);
    expect(idx.text).toMatch(/To start a new idea/);

    // ...and continuing it returns a live dossier with the chat protocols.
    const pack = await call("ctx_pack", { branch: "notes-app" });
    expect(pack.text).toMatch(/^# Context dossier: notes-app\/research/);
    expect(pack.text).toMatch(/## Thesis\n\n- Offline-first, sync later \[c:[0-9a-f]{4}\]\n  - why: people take notes/);
    expect(pack.text).toMatch(/## Contradictions and rejected paths\n\n- Real-time collaboration in v1/);
    expect(pack.text).toMatch(/When I say "ctx spec"/);

    // claude.ai saves the spec; the dossier then lists it as a document.
    await call("ctx_append", { branch: "notes-app", kind: "decision", text: "Build v1 per the spec", doc: "# Notes v1\n\nNotes and todos." });
    const withSpec = await call("ctx_pack", { branch: "notes-app" });
    expect(withSpec.text).toMatch(/## Documents\n\n- `spec`: Notes v1/);
    expect((await call("ctx_pack", { branch: "notes-app", doc: "spec" })).text).toMatch(/^# Notes v1/);
  });

  it("a removed idea disappears from the index, including its spec", async () => {
    const claim = buildClaim({ branch: "gone/research", kind: "fact", text: "x", status: "active" });
    const doc = buildDoc({ branch: "gone/research", body: "# Gone spec" });
    const status = (target: string) =>
      JSON.stringify({ rec: "status", id: ulid(), claim: target, to: "archived", src: "cli", t_tx: "2026-09-19T00:00:00Z" });
    fakeGitHub({
      "log/m/2026-09.jsonl": [JSON.stringify(claim), JSON.stringify(doc), status(claim.id), status(doc.id)].join("\n") + "\n",
    });
    const idx = await call("ctx_index", {});
    expect(idx.text).not.toMatch(/gone/);
    expect((await call("ctx_pack", { branch: "gone", doc: "spec" })).text).toMatch(/No document named "spec"/);
  });

  it("connected chats are told to save specs with the tool, not to print them", async () => {
    // The tail the laptop's packer writes for chats without a connector.
    const laptopProtocol =
      '---\nWhen I say "ctx save", reply with only one fenced code block tagged `ctx-claims` containing a JSON array. Nothing else.\n' +
      'When I say "ctx spec", write the complete spec inside one fenced block opened with four backticks and the tag ctx-spec. Nothing else.\n';
    fakeGitHub({
      "packs/idea/research.md": `# Context dossier: idea/research\n\n- x [c:0000]\n\n${laptopProtocol}<!-- ctx/1 b=idea/research -->\n`,
    });
    const r = await call("ctx_pack", { branch: "idea/research" });
    expect(r.text).not.toMatch(/fenced block opened with four backticks/);
    expect(r.text).toMatch(/SAVE it: call ctx_append with kind "decision".*full markdown in doc\. Do not just print it/);
    expect(r.text).toMatch(/<!-- ctx\/1 b=idea\/research -->/);
  });

  it("a compiled pack gets claims saved from chats since it was compiled", async () => {
    const old = buildClaim({ branch: "idea/research", kind: "fact", text: "Already compiled", status: "active" });
    const fresh = buildClaim({ branch: "idea/research", kind: "fact", text: "Saved from a chat later", status: "active" });
    const oldTag = old.cid.slice(3, 7);
    fakeGitHub({
      "packs/idea/research.md": `# Context dossier: idea/research\n\n- Already compiled [c:${oldTag}]\n`,
      "log/m/2026-09.jsonl": JSON.stringify(old) + "\n",
      "log/cloud/2026-09.jsonl": JSON.stringify(fresh) + "\n",
    });
    const r = await call("ctx_pack", { branch: "idea/research" });
    expect(r.text).toMatch(/## Saved since this pack was compiled\n\n- Saved from a chat later/);
    expect(r.text.match(/Already compiled/g)).toHaveLength(1);
  });
});

describe("rate limit guard", () => {
  it("returns 429 with a JSON-RPC error when the limiter refuses, before touching GitHub", async () => {
    const fetchSpy = vi.fn();
    vi.stubGlobal("fetch", fetchSpy);
    const limited: Env = { ...env, LIMITER: { limit: async () => ({ success: false }) } };
    const res = await worker.fetch(
      new Request("https://w.example/mcp/s3cret", {
        method: "POST",
        body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list" }),
      }),
      limited,
    );
    expect(res.status).toBe(429);
    const body = (await res.json()) as { error: { code: number; message: string } };
    expect(body.error.code).toBe(-32000);
    expect(body.error.message).toMatch(/62 requests\/minute/);
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("passes through when the limiter allows, and keys globally", async () => {
    const keys: string[] = [];
    const ok: Env = { ...env, LIMITER: { limit: async ({ key }) => (keys.push(key), { success: true }) } };
    const res = await worker.fetch(
      new Request("https://w.example/mcp/s3cret", {
        method: "POST",
        body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "ping" }),
      }),
      ok,
    );
    expect(res.status).toBe(200);
    expect(keys).toEqual(["global"]);
  });
});
