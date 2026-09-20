//! The idea-to-build flow and the project map.
//!
//! ```text
//! ctx new my-idea            start an idea; chat surfaces now write to my-idea/research
//!   ... research in claude.ai / ChatGPT (Worker, extension or clipboard) ...
//! "ctx spec" in the chat     the model writes the spec; saved as the project's spec
//! ctx build my-idea          repo + SPEC.md + AGENTS.md + agent wiring; then run `claude`
//! ```

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use ctx_app::{App, SPEC, SPEC_FILE, SpecFile};
use ctx_core::{BranchRef, Claim, Filter, Status, Store};
use ctx_git::CtxHome;

/// `ctx new <idea>`
pub fn new(home: &CtxHome, name: &str) -> Result<()> {
    let project = crate::slug(name);
    let mut app = App::open(home.clone(), None)?;
    let research = app.new_project(&project)?;
    crate::sync_now(&mut app, false)?;
    println!("Started `{project}`. Research now goes to {research}.\n");
    println!("Where to do the research:");
    println!("  claude.ai / ChatGPT   with the ContextOS connector (worker/README.md), just talk;");
    println!(
        "                        say \"save that\" to record findings, \"ctx spec\" when you're done"
    );
    println!(
        "  any other chat        ctx pack research --clip, paste, and later: ctx save --paste"
    );
    println!("  terminal              ctx save \"...\" -k fact     (lands in {research})");
    println!("\nWhen the spec is saved:");
    println!(
        "  ctx build {project}      creates ./{project} with the spec and wires your coding agent"
    );
    Ok(())
}

/// `ctx spec save`: a spec from a file, stdin or the clipboard.
pub fn save(
    app: &mut App,
    branch: &BranchRef,
    text: &str,
    name: &str,
    title: Option<&str>,
    unwrap_fence: bool,
) -> Result<()> {
    let body = if unwrap_fence {
        ctx_app::docs::parse_spec_block(text)?
    } else {
        text.to_owned()
    };
    let saved = app.save_doc(branch, name, title, &body, "cli")?;
    if saved.duplicate {
        println!("`{}` on {branch} is unchanged", saved.doc.name);
    } else {
        println!(
            "saved `{}` on {branch}: \"{}\" (~{} tokens)",
            saved.doc.name,
            saved.doc.title,
            ctx_core::tokens::estimate(&saved.doc.body)
        );
    }
    if let Ok(SpecFile::Written) = app.refresh_spec_file() {
        println!("updated {SPEC_FILE}");
    }
    Ok(())
}

pub fn show(app: &App, branch: &BranchRef, name: &str) -> Result<()> {
    match app.find_doc(branch, name)? {
        Some(d) => {
            print!("{}", d.body);
            if !d.body.ends_with('\n') {
                println!();
            }
            Ok(())
        }
        None => {
            bail!("no `{name}` for {branch} yet: save one with `ctx spec save FILE` (or --paste)")
        }
    }
}

pub fn list(app: &App, branch: &BranchRef) -> Result<()> {
    let docs = app.visible_docs(branch)?;
    if docs.is_empty() {
        println!("no documents for {branch} yet");
    }
    for d in docs {
        println!(
            "{:<12} {}  (~{} tokens, {}, updated {})",
            d.name,
            d.title,
            ctx_core::tokens::estimate(&d.body),
            d.branch,
            d.t_tx.format("%Y-%m-%d")
        );
    }
    Ok(())
}

/// `ctx build <idea> [dir]`: hand the idea to a coding agent.
pub fn build(
    home: &CtxHome,
    name: &str,
    dir: Option<&Path>,
    agents: &[String],
    no_agents: bool,
) -> Result<()> {
    let project = crate::slug(name);
    {
        // Research done in the browser lands in the cloud shard; fetch it
        // first so the spec written below is the latest one.
        let mut app = App::open(home.clone(), None)?;
        if let Err(e) = app.pull_quick(Duration::from_secs(10)) {
            eprintln!(
                "note: could not pull the latest research ({e}); using what's on this machine"
            );
        }
        let code = BranchRef::new(&format!("{project}/code"))?;
        if app.find_doc(&code, SPEC)?.is_none() {
            eprintln!(
                "note: `{project}` has no spec yet; building from research claims only.\n      \
                 Save one with \"ctx spec\" in your chat, or `ctx spec save FILE --to {project}/research`."
            );
        }
    }
    let dir = dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| crate::cwd().join(&project));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    if !dir.join(".git").exists() && ctx_wire::which("git").is_some() {
        let _ = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&dir)
            .status();
    }
    crate::init(
        home,
        Some(project.clone()),
        Some("code".into()),
        agents,
        no_agents,
        &dir,
    )?;
    println!(
        "\nReady. Start building:\n  cd {}\n  claude     (or codex, cursor, gemini)",
        dir.display()
    );
    println!("Tell it: \"Build this from {SPEC_FILE}.\"");
    Ok(())
}

fn node_label(c: &Claim) -> String {
    let mut t: String = c
        .text
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(48)
        .collect();
    if c.text.chars().count() > 48 || c.text.contains('\n') {
        t.push('…');
    }
    // Mermaid label escaping: quotes and angle brackets as entities.
    let t = t
        .replace('"', "#quot;")
        .replace('<', "#lt;")
        .replace('>', "#gt;");
    format!("{}: {t}", c.kind)
}

/// `ctx map`: a metro-style Mermaid map of a project. Each branch is a
/// line, claims are stations in the order they were recorded, colour shows
/// the kind, dashed arrows show inheritance and supersession. GitHub and
/// most markdown viewers render it directly.
pub fn map(app: &App, project: Option<&str>, per_branch: usize) -> Result<String> {
    let project = match project {
        Some(p) => crate::slug(p),
        None => app
            .default_branch()?
            .as_str()
            .split_once('/')
            .map(|(p, _)| p.to_owned())
            .context("no project here: pass one, e.g. `ctx map my-idea`")?,
    };
    let prefix = format!("{project}/");
    let mut branches: BTreeSet<String> = app
        .branches
        .all()
        .into_iter()
        .map(String::from)
        .filter(|b| b.starts_with(&prefix))
        .collect();
    for s in app.store.branches()? {
        if s.branch.starts_with(&prefix) {
            branches.insert(s.branch);
        }
    }
    if branches.is_empty() {
        bail!("no branches for `{project}`");
    }

    let mut out = String::from("```mermaid\nflowchart LR\n");
    let mut ids: std::collections::HashMap<ulid::Ulid, String> = Default::default();
    let mut edges = String::new();
    let mut n = 0usize;
    for (bi, b) in branches.iter().enumerate() {
        let r = BranchRef::new(b)?;
        let mut claims = app.store.scan(&Filter {
            branch: Some(r.clone()),
            statuses: vec![
                Status::Active,
                Status::Superseded,
                Status::Proposed,
                Status::Rejected,
            ],
            limit: Some(per_branch),
            ..Default::default()
        })?;
        claims.sort_by_key(|c| c.id);
        let _ = writeln!(out, "  subgraph b{bi}[\"{b}\"]\n    direction LR");
        if claims.is_empty() {
            let _ = writeln!(out, "    e{bi}([\"no claims yet\"]):::empty");
        }
        let mut prev: Option<String> = None;
        for c in &claims {
            let id = format!("n{n}");
            n += 1;
            let class = match c.status {
                Status::Superseded => "superseded".to_owned(),
                Status::Proposed => "proposed".to_owned(),
                Status::Rejected => "superseded".to_owned(),
                _ => c.kind.to_string(),
            };
            let _ = writeln!(out, "    {id}[\"{}\"]:::{class}", node_label(c));
            if let Some(p) = &prev {
                let _ = writeln!(out, "    {p} --- {id}");
            }
            prev = Some(id.clone());
            ids.insert(c.id, id);
        }
        out.push_str("  end\n");
        if let Some(def) = app.branches.get(&r) {
            for (from, kinds) in &def.inherits {
                let full = if from.contains('/') {
                    from.clone()
                } else {
                    format!("{project}/{from}")
                };
                if let Some(fi) = branches.iter().position(|x| *x == full) {
                    let kinds: Vec<&str> = kinds.iter().map(|k| k.as_str()).collect();
                    let _ = writeln!(edges, "  b{fi} -. \"{}\" .-> b{bi}", kinds.join(", "));
                }
            }
        }
    }
    // Supersession edges between claims that are both on the map.
    for b in &branches {
        for c in app.store.scan(&Filter {
            branch: Some(BranchRef::new(b)?),
            ..Default::default()
        })? {
            if let (Some(old), Some(new)) = (c.supersedes.and_then(|o| ids.get(&o)), ids.get(&c.id))
            {
                let _ = writeln!(edges, "  {new} -. supersedes .-> {old}");
            }
        }
    }
    out.push_str(&edges);
    out.push_str(
        "  classDef decision fill:#dbeafe,stroke:#1d4ed8,color:#0b1b3a\n\
         \x20 classDef constraint fill:#ffedd5,stroke:#c2410c,color:#3a1a05\n\
         \x20 classDef rejected fill:#fee2e2,stroke:#b91c1c,color:#3a0b0b\n\
         \x20 classDef fact fill:#f1f5f9,stroke:#475569,color:#0f172a\n\
         \x20 classDef question fill:#fef9c3,stroke:#a16207,color:#3a2a05\n\
         \x20 classDef claim fill:#ede9fe,stroke:#6d28d9,color:#1e0b3a\n\
         \x20 classDef superseded fill:#f8fafc,stroke:#94a3b8,color:#64748b,stroke-dasharray:4 3\n\
         \x20 classDef proposed fill:#ffffff,stroke:#0d9488,color:#134e4a,stroke-dasharray:2 2\n\
         \x20 classDef empty fill:#ffffff,stroke:#cbd5e1,color:#94a3b8\n",
    );
    out.push_str("```\n");
    Ok(out)
}
