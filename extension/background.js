// RecurOS background service worker.
//
// This is the ONLY place that talks to ctxd. Content scripts cannot fetch
// http://127.0.0.1 because claude.ai / chatgpt.com page CSP blocks it (the
// resulting error looks like CORS but isn't); the service worker is not subject
// to page CSP. MV3 service workers are killed after ~30s idle, so nothing is
// kept in memory: every handler re-reads settings from chrome.storage.local.

const DEFAULTS = { url: "http://127.0.0.1:7777", token: "", branch: "" };
const DEFAULT_BUDGET = 4000;

async function settings() {
  const s = await chrome.storage.local.get(DEFAULTS);
  return { ...DEFAULTS, ...s, url: (s.url || DEFAULTS.url).replace(/\/+$/, "") };
}

class CtxError extends Error {}

/** Call ctxd. Throws CtxError with a message that tells the user how to fix it. */
async function ctxd(path, { method = "GET", body, auth = true, as = "json" } = {}) {
  const s = await settings();
  if (auth && !s.token) {
    throw new CtxError("No token set. Run `ctx daemon token` and paste it in the extension Options.");
  }
  const headers = {};
  if (auth) headers.Authorization = `Bearer ${s.token}`;
  if (body !== undefined) headers["Content-Type"] = "application/json";
  let res;
  try {
    res = await fetch(s.url + path, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch (e) {
    throw new CtxError(`Cannot reach ctxd at ${s.url} — is ctxd running? Run \`ctx daemon\`.`);
  }
  if (res.status === 401 || res.status === 403) {
    throw new CtxError("ctxd rejected the token. Run `ctx daemon token` and paste it in the extension Options.");
  }
  if (!res.ok) {
    let detail = "";
    try {
      detail = (await res.text()).slice(0, 300);
    } catch (_) {}
    throw new CtxError(`ctxd returned ${res.status}${detail ? `: ${detail}` : ""}`);
  }
  return as === "text" ? res.text() : res.json();
}

async function fetchPack({ branch, task, budget, format = "dossier" } = {}) {
  const s = await settings();
  const params = new URLSearchParams({ for: format });
  const b = branch || s.branch;
  if (b) params.set("branch", b);
  if (task) params.set("task", task);
  params.set("budget", String(budget || DEFAULT_BUDGET));
  return ctxd(`/v1/pack?${params}`, { as: "text" });
}

/** Deliver text to the tab's content script, injecting it if needed (activeTab). */
async function insertIntoTab(tabId, text) {
  const msg = { type: "ctx:insert", text };
  try {
    return await chrome.tabs.sendMessage(tabId, msg);
  } catch (_) {
    // No content script on this page (not a listed chat host, or loaded before
    // install). activeTab + scripting lets us inject on user gesture.
    await chrome.scripting.executeScript({ target: { tabId }, files: ["content.js"] });
    return chrome.tabs.sendMessage(tabId, msg);
  }
}

async function handle(msg) {
  switch (msg && msg.type) {
    case "ctx:health":
      return ctxd("/v1/health", { auth: false });
    case "ctx:branches":
      return ctxd("/v1/branches");
    case "ctx:pack":
      return { text: await fetchPack(msg) };
    case "ctx:insertPack": {
      const text = await fetchPack(msg);
      const result = await insertIntoTab(msg.tabId, text);
      if (!result || !result.ok) {
        throw new CtxError((result && result.error) || "Could not find a chat composer on this page. Use Copy instead.");
      }
      return { inserted: true, chars: text.length };
    }
    case "ctx:saveClaims": {
      const s = await settings();
      const body = { claims: msg.claims, src: msg.src };
      const branch = msg.branch || s.branch;
      if (branch) body.branch = branch;
      return ctxd("/v1/claims", { method: "POST", body });
    }
    case "ctx:saveDoc": {
      const s = await settings();
      const body = { name: msg.name || "spec", body: msg.body, src: msg.src };
      if (msg.title) body.title = msg.title;
      const branch = msg.branch || s.branch;
      if (branch) body.branch = branch;
      return ctxd("/v1/docs", { method: "POST", body });
    }
    case "ctx:docs": {
      const params = new URLSearchParams();
      if (msg.branch) params.set("branch", msg.branch);
      return ctxd(`/v1/docs?${params}`);
    }
    default:
      throw new CtxError(`Unknown message type: ${msg && msg.type}`);
  }
}

chrome.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
  handle(msg)
    .then((data) => sendResponse({ ok: true, data }))
    .catch((e) => sendResponse({ ok: false, error: e instanceof CtxError ? e.message : String(e && e.message ? e.message : e) }));
  return true; // async response
});

chrome.commands.onCommand.addListener(async (command, tab) => {
  if (command !== "insert-context") return;
  try {
    const target = tab && tab.id != null ? tab : (await chrome.tabs.query({ active: true, currentWindow: true }))[0];
    if (!target) return;
    const text = await fetchPack({});
    await insertIntoTab(target.id, text);
  } catch (e) {
    console.warn("[ctx] insert-context failed:", e && e.message ? e.message : e);
  }
});
