//! `ctx doctor`: diagnose the store, git, this repo's wiring, and the daemon.
//! Every check prints what's wrong *and* the command that fixes it.

use std::path::Path;
use std::time::Duration;

use ctx_app::App;
use ctx_branch::Binding;
use ctx_git::{CtxHome, sync};
use ctx_wire::Agent;

use crate::http;

#[derive(Default)]
struct Report {
    problems: usize,
    warnings: usize,
}

impl Report {
    fn ok(&self, msg: impl AsRef<str>) {
        println!("  ✓ {}", msg.as_ref());
    }
    fn warn(&mut self, msg: impl AsRef<str>, fix: impl AsRef<str>) {
        self.warnings += 1;
        println!("  ! {}\n      → {}", msg.as_ref(), fix.as_ref());
    }
    fn fail(&mut self, msg: impl AsRef<str>, fix: impl AsRef<str>) {
        self.problems += 1;
        println!("  ✗ {}\n      → {}", msg.as_ref(), fix.as_ref());
    }
}

pub fn run(home: CtxHome, cwd: &Path) -> bool {
    let mut r = Report::default();

    println!("store");
    if !home.exists() {
        r.fail(
            format!("no store at {}", home.root().display()),
            "run `ctx init` in a project, or `ctx save \"...\"` to create it",
        );
        println!("\n1 problem");
        return false;
    }
    r.ok(format!("store at {}", home.root().display()));
    let app = match App::open(home.clone(), Some(cwd)) {
        Ok(a) => a,
        Err(e) => {
            r.fail(
                format!("cannot open the store: {e:#}"),
                "fix the file named above, then `ctx reindex`",
            );
            println!("\n{} problem(s)", r.problems);
            return false;
        }
    };
    r.ok(format!("machine id `{}`", app.machine()));
    match ctx_core::Store::stats(&app.store) {
        Ok(s) => r.ok(format!(
            "{} claims indexed from {} shard(s)",
            s.claims, s.shards
        )),
        Err(e) => r.fail(format!("index unreadable: {e}"), "`ctx reindex`"),
    }

    println!("\ngit sync");
    if ctx_wire::which("git").is_none() {
        r.warn(
            "git is not installed: ContextOS works, but won't sync",
            "install git to sync between machines",
        );
    } else if !sync::is_repo(&home) {
        r.warn(
            "the store is not a git repo",
            format!("git -C \"{}\" init", home.root().display()),
        );
    } else {
        match sync::remote_url(&home) {
            Some(url) => r.ok(format!("remote {url}")),
            None => r.warn(
                "no git remote: everything works locally, but nothing syncs",
                format!(
                    "create a PRIVATE repo, then: git -C \"{}\" remote add origin <url> && ctx sync",
                    home.root().display()
                ),
            ),
        }
    }

    println!("\nthis directory");
    match Binding::discover(cwd) {
        Ok(Some(b)) => {
            let branch = b.branch_ref().map(|x| x.to_string()).unwrap_or_default();
            r.ok(format!(
                "bound to `{branch}` by {}",
                b.root.join(".ctx.yaml").display()
            ));
            if let Ok(br) = b.branch_ref()
                && !app.branches.contains(&br)
            {
                r.warn(
                    format!("branch `{branch}` isn't defined in refs/branches.yaml"),
                    "`ctx init` creates the standard research/code branches",
                );
            }
            let agents_md = b.root.join("AGENTS.md");
            if !ctx_app::agents_md::has_block(&b.root) {
                r.fail(
                    "AGENTS.md has no ContextOS section",
                    "`ctx init` (or `ctx pack --out AGENTS.md`)",
                );
            } else {
                match app.pack(&ctx_app::PackOpts {
                    branch: Some(branch.clone()),
                    projection: Some(ctx_pack::Projection::AgentsMd),
                    ..Default::default()
                }) {
                    Ok(p) => {
                        let current = std::fs::read_to_string(&agents_md).unwrap_or_default();
                        if ctx_app::agents_md::current_block(&current) == Some(p.markdown.as_str())
                        {
                            r.ok("AGENTS.md is up to date");
                        } else {
                            r.warn(
                                "AGENTS.md is stale",
                                "`ctx sync` or any `ctx save` refreshes it",
                            );
                        }
                    }
                    Err(e) => r.fail(
                        format!("cannot compile this branch: {e:#}"),
                        "check refs/branches.yaml",
                    ),
                }
            }
            let (_, on_path) = ctx_wire::ctx_command();
            if !on_path {
                r.warn(
                    "`ctx` is not on PATH, so agent configs point at this binary's full path",
                    "install ctx on PATH (see README) and re-run `ctx init`",
                );
            }
            let user_home = std::env::home_dir().unwrap_or_default();
            for agent in Agent::ALL.into_iter().filter(|a| a.detect(&user_home)) {
                let files: Vec<std::path::PathBuf> = match agent {
                    Agent::ClaudeCode => vec![b.root.join(".mcp.json")],
                    Agent::Codex => vec![user_home.join(".codex/config.toml")],
                    Agent::Cursor => vec![b.root.join(".cursor/mcp.json")],
                    Agent::Gemini => vec![b.root.join(".gemini/settings.json")],
                };
                let wired = files.iter().all(|f| {
                    std::fs::read_to_string(f)
                        .is_ok_and(|s| s.contains("\"ctx\"") || s.contains("[mcp_servers.ctx]"))
                });
                if wired {
                    r.ok(format!("{agent} is wired"));
                } else {
                    r.warn(
                        format!("{agent} is installed but not wired here"),
                        "`ctx init`",
                    );
                }
            }
        }
        Ok(None) => r.warn(
            "not inside a bound project (no .ctx.yaml)",
            "run `ctx init` in your project to wire it up",
        ),
        Err(e) => r.fail(
            format!("bad .ctx.yaml: {e}"),
            "fix or delete it, then `ctx init`",
        ),
    }

    println!("\nbrowser extension daemon");
    match ctx_daemon::token_path() {
        Ok(p) if p.exists() => r.ok(format!("token at {}", p.display())),
        _ => r.warn("no daemon token yet", "`ctx daemon token` creates one"),
    }
    match http::request(
        "GET",
        &format!("http://{}/v1/health", ctx_daemon::DEFAULT_ADDR),
        None,
        Duration::from_millis(500),
    ) {
        Ok((200, _)) => r.ok(format!("daemon running on {}", ctx_daemon::DEFAULT_ADDR)),
        _ => {
            println!("  · daemon not running (only needed for the browser extension: `ctx daemon`)")
        }
    }

    println!();
    match (r.problems, r.warnings) {
        (0, 0) => println!("all good"),
        (0, w) => println!("{w} warning(s), no problems"),
        (p, w) => println!("{p} problem(s), {w} warning(s)"),
    }
    r.problems == 0
}
