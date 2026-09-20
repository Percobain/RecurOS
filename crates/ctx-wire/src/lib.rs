//! Wiring RecurOS into coding agents (spec §12).
//!
//! There is no universal hook layer, so this is deliberately conservative:
//!
//! * `AGENTS.md` is the floor and is handled by `ctx-app`; everything here is
//!   an optional improvement on top, so a failure is reported, never fatal.
//! * We add our own entry to config files and never modify anyone else's.
//! * We never merge into an existing hooks configuration (spec §12.1): if the
//!   user already has hooks, we say so and leave the file alone.
//! * Hooks inject on SessionStart only, never per prompt, so the prompt
//!   prefix stays stable and prompt caching keeps working (spec §12.2).

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    ClaudeCode,
    Codex,
    Cursor,
    Gemini,
}

impl Agent {
    pub const ALL: [Agent; 4] = [
        Agent::ClaudeCode,
        Agent::Codex,
        Agent::Cursor,
        Agent::Gemini,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude-code",
            Agent::Codex => "codex",
            Agent::Cursor => "cursor",
            Agent::Gemini => "gemini",
        }
    }

    pub fn parse(s: &str) -> Option<Agent> {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" | "claudecode" => Some(Agent::ClaudeCode),
            "codex" => Some(Agent::Codex),
            "cursor" => Some(Agent::Cursor),
            "gemini" | "gemini-cli" => Some(Agent::Gemini),
            _ => None,
        }
    }

    /// Installed if its CLI is on PATH or its config dir exists.
    pub fn detect(self, home: &Path) -> bool {
        let (bin, dir) = match self {
            Agent::ClaudeCode => ("claude", ".claude"),
            Agent::Codex => ("codex", ".codex"),
            Agent::Cursor => ("cursor", ".cursor"),
            Agent::Gemini => ("gemini", ".gemini"),
        };
        which(bin).is_some() || home.join(dir).is_dir()
    }
}

impl fmt::Display for Agent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Written,
    Unchanged,
    Skipped(String),
}

#[derive(Debug, Clone)]
pub struct Step {
    pub what: String,
    pub path: PathBuf,
    pub outcome: Outcome,
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mark = match &self.outcome {
            Outcome::Written => "✓ wrote    ",
            Outcome::Unchanged => "· current  ",
            Outcome::Skipped(_) => "! skipped  ",
        };
        write!(f, "{mark}{} ({})", self.what, self.path.display())?;
        if let Outcome::Skipped(why) = &self.outcome {
            write!(f, "\n             {why}")?;
        }
        Ok(())
    }
}

/// Find an executable on PATH (honouring PATHEXT on Windows).
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT".into())
            .split(';')
            .map(|e| e.to_ascii_lowercase())
            .chain([String::new()])
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// How agents should invoke us. Prefer plain `ctx` when it is on PATH, since
/// several of these files are committed and must work on teammates'
/// machines. Otherwise use this binary's absolute path (forward slashes:
/// agents on Windows often launch through a POSIX shell).
pub fn ctx_command() -> (String, bool) {
    if which("ctx").is_some() {
        return ("ctx".into(), true);
    }
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| "ctx".into());
    (exe, false)
}

fn read_json_object(path: &Path) -> Result<Option<Map<String, Value>>, String> {
    match fs::read_to_string(path) {
        Ok(s) if s.trim().is_empty() => Ok(Some(Map::new())),
        Ok(s) => match serde_json::from_str::<Value>(&s) {
            Ok(Value::Object(m)) => Ok(Some(m)),
            Ok(_) => Err("not a JSON object; left untouched".into()),
            Err(e) => Err(format!("not valid JSON ({e}); left untouched")),
        },
        Err(_) => Ok(None),
    }
}

fn write_json(path: &Path, v: &Map<String, Value>) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut s = serde_json::to_string_pretty(v)?;
    s.push('\n');
    fs::write(path, s).with_context(|| format!("writing {}", path.display()))
}

/// Add `mcpServers.ctx` to a JSON config (Claude `.mcp.json`, Cursor,
/// Gemini). Other servers and settings are preserved; an existing `ctx`
/// entry is left as the user configured it.
pub fn ensure_mcp_json(path: &Path, command: &str, what: &str) -> Step {
    let step = |outcome| Step {
        what: what.to_owned(),
        path: path.to_owned(),
        outcome,
    };
    let mut obj = match read_json_object(path) {
        Ok(o) => o.unwrap_or_default(),
        Err(e) => return step(Outcome::Skipped(e)),
    };
    let servers = obj.entry("mcpServers").or_insert_with(|| json!({}));
    let Some(servers) = servers.as_object_mut() else {
        return step(Outcome::Skipped(
            "`mcpServers` is not an object; left untouched".into(),
        ));
    };
    if servers.contains_key("ctx") {
        return step(Outcome::Unchanged);
    }
    servers.insert("ctx".into(), json!({"command": command, "args": ["mcp"]}));
    match write_json(path, &obj) {
        Ok(()) => step(Outcome::Written),
        Err(e) => step(Outcome::Skipped(e.to_string())),
    }
}

/// Claude Code SessionStart hook, in the *local* (uncommitted) settings file
/// so teammates without RecurOS don't get a failing hook.
pub fn ensure_claude_hook(repo: &Path, command: &str) -> Step {
    let path = repo.join(".claude").join("settings.local.json");
    let step = |outcome| Step {
        what: "Claude Code SessionStart hook".into(),
        path: path.clone(),
        outcome,
    };
    // Hook commands run through a shell; quote a path with spaces
    // (e.g. `C:/Program Files/...`).
    let exe = if command.contains(' ') {
        format!("\"{command}\"")
    } else {
        command.to_owned()
    };
    let hook_cmd = format!("{exe} hook session-start");
    let mut obj = match read_json_object(&path) {
        Ok(o) => o.unwrap_or_default(),
        Err(e) => return step(Outcome::Skipped(e)),
    };
    if let Some(hooks) = obj.get("hooks") {
        if hooks.to_string().contains("hook session-start") {
            return step(Outcome::Unchanged);
        }
        return step(Outcome::Skipped(format!(
            "this file already defines hooks and we never merge into a hooks config. \
             To add it yourself, put a SessionStart command hook running `{hook_cmd}`."
        )));
    }
    obj.insert(
        "hooks".into(),
        json!({"SessionStart": [{"hooks": [{"type": "command", "command": hook_cmd}]}]}),
    );
    match write_json(&path, &obj) {
        Ok(()) => step(Outcome::Written),
        Err(e) => step(Outcome::Skipped(e.to_string())),
    }
}

/// Codex reads MCP servers from `~/.codex/config.toml`. We append our own
/// table if absent; the file is otherwise untouched.
pub fn ensure_codex(home: &Path, command: &str) -> Step {
    let path = home.join(".codex").join("config.toml");
    let step = |outcome| Step {
        what: "Codex MCP server".into(),
        path: path.clone(),
        outcome,
    };
    let existing = fs::read_to_string(&path).unwrap_or_default();
    if existing.contains("[mcp_servers.ctx]") {
        return step(Outcome::Unchanged);
    }
    let mut out = existing.clone();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.is_empty() {
        out.push('\n');
    }
    // TOML basic strings: escape backslashes and quotes in the path.
    let cmd = command.replace('\\', "\\\\").replace('"', "\\\"");
    out.push_str(&format!(
        "[mcp_servers.ctx]\ncommand = \"{cmd}\"\nargs = [\"mcp\"]\n"
    ));
    let result =
        fs::create_dir_all(path.parent().unwrap_or(home)).and_then(|_| fs::write(&path, out));
    match result {
        Ok(()) => step(Outcome::Written),
        Err(e) => step(Outcome::Skipped(e.to_string())),
    }
}

/// Make an agent memory file (`CLAUDE.md`, `GEMINI.md`) import `AGENTS.md`.
/// A plain file with `@AGENTS.md` works on every OS, so no symlinks.
pub fn ensure_import(path: &Path, what: &str) -> Step {
    let step = |outcome| Step {
        what: what.to_owned(),
        path: path.to_owned(),
        outcome,
    };
    let existing = fs::read_to_string(path).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == "@AGENTS.md") {
        return step(Outcome::Unchanged);
    }
    let out = if existing.trim().is_empty() {
        "@AGENTS.md\n".to_owned()
    } else {
        format!("{}\n\n@AGENTS.md\n", existing.trim_end())
    };
    match fs::write(path, out) {
        Ok(()) => step(Outcome::Written),
        Err(e) => step(Outcome::Skipped(e.to_string())),
    }
}

/// Wire one agent into `repo`. `home` is the user's home directory.
pub fn wire(agent: Agent, repo: &Path, home: &Path, command: &str) -> Vec<Step> {
    match agent {
        Agent::ClaudeCode => vec![
            ensure_import(&repo.join("CLAUDE.md"), "CLAUDE.md imports AGENTS.md"),
            ensure_mcp_json(&repo.join(".mcp.json"), command, "Claude Code MCP server"),
            ensure_claude_hook(repo, command),
        ],
        // Codex reads AGENTS.md natively.
        Agent::Codex => vec![ensure_codex(home, command)],
        // Cursor reads AGENTS.md natively.
        Agent::Cursor => vec![ensure_mcp_json(
            &repo.join(".cursor").join("mcp.json"),
            command,
            "Cursor MCP server",
        )],
        Agent::Gemini => vec![
            ensure_import(&repo.join("GEMINI.md"), "GEMINI.md imports AGENTS.md"),
            ensure_mcp_json(
                &repo.join(".gemini").join("settings.json"),
                command,
                "Gemini CLI MCP server",
            ),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_json_adds_only_our_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        fs::write(
            &path,
            r#"{"mcpServers":{"other":{"command":"x"}},"keep":1}"#,
        )
        .unwrap();
        assert_eq!(ensure_mcp_json(&path, "ctx", "t").outcome, Outcome::Written);
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["other"]["command"], "x");
        assert_eq!(v["mcpServers"]["ctx"]["args"][0], "mcp");
        assert_eq!(v["keep"], 1);
        assert_eq!(
            ensure_mcp_json(&path, "ctx", "t").outcome,
            Outcome::Unchanged
        );

        fs::write(&path, "{ not json").unwrap();
        assert!(matches!(
            ensure_mcp_json(&path, "ctx", "t").outcome,
            Outcome::Skipped(_)
        ));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "{ not json",
            "bad files are never touched"
        );
    }

    #[test]
    fn never_merges_into_existing_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".claude/settings.local.json");
        assert_eq!(
            ensure_claude_hook(dir.path(), "C:/Program Files/ctx.exe").outcome,
            Outcome::Written
        );
        let written = fs::read_to_string(&path).unwrap();
        assert!(
            written.contains(r#""\"C:/Program Files/ctx.exe\" hook session-start""#),
            "{written}"
        );
        assert_eq!(
            ensure_claude_hook(dir.path(), "ctx").outcome,
            Outcome::Unchanged
        );

        fs::write(&path, r#"{"hooks":{"Stop":[]}}"#).unwrap();
        assert!(matches!(
            ensure_claude_hook(dir.path(), "ctx").outcome,
            Outcome::Skipped(_)
        ));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            r#"{"hooks":{"Stop":[]}}"#
        );
    }

    #[test]
    fn imports_and_codex_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join("CLAUDE.md");
        fs::write(&claude, "# Notes\n").unwrap();
        assert_eq!(ensure_import(&claude, "t").outcome, Outcome::Written);
        assert_eq!(ensure_import(&claude, "t").outcome, Outcome::Unchanged);
        assert_eq!(
            fs::read_to_string(&claude).unwrap(),
            "# Notes\n\n@AGENTS.md\n"
        );

        let toml = dir.path().join(".codex/config.toml");
        fs::create_dir_all(toml.parent().unwrap()).unwrap();
        fs::write(&toml, "model = \"x\"").unwrap();
        assert_eq!(
            ensure_codex(dir.path(), "C:\\bin\\ctx.exe").outcome,
            Outcome::Written
        );
        assert_eq!(ensure_codex(dir.path(), "ctx").outcome, Outcome::Unchanged);
        let t = fs::read_to_string(&toml).unwrap();
        assert!(t.starts_with("model = \"x\"\n\n[mcp_servers.ctx]"));
        assert!(t.contains(r#"command = "C:\\bin\\ctx.exe""#), "{t}");
    }
}
