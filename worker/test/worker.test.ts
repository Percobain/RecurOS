import { afterEach, describe, expect, it, vi } from "vitest";
import { canonicalContent, cidOf } from "../src/canonical.js";
import worker, { type Env } from "../src/index.js";
import { base64ToUtf8 } from "../src/github.js";
import { buildClaim, parseLog, search, validBranch } from "../src/tools.js";
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
