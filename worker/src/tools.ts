// The five MCP tools (spec §9.1), backed by the GitHub copy of ~/ctx.
// Packs are precompiled by `ctx sync` into packs/, so the Worker never runs
// the packer; it serves files, does keyword search, and appends records.

import { canonicalContent, cidOf, docCid, isBlank, isKind, KINDS, normalizeText, type Kind } from "./canonical.js";
import { appendLines, getBlob, getFile, listTree, type Env } from "./github.js";
import { ulid } from "./ulid.js";

export interface ToolDef {
  name: string;
  description: string;
  inputSchema: Record<string, unknown>;
}

const kindSchema = { type: "string", enum: [...KINDS] };
const branchDesc =
  'The idea this belongs to, as a short lowercase name, e.g. "notes-app" (means notes-app/research). ' +
  'If the user names an idea ("an idea called X", "new idea X", "save this as X"), that name IS the branch: ' +
  "never put a new idea into an existing one. Omit only to continue the current idea.";

export const TOOLS: ToolDef[] = [
  {
    name: "ctx_index",
    description: "Overview of all ideas/projects and what is saved for each. Call first.",
    inputSchema: { type: "object", properties: {} },
  },
  {
    name: "ctx_pack",
    description:
      "Compiled context for a branch. Optionally add claims relevant to a task, or pass doc (e.g. \"spec\") to read a saved document.",
    inputSchema: {
      type: "object",
      properties: {
        branch: { type: "string", description: branchDesc },
        task: { type: "string" },
        budget: { type: "integer" },
        doc: { type: "string", description: 'Document name to read, e.g. "spec".' },
      },
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
    description:
      "Record a claim. Only when the user explicitly asks to save. To save a spec or long document, " +
      'use kind "decision", a one-line summary as text, and the full markdown in doc.',
    inputSchema: {
      type: "object",
      properties: {
        branch: { type: "string", description: branchDesc },
        kind: kindSchema,
        text: { type: "string" },
        why: { type: "string" },
        refs: { type: "array", items: { type: "string" } },
        doc: { type: "string", description: "Full markdown of a spec or document to attach." },
        doc_name: { type: "string", description: 'Document name, default "spec".' },
      },
      required: ["kind", "text"],
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

/**
 * Chat users talk about ideas, not branches. A bare name like "notes-app"
 * (or "Notes App") means that idea's research branch, where chat work lives.
 */
export function resolveBranch(raw: string): string {
  let b = raw.trim().toLowerCase();
  if (b !== "default" && !b.includes("/")) {
    b = b.replace(/[^a-z0-9_-]+/g, "-").replace(/^-+|-+$/g, "");
    if (b) b = `${b}/research`;
  }
  return b;
}

function branchArg(args: Record<string, unknown>, key: string, required: boolean): string | undefined {
  const raw = str(args, key, required);
  const b = raw === undefined || raw.trim() === "" ? undefined : resolveBranch(raw);
  if (b === undefined && required) throw new ToolError(`missing argument: ${key}`);
  if (b !== undefined && !validBranch(b)) {
    throw new ToolError(`invalid branch "${b}": use lowercase "project/branch"`);
  }
  return b;
}

/**
 * The branch `ctx use` selected on the laptop (refs/active), published with
 * the store. Chat surfaces default to it so the user never has to type a
 * branch name in a chat.
 */
export async function activeBranch(env: Env): Promise<string> {
  const f = await getFile(env, "refs/active");
  const b = f?.text.trim();
  return b && validBranch(b) ? b : "default";
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

// ---- documents (protocol §2, `doc` record) ------------------------------

export interface DocRecord {
  rec: "doc";
  id: string;
  branch: string;
  name: string;
  title: string;
  body: string;
  cid: string;
  src: string;
  t_tx: string;
}

/** Same rule as Rust `Doc::new`: lowercase letters, digits and '-'. */
export function validDocName(n: string): boolean {
  return n.length > 0 && n.length <= 64 && /^[a-z0-9-]+$/.test(n);
}

function deriveTitle(body: string, name: string): string {
  for (const line of body.split("\n")) {
    const t = line.trim();
    if (!t) continue;
    const title = [...t.replace(/^#+/, "").trim()].slice(0, 120).join("");
    return title || name;
  }
  return name;
}

export function buildDoc(input: {
  branch: string;
  name?: string;
  title?: string;
  body: string;
  now?: Date;
}): DocRecord {
  if (!validBranch(input.branch)) throw new ToolError(`invalid branch "${input.branch}"`);
  const name = (input.name ?? "spec").trim().toLowerCase();
  if (!validDocName(name)) throw new ToolError(`invalid doc_name "${name}": use lowercase letters, digits and '-'`);
  const body = normalizeText(input.body);
  if (isBlank(body)) throw new ToolError("doc is empty");
  const givenTitle = input.title !== undefined ? normalizeText(input.title).trim() : "";
  const now = input.now ?? new Date();
  // Key order mirrors the Rust serializer.
  return {
    rec: "doc",
    id: ulid(now.getTime()),
    branch: input.branch,
    name,
    title: givenTitle || deriveTitle(body, name),
    body,
    cid: docCid(body),
    src: "cloud",
    t_tx: now.toISOString(),
  };
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

export interface LoggedDoc {
  id: string;
  branch: string;
  name: string;
  title: string;
  body: string;
  cid: string;
}

const STATUS_RANK: Record<string, number> = { proposed: 0, active: 1, superseded: 2, rejected: 3, archived: 4 };

function maxStatus(a: string, b: string): string {
  return (STATUS_RANK[b] ?? -1) > (STATUS_RANK[a] ?? -1) ? b : a;
}

function* records(files: string[]): Generator<Record<string, unknown>> {
  for (const file of files) {
    for (const line of file.split("\n")) {
      if (!line.trim()) continue;
      try {
        yield JSON.parse(line);
      } catch {
        // torn or corrupt line: skip, never fail
      }
    }
  }
}

/** All claims in the log, with effective status applied (protocol §2). */
export function parseLog(files: string[]): LoggedClaim[] {
  const claims = new Map<string, LoggedClaim>();
  const transitions: { claim: string; to: string }[] = [];
  for (const r of records(files)) {
    if (r.rec === "claim" && typeof r.id === "string" && typeof r.text === "string") {
      claims.set(r.id, r as unknown as LoggedClaim);
      if (typeof r.supersedes === "string") transitions.push({ claim: r.supersedes, to: "superseded" });
    } else if (r.rec === "status" && typeof r.claim === "string" && typeof r.to === "string") {
      transitions.push({ claim: r.claim, to: r.to });
    }
  }
  for (const t of transitions) {
    const c = claims.get(t.claim);
    if (c) c.status = maxStatus(c.status, t.to);
  }
  return [...claims.values()].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
}

/**
 * The current version of every document: for each (branch, name), the doc
 * record with the greatest id. Records whose cid doesn't match their body
 * are skipped, like the Rust reader does.
 */
export function parseDocs(files: string[]): LoggedDoc[] {
  // `ctx remove` hides a document version with an archived status record
  // that targets its id, exactly as it hides a claim.
  const archived = new Set<string>();
  for (const r of records(files)) {
    if (r.rec === "status" && r.to === "archived" && typeof r.claim === "string") archived.add(r.claim);
  }
  const latest = new Map<string, LoggedDoc>();
  for (const r of records(files)) {
    if (typeof r.id === "string" && archived.has(r.id)) continue;
    if (
      r.rec !== "doc" ||
      typeof r.id !== "string" ||
      typeof r.branch !== "string" ||
      typeof r.name !== "string" ||
      typeof r.body !== "string" ||
      typeof r.cid !== "string"
    ) {
      continue;
    }
    if (docCid(normalizeText(r.body)) !== r.cid) continue;
    const key = `${r.branch}\u0000${r.name}`;
    const prev = latest.get(key);
    if (!prev || r.id > prev.id) {
      latest.set(key, {
        id: r.id,
        branch: r.branch,
        name: r.name,
        title: typeof r.title === "string" ? r.title : r.name,
        body: r.body,
        cid: r.cid,
      });
    }
  }
  return [...latest.values()].sort((a, b) => (a.id < b.id ? -1 : 1));
}

/**
 * Find the current `name` doc: on `branch` itself first, then the newest one
 * on any branch of the same project (a spec written on research is what the
 * code branch builds from).
 */
export function findDoc(docs: LoggedDoc[], branch: string, name: string): LoggedDoc | undefined {
  const exact = docs.filter((d) => d.branch === branch && d.name === name);
  if (exact.length) return exact[exact.length - 1];
  const project = branch.includes("/") ? branch.slice(0, branch.indexOf("/")) : branch;
  const sameProject = docs.filter((d) => d.name === name && d.branch.startsWith(project + "/"));
  return sameProject[sameProject.length - 1];
}

async function readLogFiles(env: Env): Promise<string[]> {
  const tree = await listTree(env);
  const shards = tree.filter((e) => e.type === "blob" && e.path.startsWith("log/") && e.path.endsWith(".jsonl"));
  const files: string[] = [];
  // Bounded concurrency: subrequest limits on the free plan.
  for (let i = 0; i < shards.length; i += 8) {
    files.push(...(await Promise.all(shards.slice(i, i + 8).map((s) => getBlob(env, s.sha)))));
  }
  return files;
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

// ---- live views from the log ----------------------------------------------
// The laptop compiles packs with the real packer, but a chat must also see
// ideas started in another chat before any laptop has synced. These views
// are built straight from the log so ChatGPT -> claude.ai works cloud-only.

const VISIBLE = new Set(["active", "superseded"]);

const CHAT_PROTOCOL =
  '---\nWhen I say "ctx save", reply with only one fenced code block tagged `ctx-claims` containing a JSON array of {"kind", "text", "why", "refs"} objects (kind is one of fact, decision, rejected, constraint, question, claim). Nothing else.\n' +
  'When I say "ctx spec", write the complete spec for what we discussed as markdown inside one fenced block opened with four backticks and the tag ctx-spec (````ctx-spec) and closed with four backticks, so code blocks inside it survive. Nothing else.\n';

const USAGE =
  "\nHow to use: to continue an idea, call ctx_pack with its name. To start a new idea (the user names it, " +
  'e.g. "an idea called X"), save with branch set to that name, never into the current idea. Save only when the user asks. When the user says "ctx spec", save the full spec with ' +
  'ctx_append (kind "decision", a one-line summary as text, the markdown in doc).\n';

export function renderIndex(claims: LoggedClaim[], docs: LoggedDoc[], active: string): string {
  const byBranch = new Map<string, { n: number; last: string }>();
  for (const c of claims) {
    if (!VISIBLE.has(c.status)) continue;
    const e = byBranch.get(c.branch) ?? { n: 0, last: "" };
    e.n += 1;
    if (c.id > e.last) e.last = c.id;
    byBranch.set(c.branch, e);
  }
  for (const d of docs) if (!byBranch.has(d.branch)) byBranch.set(d.branch, { n: 0, last: d.id });
  let out = `# ContextOS index\n\nCurrent branch: \`${active}\`\n\n`;
  if (byBranch.size === 0) out += "_Nothing saved yet._\n";
  for (const [b, e] of [...byBranch.entries()].sort((a, c) => (a[1].last < c[1].last ? 1 : -1))) {
    const ds = docs.filter((d) => d.branch === b).map((d) => `${d.name}: "${d.title}"`);
    out += `- \`${b}\`: ${e.n} claims${ds.length ? `; documents: ${ds.join(", ")}` : ""}\n`;
  }
  return out + USAGE;
}

const SECTIONS: [string, (c: LoggedClaim) => boolean][] = [
  ["Thesis", (c) => c.status === "active" && (c.kind === "decision" || c.kind === "claim")],
  ["Evidence", (c) => c.status === "active" && c.kind === "fact"],
  ["Constraints", (c) => c.status === "active" && c.kind === "constraint"],
  ["Contradictions and rejected paths", (c) => c.kind === "rejected" || c.status === "superseded"],
  ["Open questions", (c) => c.status === "active" && c.kind === "question"],
];

function bullet(c: LoggedClaim): string {
  const tag = c.cid.replace(/^b3:/, "").slice(0, 4);
  const prev = c.status === "superseded" ? "Previously: " : "";
  const why = c.why ? `\n  - why: ${c.why.replace(/\n/g, " ")}` : "";
  return `- ${prev}${c.text.replace(/\n/g, " ")} [c:${tag}]${why}`;
}

/** A dossier rendered directly from the log (no packer, no budget: chat windows are large). */
export function renderLiveDossier(branch: string, claims: LoggedClaim[], docs: LoggedDoc[]): string {
  const mine = claims.filter((c) => c.branch === branch && VISIBLE.has(c.status));
  let out = `# Context dossier: ${branch}\n\nWhat is known about this idea so far. Treat it as background you already agreed with.\n\n`;
  const ds = docs.filter((d) => d.branch === branch);
  if (ds.length) {
    out += "## Documents\n\n";
    for (const d of ds) out += `- \`${d.name}\`: ${d.title} (read it with ctx_pack doc="${d.name}")\n`;
    out += "\n";
  }
  if (mine.length === 0) out += "_Nothing saved for this idea yet._\n\n";
  for (const [title, pick] of SECTIONS) {
    const items = mine.filter(pick);
    if (items.length) out += `## ${title}\n\n${items.map(bullet).join("\n")}\n\n`;
  }
  return out + CHAT_PROTOCOL;
}

/** Claims on `branch` that a compiled pack doesn't contain yet (saved since, from any chat). */
export function unseenClaims(pack: string, branch: string, claims: LoggedClaim[]): LoggedClaim[] {
  return claims.filter((c) => {
    if (c.branch !== branch || c.status !== "active") return false;
    const tag = `[c:${c.cid.replace(/^b3:/, "").slice(0, 4)}]`;
    return !pack.includes(tag);
  });
}

// ---- dispatch ------------------------------------------------------------

export async function callTool(env: Env, name: string, args: Record<string, unknown>): Promise<string> {
  switch (name) {
    case "ctx_index": {
      const [files, active] = await Promise.all([readLogFiles(env), activeBranch(env)]);
      return renderIndex(parseLog(files), parseDocs(files), active);
    }
    case "ctx_pack": {
      const branch = branchArg(args, "branch", false) ?? (await activeBranch(env));
      const task = str(args, "task", false);
      const docName = str(args, "doc", false)?.trim().toLowerCase();
      if (docName) {
        const docs = parseDocs(await readLogFiles(env));
        const doc = findDoc(docs, branch, docName);
        if (doc) return `# ${doc.title}\n\n${doc.body}\n`;
        const names = [...new Set(docs.filter((d) => d.branch.split("/")[0] === branch.split("/")[0]).map((d) => d.name))];
        return (
          `No document named "${docName}" for ${branch}. ` +
          (names.length ? `Available: ${names.join(", ")}.` : "No documents saved for this project yet.")
        );
      }
      const [f, files] = await Promise.all([getFile(env, `packs/${branch}.md`), readLogFiles(env)]);
      const claims = parseLog(files);
      let out: string;
      if (f) {
        // Compiled on a laptop; add anything saved from a chat since then.
        out = f.text;
        const fresh = unseenClaims(out, branch, claims);
        if (fresh.length) out += `\n\n## Saved since this pack was compiled\n\n${fresh.map(bullet).join("\n")}\n`;
      } else {
        out = renderLiveDossier(branch, claims, parseDocs(files));
      }
      if (task && task.trim()) {
        const hits = search(claims, task, branch);
        if (hits.length) out += `\n\n## Relevant to: ${task}\n\n${renderClaims(hits)}\n`;
      }
      return out;
    }
    case "ctx_search": {
      const query = str(args, "query", true)!;
      const branch = branchArg(args, "branch", false);
      const hits = search(parseLog(await readLogFiles(env)), query, branch);
      return hits.length ? renderClaims(hits) : "No matching claims.";
    }
    case "ctx_append":
    case "ctx_propose": {
      const propose = name === "ctx_propose";
      const branch = propose
        ? branchArg(args, "target_branch", true)!
        : (branchArg(args, "branch", false) ?? (await activeBranch(env)));
      const now = new Date();
      const docBody = propose ? undefined : str(args, "doc", false);
      const hasDoc = docBody !== undefined && !isBlank(docBody);

      let doc: DocRecord | undefined;
      let refs = propose ? undefined : args.refs;
      if (hasDoc) {
        doc = buildDoc({ branch, name: str(args, "doc_name", false), body: docBody!, now });
        const extra = `doc:${doc.name}`;
        refs = Array.isArray(refs) ? [...refs, extra] : [extra];
      }
      const rec = buildClaim({
        branch,
        kind: str(args, "kind", true)!,
        text: str(args, "text", true)!,
        why: str(args, "why", false),
        refs,
        status: propose ? "proposed" : "active",
        now,
      });

      // Dedup: an identical current version of the doc is not written again.
      let docSkipped = false;
      if (doc) {
        const current = findDoc(parseDocs(await readLogFiles(env)), branch, doc.name);
        docSkipped = current !== undefined && current.branch === branch && current.cid === doc.cid;
      }
      const lines = [...(doc && !docSkipped ? [JSON.stringify(doc)] : []), JSON.stringify(rec)];
      const what = doc && !docSkipped ? `${rec.kind} + doc "${doc.name}"` : rec.kind;
      await appendLines(env, shardPath(now), lines, `ctx: ${propose ? "propose" : "append"} ${what} to ${branch} (cloud)`);

      const tag = rec.cid.slice(3, 7);
      if (propose) {
        return `Proposed ${rec.kind} for ${branch} [c:${tag}] (id ${rec.id}); it will appear after review with \`ctx review\`.`;
      }
      let msg = `Saved ${rec.kind} to ${branch} [c:${tag}] (id ${rec.id}).`;
      if (doc) {
        msg += docSkipped
          ? ` Document "${doc.name}" is unchanged, so it was not saved again.`
          : ` Saved document "${doc.name}" ("${doc.title}"). On the laptop, \`ctx build\` turns it into SPEC.md for a coding agent.`;
      }
      return msg;
    }
    default:
      throw new ToolError(`unknown tool: ${name}`);
  }
}
