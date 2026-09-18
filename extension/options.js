// Options: where ctxd is, its bearer token, and an optional default branch.

const DEFAULTS = { url: "http://127.0.0.1:7777", token: "", branch: "" };
const $ = (id) => document.getElementById(id);

function status(msg, cls = "") {
  $("status").textContent = msg;
  $("status").className = cls;
}

async function load() {
  const s = await chrome.storage.local.get(DEFAULTS);
  $("url").value = s.url || DEFAULTS.url;
  $("token").value = s.token || "";
  $("branch").value = s.branch || "";
}

async function save() {
  await chrome.storage.local.set({
    url: ($("url").value.trim() || DEFAULTS.url).replace(/\/+$/, ""),
    token: $("token").value.trim(),
    branch: $("branch").value.trim(),
  });
}

async function send(msg) {
  const res = await chrome.runtime.sendMessage(msg);
  if (!res || !res.ok) throw new Error((res && res.error) || "No response from extension background.");
  return res.data;
}

$("save").addEventListener("click", async () => {
  await save();
  status("Saved.", "ok");
});

$("test").addEventListener("click", async () => {
  await save();
  status("Testing…");
  try {
    const health = await send({ type: "ctx:health" });
    const branches = await send({ type: "ctx:branches" });
    const n = (branches.branches || []).length;
    status(
      `Connected to ctxd ${health.version || ""}\nToken accepted · ${n} branch${n === 1 ? "" : "es"} · active: ${branches.active || "(none)"}`,
      "ok",
    );
  } catch (e) {
    status(e.message, "err");
  }
});

load();
