// Minimal GitHub REST client for the one private repo that holds ~/ctx.
//
// Base64 appears here only because it is the GitHub Contents API transport
// encoding. It is decoded before anything reaches a model (invariant 9).

export interface Env {
  GITHUB_TOKEN: string;
  GITHUB_REPO: string; // "owner/name"
  GITHUB_BRANCH?: string;
  CTX_SECRET: string;
  /**
   * Workers Rate Limiting binding (see wrangler.toml). Optional so tests
   * and `wrangler dev` without the binding still work.
   */
  LIMITER?: { limit(options: { key: string }): Promise<{ success: boolean }> };
}

export class GitHubError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}

export interface TreeEntry {
  path: string;
  type: "blob" | "tree" | "commit";
  sha: string;
}

const API = "https://api.github.com";

function branch(env: Env): string {
  return env.GITHUB_BRANCH || "main";
}

function encodePath(path: string): string {
  return path.split("/").map(encodeURIComponent).join("/");
}

async function gh(env: Env, path: string, init: RequestInit = {}): Promise<Response> {
  // A missing secret would otherwise be sent as "Bearer undefined" and come
  // back from GitHub as a confusing 401. Also tolerate stray whitespace from
  // pasting the token into `wrangler secret put`.
  const token = (env.GITHUB_TOKEN ?? "").trim();
  if (!token) {
    throw new Error(
      "the GITHUB_TOKEN secret is not set on this Worker. Run `npx wrangler secret put GITHUB_TOKEN` " +
        "in the worker/ directory and paste the token when prompted (the name is literally GITHUB_TOKEN).",
    );
  }
  return fetch(API + path, {
    ...init,
    headers: {
      Authorization: `Bearer ${token}`,
      Accept: "application/vnd.github+json",
      "X-GitHub-Api-Version": "2022-11-28",
      "User-Agent": "recuros-worker",
      ...(init.body ? { "Content-Type": "application/json" } : {}),
    },
  });
}

async function fail(res: Response, what: string): Promise<never> {
  const body = await res.text().catch(() => "");
  throw new GitHubError(res.status, `GitHub ${what} failed (${res.status}): ${body.slice(0, 300)}`);
}

export function base64ToUtf8(b64: string): string {
  const bin = atob(b64.replace(/\s/g, ""));
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return new TextDecoder().decode(bytes);
}

export function utf8ToBase64(s: string): string {
  const bytes = new TextEncoder().encode(s);
  let bin = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    bin += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(bin);
}

/** A file's text and blob sha, or null if it doesn't exist. */
export async function getFile(env: Env, path: string): Promise<{ text: string; sha: string } | null> {
  const res = await gh(
    env,
    `/repos/${env.GITHUB_REPO}/contents/${encodePath(path)}?ref=${encodeURIComponent(branch(env))}`,
  );
  if (res.status === 404) return null;
  if (!res.ok) await fail(res, `read ${path}`);
  const json = (await res.json()) as { sha: string; content?: string; encoding?: string };
  // Files over 1 MB come back without inline content; fetch the blob instead.
  if (json.encoding !== "base64" || !json.content) {
    return { text: await getBlob(env, json.sha), sha: json.sha };
  }
  return { text: base64ToUtf8(json.content), sha: json.sha };
}

export async function getBlob(env: Env, sha: string): Promise<string> {
  const res = await gh(env, `/repos/${env.GITHUB_REPO}/git/blobs/${sha}`);
  if (!res.ok) await fail(res, `read blob ${sha}`);
  const json = (await res.json()) as { content: string };
  return base64ToUtf8(json.content);
}

/** Every entry in the branch's tree (recursive). */
export async function listTree(env: Env): Promise<TreeEntry[]> {
  const res = await gh(
    env,
    `/repos/${env.GITHUB_REPO}/git/trees/${encodeURIComponent(branch(env))}?recursive=1`,
  );
  if (res.status === 404 || res.status === 409) return []; // empty repo
  if (!res.ok) await fail(res, "list tree");
  const json = (await res.json()) as { tree: TreeEntry[] };
  return json.tree;
}

/**
 * Append one or more lines to a file in a single commit, creating it if
 * needed. GitHub's Contents API is compare-and-swap on the blob sha, so
 * concurrent Worker instances can race; on a sha conflict we re-read and
 * retry. Several lines in one PUT means a doc and the claim that refers to
 * it land atomically.
 */
export async function appendLines(env: Env, path: string, lines: string[], message: string): Promise<void> {
  await appendLine(env, path, lines.map((l) => (l.endsWith("\n") ? l : l + "\n")).join(""), message);
}

/** Append text ending in one or more complete lines (see appendLines). */
export async function appendLine(env: Env, path: string, line: string, message: string): Promise<void> {
  for (let attempt = 0; attempt < 5; attempt++) {
    const existing = await getFile(env, path);
    let text = existing?.text ?? "";
    // Same crash-safety rule as the Rust shard writer: never glue a record
    // onto an unterminated line.
    if (text !== "" && !text.endsWith("\n")) text += "\n";
    text += line.endsWith("\n") ? line : line + "\n";

    const res = await gh(env, `/repos/${env.GITHUB_REPO}/contents/${encodePath(path)}`, {
      method: "PUT",
      body: JSON.stringify({
        message,
        content: utf8ToBase64(text),
        branch: branch(env),
        ...(existing ? { sha: existing.sha } : {}),
      }),
    });
    if (res.ok) return;
    if (res.status === 409 || res.status === 422) {
      await new Promise((r) => setTimeout(r, 100 * 2 ** attempt));
      continue;
    }
    await fail(res, `write ${path}`);
  }
  throw new GitHubError(409, `could not append to ${path}: too many concurrent writes, try again`);
}
