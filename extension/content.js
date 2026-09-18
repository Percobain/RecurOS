// ContextOS content script.
//
// Two jobs, both deliberately narrow:
//  1. Insert plain markdown into the chat composer when asked.
//  2. Find ```ctx-claims code blocks and offer a "Save to ctx" button, and
//     ````ctx-spec blocks (a full markdown spec) with a "Save spec to ctx" one.
//
// It never fetches localhost (page CSP blocks it; background.js does HTTP) and
// never scrapes chat content: selectors only look at code blocks, which depend
// on markdown fences rather than a chat UI's ever-changing DOM.

(() => {
  if (window.__ctxContentLoaded) return;
  window.__ctxContentLoaded = true;

  const KINDS = new Set(["fact", "decision", "rejected", "constraint", "question", "claim"]);

  // ---- 1. Insertion --------------------------------------------------------

  function isEditable(el) {
    if (!el || el === document.body) return false;
    if (el.tagName === "TEXTAREA") return !el.disabled && !el.readOnly;
    if (el.tagName === "INPUT") return false;
    return el.isContentEditable === true;
  }

  function findComposer() {
    const active = document.activeElement;
    if (isEditable(active)) return active;
    const candidates = [
      ...document.querySelectorAll('div[contenteditable="true"]'),
      ...document.querySelectorAll("textarea"),
    ];
    return candidates.find((el) => el.offsetParent !== null && isEditable(el)) || candidates.find(isEditable) || null;
  }

  function insertText(text) {
    const el = findComposer();
    if (!el) return { ok: false, error: "Could not find a chat composer on this page. Use Copy instead." };
    el.focus();
    if (el.tagName === "TEXTAREA") {
      // React-controlled textareas ignore direct .value writes; use the native
      // setter and dispatch input so the framework sees the change.
      const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value").set;
      const start = el.selectionStart ?? el.value.length;
      const end = el.selectionEnd ?? el.value.length;
      setter.call(el, el.value.slice(0, start) + text + el.value.slice(end));
      el.dispatchEvent(new Event("input", { bubbles: true }));
      return { ok: true };
    }
    // contenteditable (ProseMirror, Lexical, Quill): execCommand goes through
    // the editor's own input handling, unlike DOM mutation.
    if (!document.execCommand("insertText", false, text)) {
      return { ok: false, error: "This editor refused the insert. Use Copy and paste instead." };
    }
    return { ok: true };
  }

  chrome.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
    if (msg && msg.type === "ctx:insert") {
      try {
        sendResponse(insertText(String(msg.text || "")));
      } catch (e) {
        sendResponse({ ok: false, error: String(e && e.message ? e.message : e) });
      }
    }
  });

  // ---- 2. Capture ----------------------------------------------------------

  function surface() {
    const h = location.hostname;
    if (h.endsWith("claude.ai")) return "claude.ai";
    if (h.endsWith("chatgpt.com")) return "chatgpt";
    if (h.endsWith("gemini.google.com")) return "gemini";
    if (h.endsWith("perplexity.ai")) return "perplexity";
    if (h.endsWith("deepseek.com")) return "deepseek";
    return h;
  }

  function hash(s) {
    // FNV-1a; only used to recognise blocks already saved on this page.
    let h = 0x811c9dc5;
    for (let i = 0; i < s.length; i++) {
      h ^= s.charCodeAt(i);
      h = Math.imul(h, 0x01000193);
    }
    return (h >>> 0).toString(16);
  }

  /** True if the code block is fenced with language `tag` (class or label). */
  function labelledAs(codeEl, tag) {
    const cls = ` ${codeEl.className || ""} ${(codeEl.parentElement && codeEl.parentElement.className) || ""} `;
    // Matches `ctx-spec`, `language-ctx-spec`, `lang-ctx-spec` as whole class tokens.
    if (cls.split(/\s+/).some((c) => c === tag || c.endsWith(`-${tag}`))) return true;
    // Many UIs render the fence language as a label just above the <pre>.
    const pre = codeEl.closest("pre") || codeEl;
    for (let node = pre, depth = 0; node && depth < 3; node = node.parentElement, depth++) {
      const prev = node.previousElementSibling;
      if (prev && prev.textContent && prev.textContent.trim().toLowerCase() === tag) return true;
      const first = node.parentElement && node.parentElement.firstElementChild;
      if (first && first !== node && first.textContent && first.textContent.trim().toLowerCase().startsWith(tag)) return true;
    }
    return false;
  }

  /**
   * A spec block: fenced as ctx-spec, or raw text that still carries the
   * opening fence (UIs that don't render markdown). Specs are markdown, never
   * parsed as claims.
   */
  function isSpecBlock(codeEl) {
    if (labelledAs(codeEl, "ctx-spec")) return true;
    return /^`{3,}[ \t]*ctx-spec\b/.test((codeEl.textContent || "").trimStart());
  }

  /** Strip a leftover ````ctx-spec fence if the UI didn't render it. */
  function specBody(raw) {
    const m = raw.trim().match(/^(`{3,})[ \t]*ctx-spec[^\n]*\n([\s\S]*?)\n?\1[ \t]*$/);
    return (m ? m[2] : raw).trim();
  }

  /** Parse a block's text into claims, or null if it isn't a claims array. */
  function parseClaims(raw) {
    let text = raw.trim();
    const fence = text.match(/^```\s*ctx-claims\s*\n([\s\S]*?)\n?```$/);
    if (fence) text = fence[1].trim();
    if (!text.startsWith("[")) return null;
    let data;
    try {
      data = JSON.parse(text);
    } catch (_) {
      return null;
    }
    if (!Array.isArray(data) || data.length === 0) return null;
    if (!data.every((c) => c && typeof c === "object" && typeof c.kind === "string" && typeof c.text === "string")) return null;
    return data;
  }

  /** Split into claims the API will accept and a count of rejected ones. */
  function validate(claims) {
    const valid = [];
    let invalid = 0;
    for (const c of claims) {
      const kind = c.kind.trim().toLowerCase();
      const text = c.text.trim();
      if (!KINDS.has(kind) || !text) {
        invalid++;
        continue;
      }
      const out = { kind, text };
      if (typeof c.why === "string" && c.why.trim()) out.why = c.why;
      if (Array.isArray(c.refs)) out.refs = c.refs.filter((r) => typeof r === "string");
      if (Array.isArray(c.entities)) out.entities = c.entities.filter((e) => typeof e === "string");
      valid.push(out);
    }
    return { valid, invalid };
  }

  const withButton = new WeakSet();
  const savedHashes = new Set();

  function styleButton(btn, state) {
    const colors = { idle: "#4f46e5", busy: "#6b7280", ok: "#047857", err: "#b91c1c" };
    btn.style.cssText = [
      "display:inline-block",
      "margin:6px 0 10px",
      "padding:4px 10px",
      "font:500 12px/1.4 system-ui,sans-serif",
      "color:#fff",
      `background:${colors[state]}`,
      "border:0",
      "border-radius:6px",
      "cursor:pointer",
      "opacity:0.92",
    ].join(";");
  }

  function addButton(codeEl) {
    const pre = codeEl.closest("pre") || codeEl;
    if (withButton.has(pre)) return;
    withButton.add(pre);

    const btn = document.createElement("button");
    btn.type = "button";
    btn.dataset.ctxSave = "1";
    btn.textContent = "Save to ctx";
    styleButton(btn, "idle");
    if (savedHashes.has(hash(codeEl.textContent || ""))) {
      btn.textContent = "Already saved";
      styleButton(btn, "ok");
    }

    btn.addEventListener("click", async (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      // Re-read at click time: the block may have finished streaming since.
      const raw = codeEl.textContent || "";
      const claims = parseClaims(raw);
      if (!claims) {
        btn.textContent = "Not a valid ctx-claims JSON array";
        styleButton(btn, "err");
        return;
      }
      const { valid, invalid } = validate(claims);
      if (valid.length === 0) {
        btn.textContent = `No valid claims (${invalid} with unknown kind or empty text)`;
        styleButton(btn, "err");
        return;
      }
      btn.textContent = "Saving…";
      styleButton(btn, "busy");
      btn.disabled = true;
      let res;
      try {
        res = await chrome.runtime.sendMessage({ type: "ctx:saveClaims", claims: valid, src: surface() });
      } catch (e) {
        res = { ok: false, error: "Extension was reloaded; refresh this page." };
      }
      btn.disabled = false;
      if (!res || !res.ok) {
        btn.textContent = `ctx: ${(res && res.error) || "unknown error"}`;
        styleButton(btn, "err");
        return;
      }
      const d = res.data || {};
      const parts = [`Saved ${(d.saved || []).length}`];
      if ((d.duplicates || []).length) parts.push(`${d.duplicates.length} duplicate${d.duplicates.length === 1 ? "" : "s"}`);
      const errs = (d.errors || []).length + invalid;
      if (errs) parts.push(`${errs} rejected`);
      btn.textContent = parts.join(" · ");
      btn.title = (d.errors || []).join("\n");
      styleButton(btn, errs ? "err" : "ok");
      savedHashes.add(hash(raw));
    });

    pre.insertAdjacentElement("afterend", btn);
  }

  function addSpecButton(codeEl) {
    const pre = codeEl.closest("pre") || codeEl;
    if (withButton.has(pre)) return;
    withButton.add(pre);

    const btn = document.createElement("button");
    btn.type = "button";
    btn.dataset.ctxSave = "1";
    btn.textContent = "Save spec to ctx";
    styleButton(btn, "idle");
    if (savedHashes.has(hash(codeEl.textContent || ""))) {
      btn.textContent = "Spec already saved";
      styleButton(btn, "ok");
    }

    btn.addEventListener("click", async (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      // Re-read at click time: the block may have finished streaming since.
      const raw = codeEl.textContent || "";
      const body = specBody(raw);
      if (!body) {
        btn.textContent = "The spec block is empty";
        styleButton(btn, "err");
        return;
      }
      btn.textContent = "Saving spec…";
      styleButton(btn, "busy");
      btn.disabled = true;
      let res;
      try {
        res = await chrome.runtime.sendMessage({ type: "ctx:saveDoc", name: "spec", body, src: surface() });
      } catch (e) {
        res = { ok: false, error: "Extension was reloaded; refresh this page." };
      }
      btn.disabled = false;
      if (!res || !res.ok) {
        btn.textContent = `ctx: ${(res && res.error) || "unknown error"}`;
        styleButton(btn, "err");
        return;
      }
      const d = res.data || {};
      btn.textContent = d.duplicate ? "Spec unchanged (already saved)" : `Spec saved to ${d.branch || "ctx"}`;
      btn.title = d.title || "";
      styleButton(btn, "ok");
      savedHashes.add(hash(raw));
    });

    pre.insertAdjacentElement("afterend", btn);
  }

  function scan() {
    for (const codeEl of document.querySelectorAll("pre code, pre")) {
      if (codeEl.tagName === "PRE" && codeEl.querySelector("code")) continue; // handled via its <code>
      const pre = codeEl.closest("pre") || codeEl;
      if (withButton.has(pre)) continue;
      const text = codeEl.textContent || "";
      if (text.length > 500000) continue;
      if (isSpecBlock(codeEl)) {
        addSpecButton(codeEl);
      } else if (text.length <= 200000 && (parseClaims(text) || (labelledAs(codeEl, "ctx-claims") && text.trim().startsWith("[")))) {
        addButton(codeEl);
      }
    }
  }

  let timer = null;
  const observer = new MutationObserver((mutations) => {
    // Ignore our own button insertions.
    if (mutations.every((m) => [...m.addedNodes].every((n) => n.dataset && n.dataset.ctxSave))) return;
    clearTimeout(timer);
    timer = setTimeout(scan, 800);
  });
  observer.observe(document.body, { childList: true, subtree: true, characterData: true });
  scan();
})();
