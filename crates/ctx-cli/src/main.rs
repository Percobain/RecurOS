//! `ctx` — the ContextOS command-line interface.

mod clipboard;
mod doctor;
mod eval;
mod http;

use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, NaiveDate, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use ctx_app::{App, PackOpts, SaveOutcome, VerifyResult, first_line};
use ctx_branch::Binding;
use ctx_core::{BranchRef, Claim, ClaimDraft, Confidence, Filter, Kind, Status, Store};
use ctx_git::CtxHome;
use ctx_pack::{Projection, UNLIMITED};
use ctx_wire::Agent;

#[derive(Parser)]
#[command(
    name = "ctx",
    version,
    about = "ContextOS: one shared, versioned memory for every AI tool you use",
    long_about = "ContextOS: one shared, versioned memory for every AI tool you use.\n\n\
        Save decisions, constraints and rejected ideas once; every agent (Claude Code, Codex, \
        Cursor, Gemini, claude.ai, ChatGPT, local models) gets a compiled, token-budgeted view.\n\n\
        Start with `ctx init` inside a project.",
    after_help = "Examples:\n  \
        ctx init                                   wire this repo to its context branch\n  \
        ctx save \"Use SQLite, not Postgres\" -k decision -w \"no server to run\"\n  \
        ctx pack                                   print the compiled context\n  \
        ctx pack --for handoff > HANDOFF.md        a handoff doc for a teammate\n  \
        ctx pack | ollama run qwen3                feed a local model"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Wire the current project: .ctx.yaml, AGENTS.md, agent configs. Idempotent.
    Init {
        /// Project name (default: the directory name).
        #[arg(long)]
        project: Option<String>,
        /// Branch this repo's agents use (default: code).
        #[arg(long)]
        branch: Option<String>,
        /// Agents to wire, comma-separated (default: every installed one).
        #[arg(long, value_delimiter = ',')]
        agents: Vec<String>,
        /// Only write .ctx.yaml and AGENTS.md; don't touch agent configs.
        #[arg(long)]
        no_agents: bool,
    },
    /// Record a claim. Instant and offline: no network, no model call.
    Save {
        /// The claim. Use `-` to read from stdin.
        text: Option<String>,
        #[arg(short, long, value_enum, default_value_t = KindArg::Fact)]
        kind: KindArg,
        /// Why it's true, or why it was decided.
        #[arg(short, long)]
        why: Option<String>,
        /// References: paths, repo@sha, URLs. Comma-separated or repeated.
        #[arg(short, long, value_delimiter = ',')]
        refs: Vec<String>,
        /// Topic tags. Comma-separated or repeated.
        #[arg(short, long = "tag", value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(short, long, value_enum, default_value_t = ConfidenceArg::Medium)]
        confidence: ConfidenceArg,
        /// Branch to save to (default: this repo's branch, or `ctx use`).
        #[arg(long)]
        to: Option<String>,
        /// This claim replaces an older one (id or c:cid prefix).
        #[arg(long)]
        supersedes: Option<String>,
        /// Read a ```ctx-claims block (from a chat) off the clipboard.
        #[arg(long, conflicts_with = "text")]
        paste: bool,
        #[arg(long, default_value = "cli", hide = true)]
        src: String,
    },
    /// Full-text search over claims.
    Search {
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
        #[arg(short, long)]
        branch: Option<String>,
        #[arg(short, long, value_enum)]
        kind: Vec<KindArg>,
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
        #[arg(short, long)]
        verbose: bool,
    },
    /// Compile a branch's context into markdown.
    Pack {
        /// Branch (default: this repo's branch, or `ctx use`).
        branch: Option<String>,
        /// Focus the pack on a task.
        #[arg(long)]
        task: Option<String>,
        /// Token budget.
        #[arg(long)]
        budget: Option<u32>,
        /// Shape: agents-md, markdown, dossier, prose/handoff.
        #[arg(long = "for", value_name = "SHAPE")]
        projection: Option<String>,
        /// Copy to the clipboard instead of printing.
        #[arg(long)]
        clip: bool,
        /// Write to a file. For AGENTS.md, only the ContextOS section is replaced.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Set the default branch for chat surfaces and commands outside a repo.
    Use { branch: String },
    /// Where am I: store, branch, counts, pending proposals.
    Status,
    /// Manage context branches.
    #[command(subcommand)]
    Branch(BranchCmd),
    /// Review proposed claims: list them, or accept/reject one.
    Review {
        #[command(subcommand)]
        action: Option<ReviewCmd>,
    },
    /// Show one claim in full, with its history.
    Show { id: String },
    /// Retire a claim (a status flip; nothing is ever deleted).
    Archive {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Mark a claim as helpful (up) or harmful (down); feeds pack ranking.
    Rate { id: String, verdict: Verdict },
    /// Find near-duplicate claims worth merging (no model involved).
    Refine {
        #[arg(short, long)]
        branch: Option<String>,
        /// Similarity threshold, 0-1.
        #[arg(long, default_value_t = 0.6)]
        threshold: f64,
    },
    /// Commit, pull and push the store (works offline: then it just commits).
    Sync,
    /// Compare a pack/state root from elsewhere with this machine.
    Verify {
        root: Option<String>,
        #[arg(short, long)]
        branch: Option<String>,
    },
    /// Drop and rebuild the index (.cache/ctx.db) from the log.
    Reindex,
    /// Show recorded claims, oldest first.
    Log {
        #[arg(short, long)]
        branch: Option<String>,
        #[arg(short, long, value_enum)]
        kind: Vec<KindArg>,
        /// Only claims recorded on or after this date (YYYY-MM-DD).
        #[arg(long)]
        since: Option<NaiveDate>,
        /// Show at most this many (the most recent). 0 = all.
        #[arg(short = 'n', long, default_value_t = 50)]
        limit: usize,
        #[arg(short, long)]
        verbose: bool,
    },
    /// Score packs against questions with known answers, to tune weights.
    Eval {
        /// Fixture file (default: <store>/refs/eval.yaml).
        #[arg(long)]
        fixtures: Option<PathBuf>,
        /// Ollama model to answer with; without it, checks the pack itself.
        #[arg(long)]
        model: Option<String>,
        /// Budgets to compare, comma-separated.
        #[arg(long, value_delimiter = ',', default_value = "300,700,1500")]
        budget: Vec<u32>,
        #[arg(long, default_value = "http://127.0.0.1:11434")]
        ollama: String,
        /// Write an example fixture file and exit.
        #[arg(long)]
        example: bool,
    },
    /// Diagnose the store, git, this repo's wiring and the daemon.
    Doctor,
    /// Run the MCP server on stdio (agents start this; you don't need to).
    Mcp,
    /// Run the local HTTP API for the browser extension.
    Daemon {
        #[command(subcommand)]
        action: Option<DaemonCmd>,
        #[arg(long, default_value = ctx_daemon::DEFAULT_ADDR)]
        addr: String,
    },
    /// Agent hook entry points (called by agents, not by you).
    #[command(subcommand, hide = true)]
    Hook(HookCmd),
}

#[derive(Subcommand)]
enum BranchCmd {
    /// Create a branch: `ctx branch new gtm --parent research --template gtm`.
    New {
        name: String,
        #[arg(long)]
        parent: Option<String>,
        /// research, code, gtm or writing.
        #[arg(long)]
        template: Option<String>,
        /// Extra visibility, e.g. `code:constraint` (repeatable).
        #[arg(long)]
        inherits: Vec<String>,
    },
    /// List branches with claim counts.
    Ls,
    /// Copy a branch's active claims into another (appends; originals stay).
    Merge {
        src: String,
        #[arg(long)]
        into: String,
    },
    /// Archive a branch and its claims (nothing is deleted).
    Archive { name: String },
}

#[derive(Subcommand)]
enum ReviewCmd {
    /// Accept a proposed claim.
    Accept { id: String },
    /// Reject a proposed claim; the reason is kept forever.
    Reject {
        id: String,
        #[arg(long)]
        reason: String,
    },
}

#[derive(Subcommand)]
enum DaemonCmd {
    /// Print the token to paste into the browser extension.
    Token,
}

#[derive(Subcommand)]
enum HookCmd {
    /// Pull, refresh AGENTS.md, and print context that changed since it was loaded.
    SessionStart,
}

#[derive(Clone, Copy, ValueEnum)]
enum Verdict {
    Up,
    Down,
}

#[derive(Clone, Copy, ValueEnum)]
enum KindArg {
    Fact,
    Decision,
    Rejected,
    Constraint,
    Question,
    Claim,
}

impl From<KindArg> for Kind {
    fn from(k: KindArg) -> Kind {
        match k {
            KindArg::Fact => Kind::Fact,
            KindArg::Decision => Kind::Decision,
            KindArg::Rejected => Kind::Rejected,
            KindArg::Constraint => Kind::Constraint,
            KindArg::Question => Kind::Question,
            KindArg::Claim => Kind::Claim,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum ConfidenceArg {
    High,
    Medium,
    Low,
}

impl From<ConfidenceArg> for Confidence {
    fn from(c: ConfidenceArg) -> Confidence {
        match c {
            ConfidenceArg::High => Confidence::High,
            ConfidenceArg::Medium => Confidence::Medium,
            ConfidenceArg::Low => Confidence::Low,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // Hooks must never break an agent session.
    let is_hook = matches!(cli.command, Command::Hook(_));
    match run(cli) {
        Ok(code) => code,
        Err(e) if is_hook => {
            eprintln!("ctx hook: {e:#}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn open(home: &CtxHome) -> Result<App> {
    App::open(home.clone(), Some(&cwd()))
}

fn run(cli: Cli) -> Result<ExitCode> {
    let home = CtxHome::locate()?;
    match cli.command {
        Command::Init {
            project,
            branch,
            agents,
            no_agents,
        } => init(&home, project, branch, &agents, no_agents)?,
        Command::Save {
            text,
            kind,
            why,
            refs,
            tags,
            confidence,
            to,
            supersedes,
            paste,
            src,
        } => {
            let mut app = open(&home)?;
            let branch = app.resolve_branch(to.as_deref())?;
            if paste {
                let block = clipboard::paste()?;
                let inputs = ctx_app::claims_block::parse(&block)?;
                let report = app.save_inputs(&inputs, &branch, "clipboard");
                for s in &report.saved {
                    println!("saved {} -> {branch}", tag(&s.cid));
                }
                for d in &report.duplicates {
                    println!("already recorded {}", tag(&d.cid));
                }
                for e in &report.errors {
                    eprintln!("skipped: {e}");
                }
            } else {
                let text = match text.as_deref() {
                    Some("-") => read_stdin()?,
                    None if !io::stdin().is_terminal() => read_stdin()?,
                    Some(t) => t.to_owned(),
                    None => {
                        bail!("nothing to save: `ctx save \"your claim\"`, or `ctx save --paste`")
                    }
                };
                let mut draft = ClaimDraft::new(kind.into(), text);
                draft.branch = branch;
                draft.why = why;
                draft.refs = refs;
                draft.entities = tags;
                draft.confidence = confidence.into();
                draft.src = src;
                if let Some(s) = supersedes {
                    draft.supersedes = Some(app.find_one(&s)?.id);
                }
                match app.save(draft)? {
                    SaveOutcome::Saved(c) => {
                        println!("saved {} {} -> {}", tag(&c.cid), c.kind, c.branch)
                    }
                    SaveOutcome::Duplicate(c) => {
                        println!("already recorded as {} ({})", tag(&c.cid), c.id)
                    }
                }
            }
            // Keep this repo's AGENTS.md current; never fail a save over it.
            if let Err(e) = app.refresh_agents_md() {
                eprintln!("warning: could not refresh AGENTS.md: {e:#}");
            }
        }
        Command::Search {
            query,
            branch,
            kind,
            limit,
            verbose,
        } => {
            let app = open(&home)?;
            let filter = Filter {
                branch: branch
                    .as_deref()
                    .map(|b| app.resolve_branch(Some(b)))
                    .transpose()?,
                kinds: kind.into_iter().map(Kind::from).collect(),
                limit: Some(limit),
                ..Default::default()
            };
            let hits = app.store.search(&query.join(" "), &filter)?;
            if hits.is_empty() {
                eprintln!("no matches");
            }
            for c in &hits {
                print_claim(c, verbose, true);
            }
        }
        Command::Pack {
            branch,
            task,
            budget,
            projection,
            clip,
            out,
        } => {
            let app = open(&home)?;
            let projection = projection
                .as_deref()
                .map(|p| p.parse::<Projection>().map_err(anyhow::Error::msg))
                .transpose()?;
            let budget = match (budget, projection) {
                (Some(b), _) => Some(b),
                (None, Some(Projection::Prose)) => Some(UNLIMITED),
                _ => None,
            };
            let pack = app.pack(&PackOpts {
                branch,
                task,
                budget,
                projection,
            })?;
            if let Some(path) = out {
                let is_agents = path
                    .file_name()
                    .is_some_and(|n| n.eq_ignore_ascii_case("AGENTS.md"));
                let content = if is_agents {
                    ctx_app::agents_md::splice(
                        &std::fs::read_to_string(&path).unwrap_or_default(),
                        &pack.markdown,
                    )
                } else {
                    pack.markdown.clone()
                };
                std::fs::write(&path, content)
                    .with_context(|| format!("writing {}", path.display()))?;
                eprintln!(
                    "wrote {} ({} claims, ~{} tokens)",
                    path.display(),
                    pack.claims.len(),
                    pack.tokens
                );
            } else if clip {
                clipboard::copy(&pack.markdown)?;
                eprintln!(
                    "copied {} claims (~{} tokens) to the clipboard: paste it into any chat",
                    pack.claims.len(),
                    pack.tokens
                );
            } else {
                print!("{}", pack.markdown);
            }
        }
        Command::Use { branch } => {
            let mut app = open(&home)?;
            let b = app.resolve_branch(Some(&branch))?;
            let known = app.branches.contains(&b)
                || app.store.branches()?.iter().any(|s| s.branch == b.as_str());
            app.set_active(&b)?;
            println!("active branch: {b}");
            if !known {
                eprintln!(
                    "note: `{b}` has no claims and isn't defined yet; `ctx branch new` defines it"
                );
            }
        }
        Command::Status => status(&home)?,
        Command::Branch(cmd) => branch_cmd(&home, cmd)?,
        Command::Review { action } => review(&home, action)?,
        Command::Show { id } => {
            let app = open(&home)?;
            let c = app.find_one(&id)?;
            print_claim(&c, true, true);
            for h in app.store.status_history(c.id)? {
                println!(
                    "{}{}  → {}{}",
                    " ".repeat(20),
                    h.t_tx.format("%Y-%m-%d"),
                    h.to,
                    h.reason.map(|r| format!(": {r}")).unwrap_or_default()
                );
            }
        }
        Command::Archive { id, reason } => {
            let mut app = open(&home)?;
            let c = app.find_one(&id)?;
            app.transition(&c, Status::Archived, reason, "cli")?;
            println!(
                "archived {} (still in the log; hidden from packs)",
                tag(&c.cid)
            );
            let _ = app.refresh_agents_md();
        }
        Command::Rate { id, verdict } => {
            let mut app = open(&home)?;
            let c = app.find_one(&id)?;
            app.rate(&c, matches!(verdict, Verdict::Up))?;
            println!("noted {}", tag(&c.cid));
        }
        Command::Refine { branch, threshold } => refine(&home, branch, threshold)?,
        Command::Sync => {
            let mut app = open(&home)?;
            let r = app.sync()?;
            match &r.remote {
                None => println!(
                    "committed locally{} (no remote yet; to sync: git -C \"{}\" remote add origin <private repo url>)",
                    if r.committed { "" } else { ": nothing new" },
                    home.root().display()
                ),
                Some(url) => println!("synced with {url}"),
            }
            let _ = app.refresh_agents_md();
        }
        Command::Verify { root, branch } => {
            let app = open(&home)?;
            let b = app.resolve_branch(branch.as_deref())?;
            match root {
                None => println!("{}  {b}", app.state_root(&b)?),
                Some(r) => match app.verify(&b, &r)? {
                    VerifyResult::InSync => println!("in-sync"),
                    VerifyResult::Behind(n) => {
                        println!("behind: that side is missing {n} claim(s) this machine has")
                    }
                    VerifyResult::AheadOrDiverged => {
                        println!(
                            "ahead-or-diverged: that side has claims this machine hasn't seen; run `ctx sync`"
                        )
                    }
                },
            }
        }
        Command::Reindex => {
            if !home.exists() {
                bail!("no ContextOS store at {}", home.root().display());
            }
            let start = Instant::now();
            let mut app = App::open(home.clone(), None)?;
            app.reindex()?;
            let stats = app.store.stats()?;
            println!(
                "reindexed {} claims from {} shards in {:.0?}",
                stats.claims,
                stats.shards,
                start.elapsed()
            );
        }
        Command::Log {
            branch,
            kind,
            since,
            limit,
            verbose,
        } => {
            let app = open(&home)?;
            let filter = Filter {
                branch: branch
                    .as_deref()
                    .map(|b| app.resolve_branch(Some(b)))
                    .transpose()?,
                kinds: kind.into_iter().map(Kind::from).collect(),
                since: since.map(|d| {
                    DateTime::<Utc>::from_naive_utc_and_offset(d.and_time(Default::default()), Utc)
                }),
                limit: (limit > 0).then_some(limit),
                ..Default::default()
            };
            let claims = app.store.scan(&filter)?;
            if claims.is_empty() {
                eprintln!("no claims yet: `ctx save \"...\"` records one");
            }
            for c in &claims {
                print_claim(c, verbose, branch.is_none());
            }
        }
        Command::Eval {
            fixtures,
            model,
            budget,
            ollama,
            example,
        } => {
            let path = fixtures.unwrap_or_else(|| home.refs_dir().join("eval.yaml"));
            if example {
                if path.exists() {
                    bail!("{} already exists", path.display());
                }
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                std::fs::write(&path, eval::EXAMPLE)?;
                println!(
                    "wrote {}: edit it with real questions, then run `ctx eval`",
                    path.display()
                );
                return Ok(ExitCode::SUCCESS);
            }
            let app = open(&home)?;
            eval::run(
                &app,
                &path,
                &eval::Options {
                    budgets: &budget,
                    model: model.as_deref(),
                    ollama: &ollama,
                    projection: Projection::Markdown,
                },
            )?;
        }
        Command::Doctor => {
            if !doctor::run(home, &cwd()) {
                return Ok(ExitCode::FAILURE);
            }
        }
        Command::Mcp => ctx_mcp::serve_stdio(open(&home)?)?,
        Command::Daemon { action, addr } => match action {
            Some(DaemonCmd::Token) => println!("{}", ctx_daemon::ensure_token()?),
            None => {
                let app = App::open(home.clone(), None)?;
                let h = home.clone();
                ctx_daemon::spawn_idle_sync(move || App::open(h.clone(), None));
                ctx_daemon::ensure_token()?;
                println!("browser extension token: run `ctx daemon token` to print it");
                ctx_daemon::serve(app, &addr)?;
            }
        },
        Command::Hook(HookCmd::SessionStart) => session_start(&home)?,
    }
    Ok(ExitCode::SUCCESS)
}

fn read_stdin() -> Result<String> {
    let mut buf = String::new();
    io::stdin()
        .read_to_string(&mut buf)
        .context("reading claim from stdin")?;
    Ok(buf)
}

/// `[c:7f2a]` from a full cid.
fn tag(cid: &str) -> String {
    let hex = cid.strip_prefix("b3:").unwrap_or(cid);
    format!("[c:{}]", &hex[..hex.len().min(4)])
}

fn print_claim(c: &Claim, verbose: bool, show_branch: bool) {
    let more = if c.text.contains('\n') { " …" } else { "" };
    let status = if c.status == Status::Active {
        String::new()
    } else {
        format!(" ({})", c.status)
    };
    let branch = if show_branch && c.branch.as_str() != BranchRef::DEFAULT {
        format!("  · {}", c.branch)
    } else {
        String::new()
    };
    println!(
        "{} {}  {:<10}  {}{more}{status}{branch}",
        c.t_tx.format("%Y-%m-%d"),
        tag(&c.cid),
        c.kind,
        first_line(&c.text),
    );
    if verbose {
        let pad = " ".repeat(20);
        for line in c.text.lines().skip(1) {
            println!("{pad}{line}");
        }
        if let Some(why) = &c.why {
            println!("{pad}why:  {}", why.replace('\n', " "));
        }
        if !c.refs.is_empty() {
            println!("{pad}refs: {}", c.refs.join(", "));
        }
        if !c.entities.is_empty() {
            println!("{pad}tags: {}", c.entities.join(", "));
        }
        println!(
            "{pad}{} · {} · {} · {} · from {}",
            c.id, c.branch, c.status, c.confidence, c.src
        );
    }
}

fn repo_root(start: &Path) -> PathBuf {
    start
        .ancestors()
        .find(|d| d.join(".git").exists())
        .unwrap_or(start)
        .to_owned()
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_owned();
    if out.is_empty() {
        "project".into()
    } else {
        out
    }
}

fn init(
    home: &CtxHome,
    project: Option<String>,
    branch: Option<String>,
    agents: &[String],
    no_agents: bool,
) -> Result<()> {
    let root = repo_root(&cwd());
    let existing = Binding::discover(&root)?.filter(|b| b.root == root);
    let project = slug(
        &project
            .or_else(|| existing.as_ref().map(|b| b.project.clone()))
            .unwrap_or_else(|| {
                root.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            }),
    );
    let branch = slug(
        &branch
            .or_else(|| existing.as_ref().map(|b| b.branch.clone()))
            .unwrap_or_else(|| "code".into()),
    );
    let created_store = !home.exists();

    let binding = Binding {
        project: project.clone(),
        branch: branch.clone(),
        root: root.clone(),
    };
    if existing.as_ref().map(|b| (&b.project, &b.branch)) != Some((&project, &branch)) {
        binding.write(&root)?;
    }
    let mut app = App::open(home.clone(), Some(&root))?;
    app.ensure_project(&project, &branch)?;
    let _ = ctx_daemon::ensure_token();
    app.refresh_agents_md()?;

    println!("ContextOS: {project}/{branch}");
    if created_store {
        println!("  created store at {}", home.root().display());
    }
    println!("  ✓ .ctx.yaml   binds this repo to {project}/{branch} (commit it)");
    println!("  ✓ AGENTS.md   ContextOS section added; your own content is kept (commit it)");

    if !no_agents {
        let user_home = std::env::home_dir().unwrap_or_default();
        let chosen: Vec<Agent> = if agents.is_empty() {
            Agent::ALL
                .into_iter()
                .filter(|a| a.detect(&user_home))
                .collect()
        } else {
            agents
                .iter()
                .map(|a| {
                    Agent::parse(a).with_context(|| {
                        format!("unknown agent `{a}` (claude-code, codex, cursor, gemini)")
                    })
                })
                .collect::<Result<_>>()?
        };
        let (command, on_path) = ctx_wire::ctx_command();
        if chosen.is_empty() {
            println!("  · no agents detected; AGENTS.md alone works with most of them");
        }
        for agent in chosen {
            println!("  {agent}:");
            for step in ctx_wire::wire(agent, &root, &user_home, &command) {
                println!("    {}", step.to_string().replace('\n', "\n    "));
            }
        }
        if !on_path {
            println!(
                "\n  note: `ctx` isn't on your PATH, so configs point at {command}\n        \
                 put ctx on PATH (see README) and re-run `ctx init` before committing them"
            );
        }
    }

    println!("\nNext:");
    println!("  ctx save \"Use SQLite, not Postgres\" -k decision -w \"no server to run\"");
    println!("  ctx pack          # see exactly what your agents will see");
    if ctx_git::sync::remote_url(home).is_none() {
        println!(
            "  sync across machines: create a PRIVATE git repo, then\n    \
             git -C \"{}\" remote add origin <url> && ctx sync",
            home.root().display()
        );
    }
    Ok(())
}

fn status(home: &CtxHome) -> Result<()> {
    let app = open(home)?;
    let stats = app.store.stats()?;
    println!("store    {}", home.root().display());
    println!(
        "remote   {}",
        ctx_git::sync::remote_url(home).unwrap_or_else(|| "none (local only)".into())
    );
    match &app.binding {
        Some(b) => println!("repo     {} → {}", b.root.display(), b.branch_ref()?),
        None => println!("repo     not bound (run `ctx init` in a project)"),
    }
    println!("branch   {}", app.default_branch()?);
    println!("claims   {} total", stats.claims);
    for (k, n) in &stats.by_kind {
        println!("         {n:>5} {k}");
    }
    let pending = app.proposals(None)?.len();
    if pending > 0 {
        println!("review   {pending} pending proposal(s): `ctx review`");
    }
    Ok(())
}

fn review(home: &CtxHome, action: Option<ReviewCmd>) -> Result<()> {
    let mut app = open(home)?;
    match action {
        None => {
            let pending = app.proposals(None)?;
            if pending.is_empty() {
                println!("no pending proposals");
                return Ok(());
            }
            for c in &pending {
                println!(
                    "{}  {} → {}  (from {})",
                    tag(&c.cid),
                    c.kind,
                    c.branch,
                    c.src
                );
                println!("    {}", c.text.replace('\n', "\n    "));
                if let Some(w) = &c.why {
                    println!("    why: {w}");
                }
            }
            println!(
                "\naccept: ctx review accept c:XXXX    reject: ctx review reject c:XXXX --reason \"...\""
            );
        }
        Some(ReviewCmd::Accept { id }) => {
            let c = app.find_one(&id)?;
            if c.status != Status::Proposed {
                bail!("{} is {}, not proposed", tag(&c.cid), c.status);
            }
            app.transition(&c, Status::Active, None, "review")?;
            println!("accepted {} into {}", tag(&c.cid), c.branch);
        }
        Some(ReviewCmd::Reject { id, reason }) => {
            let c = app.find_one(&id)?;
            if c.status != Status::Proposed {
                bail!("{} is {}, not proposed", tag(&c.cid), c.status);
            }
            app.transition(&c, Status::Rejected, Some(reason), "review")?;
            println!("rejected {} (the reason is kept)", tag(&c.cid));
        }
    }
    Ok(())
}

fn branch_cmd(home: &CtxHome, cmd: BranchCmd) -> Result<()> {
    let mut app = open(home)?;
    match cmd {
        BranchCmd::New {
            name,
            parent,
            template,
            inherits,
        } => {
            let b = app.resolve_branch(Some(&name))?;
            if !b.as_str().contains('/') {
                bail!(
                    "branches live in a project: use `project/{name}`, or run inside a bound repo"
                );
            }
            let inherits = inherits
                .iter()
                .map(|s| ctx_app::parse_inherits(s))
                .collect::<Result<Vec<_>>>()?;
            app.branch_new(&b, parent.as_deref(), template.as_deref(), &inherits)?;
            println!("created {b}");
        }
        BranchCmd::Ls => {
            let counts = app.store.branches()?;
            let active = app.default_branch()?;
            let mut names: Vec<String> = app.branches.all().into_iter().map(String::from).collect();
            for c in &counts {
                if !names.contains(&c.branch) {
                    names.push(c.branch.clone());
                }
            }
            names.sort();
            if names.is_empty() {
                println!("no branches yet: `ctx init` in a project creates research and code");
            }
            for n in names {
                let r = BranchRef::new(&n)?;
                let def = app.branches.get(&r);
                let c = counts.iter().find(|c| c.branch == n);
                let marker = if r == active { "*" } else { " " };
                let parent = def
                    .and_then(|d| d.parent.as_deref())
                    .map(|p| format!("  (inherits from {p})"))
                    .unwrap_or_default();
                let archived = if def.is_some_and(|d| d.archived) {
                    "  [archived]"
                } else {
                    ""
                };
                let last = c
                    .and_then(|c| c.last_tx)
                    .map(|t| t.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "-".into());
                println!(
                    "{marker} {n:<28} {:>5} claims {:>3} proposed   last {last}{parent}{archived}",
                    c.map(|c| c.active).unwrap_or(0),
                    c.map(|c| c.proposed).unwrap_or(0),
                );
            }
        }
        BranchCmd::Merge { src, into } => {
            let s = app.resolve_branch(Some(&src))?;
            let d = app.resolve_branch(Some(&into))?;
            let (merged, skipped) = app.merge(&s, &d)?;
            println!(
                "merged {merged} claim(s) from {s} into {d} ({skipped} skipped: already there or not held)"
            );
        }
        BranchCmd::Archive { name } => {
            let b = app.resolve_branch(Some(&name))?;
            let n = app.branch_archive(&b)?;
            println!("archived {b} ({n} claims flipped; nothing deleted)");
        }
    }
    Ok(())
}

/// ACE-style grow-and-refine without a model: surface near-duplicates so a
/// human can supersede one with the other.
fn refine(home: &CtxHome, branch: Option<String>, threshold: f64) -> Result<()> {
    use ctx_pack::select::{jaccard, word_set};
    let app = open(home)?;
    let b = app.resolve_branch(branch.as_deref())?;
    let claims = app.store.scan(&Filter {
        branch: Some(b.clone()),
        statuses: vec![Status::Active],
        ..Default::default()
    })?;
    let words: Vec<Vec<u64>> = claims.iter().map(|c| word_set(&c.text)).collect();
    let mut pairs = Vec::new();
    for i in 0..claims.len() {
        for j in i + 1..claims.len() {
            let s = jaccard(&words[i], &words[j]);
            if s >= threshold {
                pairs.push((s, i, j));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0));
    if pairs.is_empty() {
        println!("no near-duplicates on {b} at similarity ≥ {threshold}");
    }
    for (s, i, j) in pairs.iter().take(30) {
        let (older, newer) = (&claims[*i], &claims[*j]);
        println!("{:.0}% similar", s * 100.0);
        println!("  {} {}", tag(&older.cid), first_line(&older.text));
        println!("  {} {}", tag(&newer.cid), first_line(&newer.text));
        println!(
            "  merge: ctx save \"<combined>\" --supersedes {}   then: ctx archive {}",
            newer.id, older.id
        );
    }
    Ok(())
}

/// SessionStart hook: bounded pull, refresh AGENTS.md, and print the new
/// context only if it changed after the agent already loaded AGENTS.md.
/// Injecting only at session start keeps the prompt prefix stable, so
/// prompt caching keeps working (spec §12.2).
fn session_start(home: &CtxHome) -> Result<()> {
    if !home.exists() {
        return Ok(());
    }
    let mut app = open(home)?;
    if app.binding.is_none() {
        return Ok(());
    }
    if let Err(e) = app.pull_quick(Duration::from_millis(1500)) {
        eprintln!("ctx: pull skipped ({e})");
    }
    if let Some(block) = app.refresh_agents_md()? {
        println!(
            "ContextOS: this project's context changed since AGENTS.md was loaded. Current version:\n\n{block}"
        );
    }
    Ok(())
}
