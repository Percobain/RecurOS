// Popup: pick a branch, fetch a dossier pack via background.js, and insert it
// into the current tab's composer (or copy it).

const $ = (id) => document.getElementById(id);

function status(msg, cls = "") {
  const el = $("status");
  el.textContent = msg;
  el.className = cls;
}

async function send(msg) {
  const res = await chrome.runtime.sendMessage(msg);
  if (!res || !res.ok) throw new Error((res && res.error) || "No response from extension background.");
  return res.data;
}

function packRequest() {
  return {
    branch: $("branch").value,
    task: $("task").value.trim(),
    budget: parseInt($("budget").value, 10) || 4000,
  };
}

async function loadBranches() {
  try {
    const { branch: preferred } = await chrome.storage.local.get({ branch: "" });
    const data = await send({ type: "ctx:branches" });
    const sel = $("branch");
    sel.innerHTML = "";
    const branches = data.branches || [];
    if (branches.length === 0) sel.append(new Option("(active branch)", ""));
    for (const b of branches) {
      const label = `${b.name}${b.name === data.active ? " (active)" : ""} · ${b.claims}`;
      sel.append(new Option(label, b.name));
    }
    sel.value = preferred || data.active || (branches[0] && branches[0].name) || "";
  } catch (e) {
    status(e.message, "err");
  }
  loadDocs();
}

/** Show the branch's documents (e.g. its spec). Optional: failures are silent. */
async function loadDocs() {
  const el = $("docs");
  el.textContent = "";
  try {
    const branch = $("branch").value;
    const data = await send({ type: "ctx:docs", branch });
    const docs = (data && data.docs) || [];
    el.textContent = docs.map((d) => `${d.name}: ${d.title}`).join(" · ");
  } catch (_) {
    // Older ctxd without /v1/docs, or no connection: the status line covers it.
  }
}

$("branch").addEventListener("change", loadDocs);

$("insert").addEventListener("click", async () => {
  status("Compiling pack…");
  try {
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    if (!tab) throw new Error("No active tab.");
    const r = await send({ type: "ctx:insertPack", tabId: tab.id, ...packRequest() });
    status(`Inserted ${r.chars} characters.`, "ok");
  } catch (e) {
    status(e.message, "err");
  }
});

$("copy").addEventListener("click", async () => {
  status("Compiling pack…");
  try {
    const { text } = await send({ type: "ctx:pack", ...packRequest() });
    await navigator.clipboard.writeText(text);
    status(`Copied ${text.length} characters. Paste into any chat.`, "ok");
  } catch (e) {
    status(e.message, "err");
  }
});

$("options").addEventListener("click", (e) => {
  e.preventDefault();
  chrome.runtime.openOptionsPage();
});

loadBranches();
