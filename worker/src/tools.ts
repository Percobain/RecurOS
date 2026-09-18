// The five MCP tools (spec §9.1), backed by the GitHub copy of ~/ctx.
// Packs are precompiled by `ctx sync` into packs/, so the Worker never runs
// the packer; it serves files, does keyword search, and appends claims.

import { canonicalContent, cidOf, isBlank, isKind, KINDS, type Kind } from "./canonical.js";
import { appendLine, getBlob, getFile, listTree, type Env } from "./github.js";
import { ulid } from "./ulid.js";

export interface ToolDef {
  name: string;
  description: string;
  inputSchema: Record<string, unknown>;
}

const kindSchema = { type: "string", enum: [...KINDS] };
const branchDesc = 'Branch as "project/branch", e.g. "sovereign/research".';

export const TOOLS: ToolDef[] = [
  {
    name: "ctx_index",
    description: "Overview of all projects and branches in ContextOS. Call first.",
    inputSchema: { type: "object", properties: {} },
  },
  {
    name: "ctx_pack",
    description: "Compiled context for a branch. Optionally add claims relevant to a task.",
    inputSchema: {
      type: "object",
      properties: {
        branch: { type: "string", description: branchDesc },
        task: { type: "string" },
        budget: { type: "integer" },
      },
      required: ["branch"],
    },
  },
  {
    name: "ctx_search",
    description: "Keyword search over recorded claims.",
    inputSchema: {
      type: "object",
      properties: { query: { type: "string" }, branch: { type: "string", description: branchDesc } },
      required: ["query"],
    },
  },
  {
    name: "ctx_append",
    description: "Record a claim. Only when the user explicitly asks to save.",
    inputSchema: {
      type: "object",
      properties: {
        branch: { type: "string", description: branchDesc },
        kind: kindSchema,
        text: { type: "string" },
        why: { type: "string" },
        refs: { type: "array", items: { type: "string" } },
      },
      required: ["branch", "kind", "text"],
    },
  },
  {
    name: "ctx_propose",
    description: "Propose a claim for another branch; it awaits review.",
    inputSchema: {
      type: "object",
      properties: {
        target_branch: { type: "string", description: branchDesc },
        kind: kindSchema,
        text: { type: "string" },
        why: { type: "string" },
      },
      required: ["target_branch", "kind", "text"],
    },
  },
];

export class ToolError extends Error {}

/** Same rule as Rust `BranchRef::new`. */
export function validBranch(b: unknown): b is string {
  return (
    typeof b === "string" &&
    b.length > 0 &&
    !b.startsWith("/") &&
    !b.endsWith("/") &&
    !b.includes("//") &&
    /^[a-z0-9_/-]+$/.test(b)
  );
}

function str(args: Record<string, unknown>, key: string, required: boolean): string | undefined {
  const v = args[key];
  if (v === undefined || v === null) {
    if (required) throw new ToolError(`missing argument: ${key}`);
    return undefined;
  }
  if (typeof v !== "string") throw new ToolError(`${key} must be a string`);
  return v;
}

function branchArg(args: Record<string, unknown>, key: string, required: boolean): string | undefined {
  const b = str(args, key, required);
  if (b !== undefined && !validBranch(b)) {
    throw new ToolError(`invalid branch "${b}": use lowercase "project/branch"`);
  }
  return b;
}

// ---- claim construction (protocol §2) -------------------------------------

export interface ClaimRecord {
  rec: "claim";
  id: string;
  cid: string;
  kind: Kind;
  branch: string;
  text: string;
  why?: string;
  refs?: string[];
  entities?: string[];
  status: "active" | "proposed";
  confidence: "medium";
  src: string;
  t_valid: string;
  t_tx: string;
  tokens: number;
}

export function buildClaim(input: {
  branch: string;
  kind: string;
  text: string;
  why?: string;
  refs?: unknown;
  status: "active" | "proposed";
  now?: Date;
}): ClaimRecord {
  if (!isKind(input.kind)) throw new ToolError(`invalid kind "${input.kind}": one of ${KINDS.join(", ")}`);
  if (!validBranch(input.branch)) throw new ToolError(`invalid branch "${input.branch}"`);
  let refs: string[] = [];
  if (input.refs !== undefined) {
    if (!Array.isArray(input.refs) || !input.refs.every((r) => typeof r === "string")) {
      throw new ToolError("refs must be an array of strings");
    }
    refs = input.refs;
  }
  const content = canonicalContent(input.kind, input.text, input.why, refs, []);
  if (isBlank(content.text)) throw new ToolError("text is empty");
  const now = input.now ?? new Date();
  const ts = now.toISOString();
  // Key order mirrors the Rust serializer; optional fields omitted when empty.
  const rec: ClaimRecord = {
    rec: "claim",
    id: ulid(now.getTime()),
    cid: cidOf(content),
    kind: content.kind,
    branch: input.branch,
    text: content.text,
    ...(content.why !== null ? { why: content.why } : {}),
    ...(content.refs.length ? { refs: content.refs } : {}),
    status: input.status,
    confidence: "medium",
    src: "cloud",
    t_valid: ts,
    t_tx: ts,
    tokens: Math.ceil([...content.text].length / 4),
  };
  return rec;
}

function shardPath(now: Date): string {
  const m = String(now.getUTCMonth() + 1).padStart(2, "0");
  return `log/cloud/${now.getUTCFullYear()}-${m}.jsonl`;
}

// ---- reading the log ------------------------------------------------------

interface LoggedClaim {
  id: string;
  cid: string;
  kind: string;
  branch: string;
  text: string;
  why?: string;
  entities?: string[];
  status: string;
}

const STATUS_RANK: Record<string, number> = { proposed: 0, active: 1, superseded: 2, rejected: 3, archived: 4 };

function maxStatus(a: string, b: string): string {
  return (STATUS_RANK[b] ?? -1) > (STATUS_RANK[a] ?? -1) ? b : a;
}

/** All claims in the log, with effective status applied (protocol §2). */
export function parseLog(files: string[]): LoggedClaim[] {
  const claims = new Map<string, LoggedClaim>();
  const transitions: { claim: string; to: string }[] = [];
  for (const file of files) {
    for (const line of file.split("\n")) {
      if (!line.trim()) continue;
      let r: Record<string, unknown>;
      try {
        r = JSON.parse(line);
      } catch {
        continue; // torn or corrupt line: skip, never fail
      }
      if (r.rec === "claim" && typeof r.id === "string" && typeof r.text === "string") {
        claims.set(r.id, r as unknown as LoggedClaim);
        if (typeof r.supersedes === "string") transitions.push({ claim: r.supersedes, to: "superseded" });
      } else if (r.rec === "status" && typeof r.claim === "string" && typeof r.to === "string") {
        transitions.push({ claim: r.claim, to: r.to });
      }
    }
  }
  for (const t of transitions) {
    const c = claims.get(t.claim);
    if (c) c.status = maxStatus(c.status, t.to);
  }
  return [...claims.values()].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
}

async function readLog(env: Env): Promise<LoggedClaim[]> {
  const tree = await listTree(env);
  const shards = tree.filter((e) => e.type === "blob" && e.path.startsWith("log/") && e.path.endsWith(".jsonl"));
  const files: string[] = [];
  // Bounded concurrency: subrequest limits on the free plan.
  for (let i = 0; i < shards.length; i += 8) {
    files.push(...(await Promise.all(shards.slice(i, i + 8).map((s) => getBlob(env, s.sha)))));
  }
  return parseLog(files);
}

function words(s: string): string[] {
  return s.toLowerCase().split(/[^\p{L}\p{N}]+/u).filter(Boolean);
}

export function search(claims: LoggedClaim[], query: string, branch?: string, limit = 20): LoggedClaim[] {
  const q = [...new Set(words(query))];
  if (q.length === 0) return [];
  const scored: { c: LoggedClaim; score: number }[] = [];
  for (const c of claims) {
    if (branch && c.branch !== branch) continue;
    if (c.status === "archived") continue;
    const hay = new Set(words([c.text, c.why ?? "", ...(c.entities ?? [])].join(" ")));
    const score = q.filter((w) => hay.has(w)).length;
    if (score > 0) scored.push({ c, score });
  }
  // Best score first; ties newest first (ULIDs sort by time).
  scored.sort((a, b) => b.score - a.score || (a.c.id < b.c.id ? 1 : -1));
  return scored.slice(0, limit).map((s) => s.c);
}

export function renderClaims(claims: LoggedClaim[]): string {
  return claims
    .map((c) => {
      const why = c.why ? ` (${c.why.replace(/\n/g, " ")})` : "";
      const status = c.status !== "active" ? ` {${c.status}}` : "";
      const tag = c.cid.replace(/^b3:/, "").slice(0, 4);
      return `- [${c.kind}] ${c.text.replace(/\n/g, " ")}${why}${status} [c:${tag}]`;
    })
    .join("\n");
}

// ---- dispatch ------------------------------------------------------------

export async function callTool(env: Env, name: string, args: Record<string, unknown>): Promise<string> {
  switch (name) {
    case "ctx_index": {
      const f = await getFile(env, "packs/index.md");
      return f?.text ?? "No index yet. Run `ctx sync` on a machine with ContextOS to publish packs.";
    }
    case "ctx_pack": {
      const branch = branchArg(args, "branch", true)!;
      const task = str(args, "task", false);
      const f = await getFile(env, `packs/${branch}.md`);
      let out: string;
      if (f) {
        out = f.text;
      } else {
        const packs = (await listTree(env))
          .filter((e) => e.type === "blob" && e.path.startsWith("packs/") && e.path.endsWith(".md") && e.path !== "packs/index.md")
          .map((e) => e.path.slice("packs/".length, -".md".length));
        out =
          `No compiled pack for "${branch}". ` +
          (packs.length
            ? `Available: ${packs.join(", ")}.`
            : "No packs published yet: run `ctx sync` on a machine with ContextOS.");
      }
      if (task && task.trim()) {
        const hits = search(await readLog(env), task, branch);
        if (hits.length) out += `\n\n## Relevant to: ${task}\n\n${renderClaims(hits)}\n`;
      }
      return out;
    }
    case "ctx_search": {
      const query = str(args, "query", true)!;
      const branch = branchArg(args, "branch", false);
      const hits = search(await readLog(env), query, branch);
      return hits.length ? renderClaims(hits) : "No matching claims.";
    }
    case "ctx_append":
    case "ctx_propose": {
      const propose = name === "ctx_propose";
      const branch = branchArg(args, propose ? "target_branch" : "branch", true)!;
      const now = new Date();
      const rec = buildClaim({
        branch,
        kind: str(args, "kind", true)!,
        text: str(args, "text", true)!,
        why: str(args, "why", false),
        refs: propose ? undefined : args.refs,
        status: propose ? "proposed" : "active",
        now,
      });
      await appendLine(env, shardPath(now), JSON.stringify(rec), `ctx: ${propose ? "propose" : "append"} ${rec.kind} to ${branch} (cloud)`);
      const tag = rec.cid.slice(3, 7);
      return propose
        ? `Proposed ${rec.kind} for ${branch} [c:${tag}] (id ${rec.id}); it will appear after review with \`ctx review\`.`
        : `Saved ${rec.kind} to ${branch} [c:${tag}] (id ${rec.id}).`;
    }
    default:
      throw new ToolError(`unknown tool: ${name}`);
  }
}
