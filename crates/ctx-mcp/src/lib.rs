//! The RecurOS MCP server: exactly five tools (spec §9.1).
//!
//! Every tool schema is permanent per-turn context tax, so there are five
//! and their descriptions are terse. Anything else is discoverable through
//! `ctx_index`'s text.
//!
//! Transport is newline-delimited JSON-RPC 2.0 over stdio. The server keeps
//! no protocol state: `initialize` is answered for clients that send it, but
//! nothing depends on it having happened (MCP 2026-07-28 is stateless). This
//! is hand-rolled rather than built on an SDK because the surface is five
//! tools and three methods, and owning it keeps the binary small and the
//! behaviour identical across client versions.

use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use ctx_app::{App, PackOpts, SaveOutcome};
use ctx_core::{ClaimDraft, Filter, Kind, Store};
use serde_json::{Value, json};

/// Wait this long after the last append before committing and pushing, so a
/// burst of saves becomes one commit (spec §6.5).
pub const SYNC_DEBOUNCE: Duration = Duration::from_secs(30);

const FALLBACK_PROTOCOL: &str = "2025-06-18";

fn tools() -> Value {
    let kind = json!({
        "type": "string",
        "enum": ["fact", "decision", "rejected", "constraint", "question", "claim"]
    });
    json!([
        {
            "name": "ctx_index",
            "description": "Overview of the project context: branches and claim counts. Call first.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "ctx_pack",
            "description": "Compiled context for a branch, optionally focused on a task.",
            "inputSchema": {"type": "object", "properties": {
                "branch": {"type": "string", "description": "Defaults to the current branch."},
                "task": {"type": "string"},
                "budget": {"type": "integer", "description": "Max tokens."},
                "doc": {"type": "string", "description": "Return this document instead, e.g. \"spec\"."}
            }}
        },
        {
            "name": "ctx_search",
            "description": "Search recorded claims.",
            "inputSchema": {"type": "object", "properties": {
                "query": {"type": "string"},
                "branch": {"type": "string"}
            }, "required": ["query"]}
        },
        {
            "name": "ctx_append",
            "description": "Record a claim, only when the user asks. To save a spec: kind decision, a one-line summary as text, the full markdown in doc.",
            "inputSchema": {"type": "object", "properties": {
                "branch": {"type": "string", "description": "Defaults to the current branch."},
                "kind": kind,
                "text": {"type": "string"},
                "why": {"type": "string"},
                "refs": {"type": "array", "items": {"type": "string"}, "description": "Paths, URLs, repo@sha."},
                "doc": {"type": "string", "description": "Full markdown document to attach."},
                "doc_name": {"type": "string"}
            }, "required": ["kind", "text"]}
        },
        {
            "name": "ctx_propose",
            "description": "Suggest a claim for another branch; the user reviews it.",
            "inputSchema": {"type": "object", "properties": {
                "target_branch": {"type": "string"},
                "kind": kind,
                "text": {"type": "string"},
                "why": {"type": "string"}
            }, "required": ["target_branch", "kind", "text"]}
        }
    ])
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    str_arg(args, key).with_context(|| format!("missing required argument `{key}`"))
}

/// Result of one tool call: text for the model, and whether the store was
/// written (so the server knows to schedule a sync).
pub struct ToolOutput {
    pub text: String,
    pub wrote: bool,
}

/// Run one tool. Errors become in-band tool errors the model can read.
pub fn call_tool(app: &mut App, name: &str, args: &Value) -> Result<ToolOutput> {
    app.catch_up()?; // see claims written by other processes since last call
    let read = |text: String| Ok(ToolOutput { text, wrote: false });
    match name {
        "ctx_index" => read(app.index()?),
        "ctx_pack" => {
            if let Some(name) = str_arg(args, "doc") {
                let branch = app.resolve_branch(str_arg(args, "branch"))?;
                return read(match app.find_doc(&branch, name)? {
                    Some(d) => format!("# {}\n\n{}\n", d.title, d.body),
                    None => {
                        let names: Vec<String> = app
                            .visible_docs(&branch)?
                            .into_iter()
                            .map(|d| d.name)
                            .collect();
                        format!(
                            "No document `{name}` for {branch}. Available: {}",
                            if names.is_empty() {
                                "none".to_owned()
                            } else {
                                names.join(", ")
                            }
                        )
                    }
                });
            }
            let budget = args
                .get("budget")
                .and_then(Value::as_u64)
                .map(|b| b.clamp(100, 200_000) as u32);
            let pack = app.pack(&PackOpts {
                branch: str_arg(args, "branch").map(str::to_owned),
                task: str_arg(args, "task").map(str::to_owned),
                budget,
                // Chat-style consumers of MCP want reasons; coding agents
                // already have AGENTS.md. Keep the branch's own projection.
                projection: None,
            })?;
            read(pack.markdown)
        }
        "ctx_search" => {
            let query = required(args, "query")?;
            let branch = str_arg(args, "branch")
                .map(|b| app.resolve_branch(Some(b)))
                .transpose()?;
            let hits = app.store.search(
                query,
                &Filter {
                    branch,
                    limit: Some(20),
                    ..Default::default()
                },
            )?;
            if hits.is_empty() {
                return read(format!("No claims match `{query}`."));
            }
            let mut out = String::new();
            for c in hits {
                out.push_str(&format!(
                    "- [{}] {} ({}",
                    c.kind,
                    c.text.replace('\n', " "),
                    c.branch
                ));
                if c.status != ctx_core::Status::Active {
                    out.push_str(&format!(", {}", c.status));
                }
                out.push_str(&format!(") [c:{}]\n", c.short_cid()));
                if let Some(w) = &c.why {
                    out.push_str(&format!("  - why: {}\n", w.replace('\n', " ")));
                }
            }
            read(out)
        }
        "ctx_append" | "ctx_propose" => {
            let propose = name == "ctx_propose";
            let kind: Kind = required(args, "kind")?.parse()?;
            let branch_arg = if propose {
                Some(required(args, "target_branch")?)
            } else {
                str_arg(args, "branch")
            };
            let mut d = ClaimDraft::new(kind, required(args, "text")?);
            d.branch = app.resolve_branch(branch_arg)?;
            d.why = str_arg(args, "why").map(str::to_owned);
            d.refs = args
                .get("refs")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            d.src = std::env::var("CTX_SRC").unwrap_or_else(|_| "mcp".into());
            // A spec or other long document travels with the claim that
            // summarises it; the claim points at it with a `doc:<name>` ref.
            let mut doc_note = String::new();
            let mut wrote_doc = false;
            if !propose && let Some(body) = str_arg(args, "doc") {
                let name = str_arg(args, "doc_name").unwrap_or(ctx_app::SPEC);
                let saved = app.save_doc(&d.branch, name, None, body, &d.src)?;
                wrote_doc = !saved.duplicate;
                d.refs.push(format!("doc:{}", saved.doc.name));
                doc_note = if saved.duplicate {
                    format!(" Document `{}` unchanged.", saved.doc.name)
                } else {
                    format!(
                        " Document `{}` saved (\"{}\").",
                        saved.doc.name, saved.doc.title
                    )
                };
            }
            let outcome = if propose {
                app.propose(d)?
            } else {
                app.save(d)?
            };
            let text = match &outcome {
                SaveOutcome::Saved(c) if propose => format!(
                    "Proposed [c:{}] for {}; the user will review it with `ctx review`.",
                    c.short_cid(),
                    c.branch
                ),
                SaveOutcome::Saved(c) => format!("Saved [c:{}] to {}.", c.short_cid(), c.branch),
                SaveOutcome::Duplicate(c) => format!("Already recorded as [c:{}].", c.short_cid()),
            };
            let text = text + &doc_note;
            let wrote = wrote_doc || matches!(outcome, SaveOutcome::Saved(_));
            if wrote && !propose {
                // Keep the repo's AGENTS.md floor current for the next session.
                let _ = app.refresh_agents_md();
            }
            Ok(ToolOutput { text, wrote })
        }
        other => bail!("unknown tool `{other}`"),
    }
}

/// Handle one JSON-RPC message. Returns the response, or `None` for
/// notifications. `wrote` is set when a tool call changed the store.
pub fn handle(app: &mut App, msg: &Value, wrote: &mut bool) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let id = id?; // notifications (no id) get no response
    let result = match method {
        "initialize" => {
            let version = msg
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(FALLBACK_PROTOCOL);
            let active = app
                .default_branch()
                .map(|b| b.to_string())
                .unwrap_or_default();
            Ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "recuros", "version": ctx_app::VERSION},
                "instructions": format!(
                    "Project context for branch {active}. Call ctx_index for an overview. \
                     Save with ctx_append only when the user asks."
                )
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => {
            let name = msg
                .pointer("/params/name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let empty = json!({});
            let args = msg.pointer("/params/arguments").unwrap_or(&empty);
            Ok(match call_tool(app, name, args) {
                Ok(out) => {
                    *wrote |= out.wrote;
                    json!({"content": [{"type": "text", "text": out.text}]})
                }
                Err(e) => json!({
                    "content": [{"type": "text", "text": format!("Error: {e:#}")}],
                    "isError": true
                }),
            })
        }
        "resources/list" => Ok(json!({"resources": []})),
        "prompts/list" => Ok(json!({"prompts": []})),
        _ => Err((-32601, format!("method not found: {method}"))),
    };
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err((code, message)) => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
        }
    })
}

/// Serve MCP over stdin/stdout until stdin closes. Appends trigger a
/// debounced sync; any pending sync is flushed on exit.
pub fn serve_stdio(mut app: App) -> Result<()> {
    // Read stdin on a thread so the main loop can wake for the debounce.
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let stdout = std::io::stdout();
    let mut dirty_since: Option<Instant> = None;
    loop {
        let wait = dirty_since
            .map(|t| SYNC_DEBOUNCE.saturating_sub(t.elapsed()))
            .unwrap_or(Duration::from_secs(3600));
        match rx.recv_timeout(wait) {
            Ok(line) => {
                if line.trim().is_empty() {
                    continue;
                }
                let response = match serde_json::from_str::<Value>(&line) {
                    Ok(Value::Array(batch)) => {
                        let mut wrote = false;
                        let out: Vec<Value> = batch
                            .iter()
                            .filter_map(|m| handle(&mut app, m, &mut wrote))
                            .collect();
                        if wrote {
                            dirty_since = Some(Instant::now());
                        }
                        (!out.is_empty()).then_some(Value::Array(out))
                    }
                    Ok(msg) => {
                        let mut wrote = false;
                        let r = handle(&mut app, &msg, &mut wrote);
                        if wrote {
                            dirty_since = Some(Instant::now());
                        }
                        r
                    }
                    Err(e) => Some(json!({
                        "jsonrpc": "2.0", "id": null,
                        "error": {"code": -32700, "message": format!("parse error: {e}")}
                    })),
                };
                if let Some(r) = response {
                    let mut out = stdout.lock();
                    writeln!(out, "{r}")?;
                    out.flush()?;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if dirty_since.take().is_some() {
                    sync_quietly(&mut app);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if dirty_since.is_some() {
                    sync_quietly(&mut app);
                }
                return Ok(());
            }
        }
    }
}

fn sync_quietly(app: &mut App) {
    // Stdout belongs to the protocol; diagnostics go to stderr, which MCP
    // clients log. A failed push is retried on the next sync.
    if let Err(e) = app.sync() {
        eprintln!("ctx: sync failed (will retry later): {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_git::CtxHome;

    fn app() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let app = App::open(CtxHome::at(dir.path().join("ctx")), None).unwrap();
        (dir, app)
    }

    fn call(app: &mut App, name: &str, args: Value) -> (Value, bool) {
        let mut wrote = false;
        let msg = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}});
        (handle(app, &msg, &mut wrote).unwrap(), wrote)
    }

    fn text(v: &Value) -> &str {
        v.pointer("/result/content/0/text")
            .and_then(Value::as_str)
            .unwrap()
    }

    #[test]
    fn exactly_five_tools() {
        let (_d, mut app) = app();
        let mut wrote = false;
        let r = handle(
            &mut app,
            &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
            &mut wrote,
        )
        .unwrap();
        let names: Vec<&str> = r["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "ctx_index",
                "ctx_pack",
                "ctx_search",
                "ctx_append",
                "ctx_propose"
            ]
        );
        // Keep the per-turn tax small (spec §12.2 budgets ~300 tokens).
        let schema_tokens = ctx_core::tokens::estimate(&r["result"]["tools"].to_string());
        assert!(
            schema_tokens < 450,
            "tool schemas cost {schema_tokens} tokens"
        );
    }

    #[test]
    fn append_search_pack_round_trip() {
        let (_d, mut app) = app();
        let (r, wrote) = call(
            &mut app,
            "ctx_append",
            json!({"kind":"decision","text":"Use a federation peg","why":"covenants are slow"}),
        );
        assert!(wrote, "{r}");
        assert!(text(&r).starts_with("Saved [c:"));
        let (r, wrote) = call(
            &mut app,
            "ctx_append",
            json!({"kind":"decision","text":"Use a federation peg","why":"covenants are slow"}),
        );
        assert!(!wrote);
        assert!(text(&r).starts_with("Already recorded"));

        let (r, _) = call(&mut app, "ctx_search", json!({"query":"peg"}));
        assert!(text(&r).contains("why: covenants are slow"));
        let (r, _) = call(&mut app, "ctx_pack", json!({}));
        assert!(text(&r).contains("Use a federation peg"));
        let (r, _) = call(&mut app, "ctx_index", json!({}));
        assert!(text(&r).contains("default"));
    }

    #[test]
    fn errors_are_in_band_and_notifications_silent() {
        let (_d, mut app) = app();
        let (r, _) = call(&mut app, "ctx_append", json!({"kind":"opinion","text":"x"}));
        assert_eq!(r["result"]["isError"], true);
        let mut wrote = false;
        assert!(
            handle(
                &mut app,
                &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                &mut wrote
            )
            .is_none()
        );
        let r = handle(
            &mut app,
            &json!({"jsonrpc":"2.0","id":7,"method":"nope"}),
            &mut wrote,
        )
        .unwrap();
        assert_eq!(r["error"]["code"], -32601);
        let r = handle(&mut app, &json!({"jsonrpc":"2.0","id":8,"method":"initialize","params":{"protocolVersion":"2026-07-28"}}), &mut wrote).unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2026-07-28");
    }
}
