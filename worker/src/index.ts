// ContextOS Cloudflare Worker: a stateless MCP server (JSON-RPC over HTTP)
// at POST /mcp/<CTX_SECRET>. No sessions, no state between requests, so any
// request can land on any instance.

import type { Env } from "./github.js";
import { callTool, ToolError, TOOLS } from "./tools.js";

export type { Env };

const DEFAULT_PROTOCOL = "2025-06-18";
const VERSION = "0.1.0";

interface RpcRequest {
  jsonrpc?: string;
  id?: string | number | null;
  method?: unknown;
  params?: Record<string, unknown>;
}

type RpcResponse =
  | { jsonrpc: "2.0"; id: string | number | null; result: unknown }
  | { jsonrpc: "2.0"; id: string | number | null; error: { code: number; message: string } };

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

function rpcError(id: string | number | null, code: number, message: string): RpcResponse {
  return { jsonrpc: "2.0", id, error: { code, message } };
}

/** Returns null for notifications (no response expected). */
async function handle(env: Env, req: RpcRequest): Promise<RpcResponse | null> {
  const isNotification = req.id === undefined;
  const id = req.id ?? null;
  if (typeof req.method !== "string") {
    return isNotification ? null : rpcError(id, -32600, "invalid request");
  }
  if (req.method.startsWith("notifications/") || isNotification) return null;

  switch (req.method) {
    case "initialize": {
      const requested = req.params?.protocolVersion;
      return {
        jsonrpc: "2.0",
        id,
        result: {
          protocolVersion: typeof requested === "string" ? requested : DEFAULT_PROTOCOL,
          capabilities: { tools: { listChanged: false } },
          serverInfo: { name: "contextos", version: VERSION },
          instructions:
            "ContextOS holds curated project context. Call ctx_index first, then ctx_pack for a branch. " +
            "Save with ctx_append only when the user explicitly asks.",
        },
      };
    }
    case "ping":
      return { jsonrpc: "2.0", id, result: {} };
    case "tools/list":
      return { jsonrpc: "2.0", id, result: { tools: TOOLS } };
    case "tools/call": {
      const name = req.params?.name;
      const args = (req.params?.arguments ?? {}) as Record<string, unknown>;
      if (typeof name !== "string") return rpcError(id, -32602, "missing tool name");
      if (!TOOLS.some((t) => t.name === name)) return rpcError(id, -32602, `unknown tool: ${name}`);
      try {
        const text = await callTool(env, name, args);
        return { jsonrpc: "2.0", id, result: { content: [{ type: "text", text }] } };
      } catch (e) {
        // Tool failures are reported in-band so the model can see and react.
        const msg = e instanceof ToolError ? e.message : `error: ${(e as Error).message}`;
        return { jsonrpc: "2.0", id, result: { content: [{ type: "text", text: msg }], isError: true } };
      }
    }
    default:
      return rpcError(id, -32601, `method not found: ${req.method}`);
  }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    // The secret path segment is the credential. Unknown paths look like
    // nothing is here at all.
    if (!env.CTX_SECRET || url.pathname !== `/mcp/${env.CTX_SECRET}`) {
      return new Response("not found", { status: 404 });
    }
    if (request.method !== "POST") {
      return new Response("method not allowed", { status: 405, headers: { Allow: "POST" } });
    }

    let body: unknown;
    try {
      body = await request.json();
    } catch {
      return json(rpcError(null, -32700, "parse error"));
    }

    if (Array.isArray(body)) {
      const results = (await Promise.all(body.map((r) => handle(env, r as RpcRequest)))).filter(
        (r): r is RpcResponse => r !== null,
      );
      return results.length ? json(results) : new Response(null, { status: 202 });
    }
    const result = await handle(env, body as RpcRequest);
    return result ? json(result) : new Response(null, { status: 202 });
  },
} satisfies ExportedHandler<Env>;
