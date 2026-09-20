//! The RecurOS service layer.
//!
//! Every surface — the `ctx` CLI, the MCP server, the local HTTP daemon —
//! performs the same operations: route to a branch, save, propose, compile a
//! pack, sync. They live here once, so the five MCP tools and the CLI can
//! never drift apart.

pub mod agents_md;
pub mod claims_block;
pub mod docs;

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use ctx_branch::{Binding, BranchConfig, BranchDef, PoolSource};
use ctx_core::{
    BranchRef, Claim, ClaimDraft, CounterUpdate, Filter, Kind, Record, Status, StatusChange, Store,
    merkle,
};
use ctx_git::{CtxHome, shard, sync};
use ctx_pack::{IndexEntry, Pack, Projection, Weights};
use ctx_store_sqlite::SqliteStore;
use ulid::Ulid;

pub use claims_block::ClaimInput;
pub use docs::{DocSaved, SPEC, SPEC_FILE, SpecFile};

/// The command reference written to `.ctx/commands.md`, so agents can run
/// `ctx` for the user. Also printed by `ctx commands`.
pub const COMMANDS_MD: &str = include_str!("commands.md");

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Retrieval depth for task-focused packs (spec §8.3).
const RETRIEVE_TOP: usize = 200;

/// What a claim the search did not rank is worth. Low enough that anything
/// it did rank wins, high enough that weight (a constraint, a pin, a recent
/// decision) can still buy a place with leftover budget.
const RELEVANCE_FLOOR: f64 = 0.02;

/// Ranks at which relevance halves.
const RELEVANCE_HALF_LIFE: f64 = 8.0;

/// Turn a position in the retrieved list into a score in (0, 1].
///
/// Fusion already happened in the store; what is left is giving one ordered
/// list a usable scale. Reciprocal rank's own `1 / (60 + rank)` curve is far
/// too flat for that: it puts the fortieth claim at a quarter of the first,
/// a gap that a claim's weight alone can overturn, which is how a task ended
/// up barely changing which claims were packed.
fn relevance_at(rank: usize) -> f64 {
    0.5_f64.powf(rank as f64 / RELEVANCE_HALF_LIFE)
}

/// How many of the best hits to expand from. Expanding the whole result would
/// pull in the graph around claims that only matched a common word.
const EXPAND_SEEDS: usize = 12;

pub struct App {
    pub home: CtxHome,
    pub store: SqliteStore,
    pub branches: BranchConfig,
    pub weights: Weights,
    /// `.ctx.yaml` found from the working directory, if any.
    pub binding: Option<Binding>,
    machine: String,
}

/// What [`App::rename_project`] carried over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Renamed {
    pub branches: usize,
    pub claims: usize,
    pub docs: usize,
}

/// The lane half of `project/lane`.
fn lane_of(b: &BranchRef) -> &str {
    b.as_str().split_once('/').map_or("code", |(_, lane)| lane)
}

#[derive(Debug)]
pub enum SaveOutcome {
    Saved(Claim),
    /// Identical content already exists on this branch; nothing was written.
    Duplicate(Claim),
}

impl SaveOutcome {
    pub fn claim(&self) -> &Claim {
        match self {
            SaveOutcome::Saved(c) | SaveOutcome::Duplicate(c) => c,
        }
    }
}

#[derive(Debug, Default, serde::Serialize)]
pub struct BatchReport {
    pub saved: Vec<Saved>,
    pub duplicates: Vec<Saved>,
    pub errors: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct Saved {
    pub id: String,
    pub cid: String,
}

/// Options for compiling a pack.
#[derive(Debug, Clone, Default)]
pub struct PackOpts {
    pub branch: Option<String>,
    pub task: Option<String>,
    pub budget: Option<u32>,
    pub projection: Option<Projection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyResult {
    InSync,
    /// The given root is an older state of ours: its holder is behind by n claims.
    Behind(usize),
    /// Not a state we have seen: the other side is ahead of us or diverged.
    AheadOrDiverged,
}

fn now() -> (SystemTime, DateTime<Utc>) {
    let sys = SystemTime::now();
    (sys, sys.into())
}

impl App {
    /// Open the store at `home` (creating it on first use), catch the index
    /// up with the log, and discover `.ctx.yaml` from `cwd`.
    pub fn open(home: CtxHome, cwd: Option<&Path>) -> Result<App> {
        home.ensure()
            .with_context(|| format!("initialising {}", home.root().display()))?;
        let machine = home.machine_id()?;
        let store = SqliteStore::open(&home.db_path())
            .with_context(|| format!("opening index {}", home.db_path().display()))?;
        let branches = BranchConfig::load(&home.refs_dir().join("branches.yaml"))?;
        let weights =
            Weights::load(&home.refs_dir().join("weights.yaml")).map_err(anyhow::Error::msg)?;
        let binding = match cwd {
            Some(dir) => Binding::discover(dir)?,
            None => None,
        };
        // Repos bound before `.ctx/` existed move their binding into it the
        // first time any command runs there.
        if let Some(b) = &binding
            && b.legacy
        {
            b.write(&b.root)?;
        }
        let mut app = App {
            home,
            store,
            branches,
            weights,
            binding,
            machine,
        };
        app.catch_up()?;
        Ok(app)
    }

    pub fn machine(&self) -> &str {
        &self.machine
    }

    fn branches_path(&self) -> PathBuf {
        self.home.refs_dir().join("branches.yaml")
    }

    // ---- log ⇄ index ------------------------------------------------------

    /// Index any log bytes the cache hasn't seen: our own appends, or other
    /// machines' shards that arrived via `git pull`. Costs a few `stat`s when
    /// nothing changed. Bad lines are warned about on stderr, never fatal.
    pub fn catch_up(&mut self) -> Result<()> {
        for rel in shard::list_shards(&self.home)? {
            let offset = self.store.cursor(&rel)?;
            let out = shard::read_from(&self.home, &rel, offset)?;
            if out.shrunk {
                eprintln!("warning: {rel} shrank since it was indexed; rebuilding the index");
                return self.reindex();
            }
            for w in &out.warnings {
                eprintln!("warning: {w}");
            }
            if out.next_offset != offset {
                self.store.ingest(&rel, &out.records, out.next_offset)?;
            }
        }
        Ok(())
    }

    /// Drop the index and rebuild it from the log.
    pub fn reindex(&mut self) -> Result<()> {
        self.store.rebuild()?;
        self.catch_up()
    }

    fn append(&mut self, record: Record, at: DateTime<Utc>) -> Result<()> {
        shard::append(&self.home, &self.machine, &record, at)?;
        self.catch_up()
    }

    // ---- routing (spec §7.4: declared, never inferred) --------------------

    fn active_file(&self) -> PathBuf {
        self.home.refs_dir().join("active")
    }

    /// The branch chat surfaces use, set by `ctx use`.
    pub fn active_ref(&self) -> Option<BranchRef> {
        let s = fs::read_to_string(self.active_file()).ok()?;
        BranchRef::new(s.trim()).ok()
    }

    /// Default branch for this invocation: the repo's `.ctx.yaml` binding,
    /// then `refs/active`, then `default`.
    pub fn default_branch(&self) -> Result<BranchRef> {
        if let Some(b) = &self.binding {
            return Ok(b.branch_ref()?);
        }
        Ok(self.active_ref().unwrap_or_else(BranchRef::default_branch))
    }

    fn current_project(&self) -> Option<String> {
        if let Some(b) = &self.binding {
            return Some(b.project.clone());
        }
        self.active_ref()
            .and_then(|r| r.as_str().split_once('/').map(|(p, _)| p.to_owned()))
    }

    /// Resolve a user-typed branch name. `research` means `research` in the
    /// current project; `other/x` is taken literally; `None` is the default.
    pub fn resolve_branch(&self, name: Option<&str>) -> Result<BranchRef> {
        let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else {
            return self.default_branch();
        };
        if name.contains('/') || name == BranchRef::DEFAULT {
            return Ok(BranchRef::new(name)?);
        }
        match self.current_project() {
            Some(p) => Ok(BranchRef::new(&format!("{p}/{name}"))?),
            None => {
                // No project context: accept a bare name only if it is
                // unambiguous among defined branches.
                let matches: Vec<BranchRef> = self
                    .branches
                    .all()
                    .into_iter()
                    .filter(|b| b.as_str().rsplit('/').next() == Some(name))
                    .collect();
                match matches.as_slice() {
                    [one] => Ok(one.clone()),
                    [] => Ok(BranchRef::new(name)?),
                    many => bail!(
                        "`{name}` is ambiguous: {}",
                        many.iter()
                            .map(|b| b.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                }
            }
        }
    }

    /// `ctx use`: set the branch chat surfaces default to.
    pub fn set_active(&mut self, branch: &BranchRef) -> Result<()> {
        fs::create_dir_all(self.home.refs_dir())?;
        fs::write(self.active_file(), format!("{branch}\n"))?;
        Ok(())
    }

    // ---- writing ------------------------------------------------------------

    /// Record a claim. No network and no model call (invariants 4 and 7).
    /// Rejects kinds the branch doesn't hold, and dedups by content address.
    pub fn save(&mut self, draft: ClaimDraft) -> Result<SaveOutcome> {
        self.branches.check_holds(&draft.branch, draft.kind)?;
        if let Some(old) = draft.supersedes
            && self.store.get(old)?.is_none()
        {
            bail!("cannot supersede {old}: no such claim");
        }
        let (sys, at) = now();
        let claim = Claim::from_draft(draft, Ulid::from_datetime(sys), at)?;
        if let Some(existing) = self.store.find_by_cid(&claim.cid, &claim.branch)?
            && existing.status != Status::Archived
        {
            return Ok(SaveOutcome::Duplicate(existing));
        }
        self.append(Record::Claim(claim.clone()), at)?;
        Ok(SaveOutcome::Saved(claim))
    }

    /// Save a batch from an external surface (extension, clipboard, MCP).
    pub fn save_inputs(
        &mut self,
        inputs: &[ClaimInput],
        branch: &BranchRef,
        src: &str,
    ) -> BatchReport {
        let mut report = BatchReport::default();
        for input in inputs {
            let result = input.kind().and_then(|kind| {
                let mut d = ClaimDraft::new(kind, input.text.clone());
                d.branch = branch.clone();
                d.why = input.why.clone();
                d.refs = input.refs.clone();
                d.entities = input.entities.clone();
                d.src = src.to_owned();
                self.save(d)
            });
            match result {
                Ok(SaveOutcome::Saved(c)) => report.saved.push(Saved {
                    id: c.id.to_string(),
                    cid: c.cid,
                }),
                Ok(SaveOutcome::Duplicate(c)) => report.duplicates.push(Saved {
                    id: c.id.to_string(),
                    cid: c.cid,
                }),
                Err(e) => report.errors.push(format!(
                    "{}: {e:#}",
                    input.text.chars().take(60).collect::<String>()
                )),
            }
        }
        report
    }

    /// Cross-branch write (spec §7.5): lands on `target` as `proposed` and
    /// stays out of packs until accepted with `ctx review`.
    pub fn propose(&mut self, mut draft: ClaimDraft) -> Result<SaveOutcome> {
        self.branches.check_holds(&draft.branch, draft.kind)?;
        let (sys, at) = now();
        draft.supersedes = None;
        let mut claim = Claim::from_draft(draft, Ulid::from_datetime(sys), at)?;
        claim.status = Status::Proposed;
        if let Some(existing) = self.store.find_by_cid(&claim.cid, &claim.branch)? {
            return Ok(SaveOutcome::Duplicate(existing));
        }
        self.append(Record::Claim(claim.clone()), at)?;
        Ok(SaveOutcome::Saved(claim))
    }

    /// Find exactly one claim by id / cid prefix (`01K3…`, `c:7f2a`).
    pub fn find_one(&self, prefix: &str) -> Result<Claim> {
        let hits = self.store.resolve(prefix)?;
        match hits.as_slice() {
            [one] => Ok(one.clone()),
            [] => bail!("no claim matches `{prefix}`"),
            many => bail!(
                "`{prefix}` matches {} claims; use more characters:\n{}",
                many.len(),
                many.iter()
                    .map(|c| format!("  {} [c:{}] {}", c.id, c.short_cid(), first_line(&c.text)))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        }
    }

    /// Move a claim along the status lattice. Deletion is always this, never
    /// removal (invariant 10).
    pub fn transition(
        &mut self,
        claim: &Claim,
        to: Status,
        reason: Option<String>,
        src: &str,
    ) -> Result<()> {
        self.status_record(claim.id, to, reason, src)
    }

    /// Append a status record for any record id (a claim, or a document
    /// version: archiving a document hides it the same way).
    fn status_record(
        &mut self,
        target: Ulid,
        to: Status,
        reason: Option<String>,
        src: &str,
    ) -> Result<()> {
        let (sys, at) = now();
        let rec = StatusChange {
            id: Ulid::from_datetime(sys),
            claim: target,
            to,
            reason,
            src: src.to_owned(),
            t_tx: at,
        };
        self.append(Record::Status(rec), at)
    }

    pub fn proposals(&self, branch: Option<&BranchRef>) -> Result<Vec<Claim>> {
        Ok(self.store.scan(&Filter {
            branch: branch.cloned(),
            statuses: vec![Status::Proposed],
            ..Default::default()
        })?)
    }

    /// Bump this machine's helpful/harmful G-counter entry for a claim.
    pub fn rate(&mut self, claim: &Claim, helpful: bool) -> Result<()> {
        let (sys, at) = now();
        let mut h = claim.helpful.get(&self.machine).copied().unwrap_or(0);
        let mut x = claim.harmful.get(&self.machine).copied().unwrap_or(0);
        if helpful {
            h += 1;
        } else {
            x += 1;
        }
        let rec = CounterUpdate {
            id: Ulid::from_datetime(sys),
            claim: claim.id,
            machine: self.machine.clone(),
            helpful: h,
            harmful: x,
            t_tx: at,
        };
        self.append(Record::Counter(rec), at)
    }

    // ---- branches -------------------------------------------------------------

    pub fn save_branches(&self) -> Result<()> {
        self.branches.save(&self.branches_path())?;
        Ok(())
    }

    /// Create a branch from a template (or empty), validating the DAG.
    pub fn branch_new(
        &mut self,
        branch: &BranchRef,
        parent: Option<&str>,
        template: Option<&str>,
        inherits: &[(String, Vec<Kind>)],
    ) -> Result<()> {
        let parent_name = parent.map(|p| p.rsplit('/').next().unwrap_or(p).to_owned());
        let mut def = match template {
            Some(t) => ctx_branch::template(t, parent_name.as_deref()).with_context(|| {
                format!(
                    "unknown template `{t}` (expected one of: {})",
                    ctx_branch::TEMPLATES.join(", ")
                )
            })?,
            None => BranchDef {
                parent: parent_name.clone(),
                ..Default::default()
            },
        };
        for (from, kinds) in inherits {
            def.inherits.insert(from.clone(), kinds.clone());
        }
        if parent_name.is_some() && !def.inherits.keys().any(|k| Some(k) == parent_name.as_ref()) {
            // Default ancestor scope: conclusions only (what was decided,
            // what must hold, and what was ruled out).
            def.inherits.insert(
                parent_name.clone().unwrap_or_default(),
                vec![Kind::Decision, Kind::Constraint, Kind::Rejected],
            );
        }
        self.branches.insert(branch, def)?;
        self.save_branches()
    }

    /// Ensure a project has the standard `research` → `code` pair; used by
    /// `ctx init` so a fresh repo gets a sensible DAG with zero config.
    pub fn ensure_project(&mut self, project: &str, code_branch: &str) -> Result<bool> {
        let research = BranchRef::new(&format!("{project}/research"))?;
        let code = BranchRef::new(&format!("{project}/{code_branch}"))?;
        let mut changed = false;
        if !self.branches.contains(&research) {
            self.branches.insert(
                &research,
                ctx_branch::template("research", None).context("template")?,
            )?;
            changed = true;
        }
        if !self.branches.contains(&code) && code != research {
            let def = ctx_branch::template("code", Some("research")).context("template")?;
            self.branches.insert(&code, def)?;
            changed = true;
        }
        if changed {
            self.save_branches()?;
        }
        Ok(changed)
    }

    /// Merge appends, it never moves (spec §7.5): each active claim on `src`
    /// that `dst` can hold is copied onto `dst` with a `ctx:<source id>` ref.
    /// The original stays where it was written, so this is reversible.
    pub fn merge(&mut self, src: &BranchRef, dst: &BranchRef) -> Result<(usize, usize)> {
        let dst_def = self.branches.get(dst).cloned();
        let already: BTreeSet<String> = self
            .store
            .scan(&Filter {
                branch: Some(dst.clone()),
                ..Default::default()
            })?
            .iter()
            .flat_map(|c| c.refs.iter().filter(|r| r.starts_with("ctx:")).cloned())
            .collect();
        let sources = self.store.scan(&Filter {
            branch: Some(src.clone()),
            statuses: vec![Status::Active],
            ..Default::default()
        })?;
        let (mut merged, mut skipped) = (0, 0);
        for c in sources {
            let marker = format!("ctx:{}", c.id);
            let held = dst_def.as_ref().is_none_or(|d| d.holds_kind(c.kind));
            if !held || already.contains(&marker) {
                skipped += 1;
                continue;
            }
            let mut d = ClaimDraft::new(c.kind, c.text.clone());
            d.branch = dst.clone();
            d.why = c.why.clone();
            d.refs = c.refs.iter().cloned().chain([marker]).collect();
            d.entities = c.entities.clone();
            d.confidence = c.confidence;
            d.src = "merge".into();
            d.t_valid = Some(c.t_valid);
            match self.save(d)? {
                SaveOutcome::Saved(_) => merged += 1,
                SaveOutcome::Duplicate(_) => skipped += 1,
            }
        }
        Ok((merged, skipped))
    }

    /// Archive a branch: flips its claims' status and marks it archived.
    /// Nothing is deleted (invariant 10).
    pub fn branch_archive(&mut self, branch: &BranchRef) -> Result<usize> {
        let claims = self.store.scan(&Filter {
            branch: Some(branch.clone()),
            ..Default::default()
        })?;
        let mut n = 0;
        for c in claims.iter().filter(|c| c.status != Status::Archived) {
            self.transition(
                c,
                Status::Archived,
                Some(format!("branch {branch} archived")),
                "cli",
            )?;
            n += 1;
        }
        if let Some(def) = self.branches.get_mut(branch) {
            def.archived = true;
        } else {
            // A branch can exist without ever being defined: saving from a
            // chat creates claims on a name nobody declared. With no
            // definition there was nowhere to record that it had been
            // deleted, so an emptied branch came back in every listing
            // forever. Declare it, archived, so the deletion sticks.
            self.branches.insert(
                branch,
                BranchDef {
                    holds: Kind::ALL.to_vec(),
                    archived: true,
                    ..BranchDef::default()
                },
            )?;
        }
        self.save_branches()?;
        Ok(n)
    }

    /// Branches belonging to `project`: defined in branches.yaml or holding
    /// any claim or document.
    pub fn project_branches(&self, project: &str) -> Result<Vec<BranchRef>> {
        let prefix = format!("{project}/");
        let mut names: BTreeSet<String> = self
            .branches
            .all()
            .into_iter()
            .map(String::from)
            .filter(|b| b.starts_with(&prefix))
            .collect();
        for s in self.store.branches()? {
            if s.branch.starts_with(&prefix) {
                names.insert(s.branch);
            }
        }
        for d in self.store.docs(None)? {
            if d.branch.as_str().starts_with(&prefix) {
                names.insert(d.branch.to_string());
            }
        }
        names.iter().map(|n| Ok(BranchRef::new(n)?)).collect()
    }

    /// What removing a branch would hide: (visible claims, document versions).
    pub fn removal_counts(&self, branch: &BranchRef) -> Result<(usize, usize)> {
        let claims = self
            .store
            .scan(&Filter {
                branch: Some(branch.clone()),
                ..Default::default()
            })?
            .into_iter()
            .filter(|c| c.status != Status::Archived)
            .count();
        Ok((claims, self.store.doc_versions(branch)?.len()))
    }

    /// `ctx remove <branch>`: hide every claim and document on a branch and
    /// mark it archived. Nothing is erased; it all stays in the log.
    pub fn remove_branch(&mut self, branch: &BranchRef) -> Result<(usize, usize)> {
        let claims = self.branch_archive(branch)?;
        let docs = self.store.doc_versions(branch)?;
        for d in &docs {
            self.status_record(
                d.id,
                Status::Archived,
                Some(format!("branch {branch} removed")),
                "cli",
            )?;
        }
        if self.active_ref().as_ref() == Some(branch) {
            self.set_active(&BranchRef::default_branch())?;
        }
        Ok((claims, docs.len()))
    }

    /// `ctx rename <old> <new>`: carry a project's context over to a new
    /// name, then retire the old one.
    ///
    /// A claim's branch is part of what it hashes, so a claim cannot change
    /// branch: renaming means re-recording every claim under the new name.
    /// The copies get new ids (and so new `c:` tags), keep their original
    /// `t_valid` so recency is unchanged, and keep their supersession chain,
    /// remapped to the new ids. The originals are archived, not erased: the
    /// old name stays readable in `ctx log --all`.
    pub fn rename_project(&mut self, old: &str, new: &str) -> Result<Renamed> {
        let old = old.trim();
        let new = new.trim();
        if old == new {
            bail!("`{old}` is already its name");
        }
        // Validate the new name the same way any branch name is validated.
        BranchRef::new(&format!("{new}/code"))?;
        if !self.project_branches(new)?.is_empty() {
            bail!("`{new}` already exists; remove it first or pick another name");
        }
        let branches = self.project_branches(old)?;
        if branches.is_empty() {
            bail!("no idea named `{old}`");
        }

        // The branch graph first, so `save` below can check what each lane
        // holds. The old definitions stay, and `remove_project` marks them
        // archived at the end.
        // Parents before children: each insert is validated on its own, and
        // a lane whose parent is not there yet is not a valid branch.
        let mut defs: Vec<(BranchRef, BranchDef)> = branches
            .iter()
            .filter_map(|b| {
                let def = self.branches.get(b).cloned()?;
                let target = BranchRef::new(&format!("{new}/{}", lane_of(b))).ok()?;
                Some((target, def))
            })
            .collect();
        defs.sort_by_key(|(_, def)| def.parent.is_some());
        for (target, def) in defs {
            self.branches.insert(&target, def)?;
        }
        self.save_branches()?;

        let mut moved: HashMap<Ulid, Ulid> = HashMap::new();
        let mut claims = 0usize;
        let mut docs = 0usize;
        for b in &branches {
            let target = BranchRef::new(&format!("{new}/{}", lane_of(b)))?;
            let mut pool = self.store.scan(&Filter {
                branch: Some(b.clone()),
                ..Default::default()
            })?;
            // Oldest first, so a claim's `supersedes` always points at one
            // that has already been re-recorded.
            pool.sort_by_key(|c| c.id);
            for c in pool {
                if c.status == Status::Archived {
                    continue; // already deleted; no reason to carry it over
                }
                let mut d = ClaimDraft::new(c.kind, c.text.clone());
                d.branch = target.clone();
                d.why = c.why.clone();
                d.refs = c.refs.clone();
                d.entities = c.entities.clone();
                d.confidence = c.confidence;
                d.src = c.src.clone();
                d.t_valid = Some(c.t_valid);
                d.supersedes = c.supersedes.and_then(|o| moved.get(&o).copied());
                let outcome = if c.status == Status::Proposed {
                    self.propose(d)?
                } else {
                    self.save(d)?
                };
                let fresh = outcome.claim().clone();
                moved.insert(c.id, fresh.id);
                // Superseded follows from the remapped chain; the rest is
                // carried over explicitly.
                if !matches!(
                    c.status,
                    Status::Active | Status::Proposed | Status::Superseded
                ) {
                    self.transition(&fresh, c.status, Some(format!("renamed from {b}")), "cli")?;
                }
                claims += 1;
            }
            // Only what this branch owns: `visible_docs` also shows the
            // documents it inherits, and copying those would give the new
            // project two of each.
            for doc in self.visible_docs(b)?.into_iter().filter(|d| d.branch == *b) {
                self.save_doc(&target, &doc.name, Some(&doc.title), &doc.body, &doc.src)?;
                docs += 1;
            }
        }

        self.remove_project(old)?;
        let active = BranchRef::new(&format!("{new}/{}", lane_of(&branches[0])))?;
        self.set_active(&active)?;
        Ok(Renamed {
            branches: branches.len(),
            claims,
            docs,
        })
    }

    /// `ctx remove <idea>`: remove every branch of a project.
    pub fn remove_project(&mut self, project: &str) -> Result<(usize, usize, usize)> {
        let branches = self.project_branches(project)?;
        let (mut claims, mut docs) = (0, 0);
        for b in &branches {
            let (c, d) = self.remove_branch(b)?;
            claims += c;
            docs += d;
        }
        Ok((branches.len(), claims, docs))
    }

    // ---- compiling --------------------------------------------------------------

    /// All claims branch `b` can see (spec §7.3), with effective statuses.
    pub fn pool(&self, b: &BranchRef) -> Result<Vec<Claim>> {
        let mut out = Vec::new();
        for PoolSource { branch, kinds } in self.branches.pool_sources(b) {
            out.extend(self.store.scan(&Filter {
                branch: Some(branch),
                kinds: kinds.unwrap_or_default(),
                statuses: vec![Status::Active, Status::Superseded],
                ..Default::default()
            })?);
        }
        Ok(out)
    }

    /// Compile a pack. Budget precedence: explicit, then the branch's own
    /// budget when compiling its configured projection, then the
    /// projection's default.
    pub fn pack(&self, opts: &PackOpts) -> Result<Pack> {
        let branch = self.resolve_branch(opts.branch.as_deref())?;
        let def = self.branches.get(&branch);
        let configured = def
            .and_then(|d| d.compile.as_deref())
            .and_then(|c| c.parse::<Projection>().ok());
        let projection = opts
            .projection
            .or(configured)
            .unwrap_or(Projection::AgentsMd);
        let budget = opts
            .budget
            .or_else(|| {
                def.filter(|_| configured == Some(projection))
                    .and_then(|d| d.budget)
            })
            .unwrap_or_else(|| projection.default_budget());

        let pool = self.pool(&branch)?;
        let relevance: Option<HashMap<Ulid, f64>> = match opts.task.as_deref().map(str::trim) {
            Some(task) if !task.is_empty() => {
                let in_pool: BTreeSet<Ulid> = pool.iter().map(|c| c.id).collect();
                let filter = Filter {
                    limit: Some(RETRIEVE_TOP * 4),
                    ..Default::default()
                };
                let mut ranked: Vec<Ulid> = self
                    .store
                    .search(task, &filter)?
                    .into_iter()
                    .map(|c| c.id)
                    .filter(|id| in_pool.contains(id))
                    .take(RETRIEVE_TOP)
                    .collect();
                // Then one hop out from the best hits. Appending rather than
                // fusing a second list is deliberate: a neighbour should sit
                // below every direct hit, not compete with the top of it.
                let seeds: Vec<Ulid> = ranked.iter().copied().take(EXPAND_SEEDS).collect();
                let known: BTreeSet<Ulid> = ranked.iter().copied().collect();
                ranked.extend(
                    self.store
                        .neighbors(&seeds, &filter, RETRIEVE_TOP)?
                        .into_iter()
                        .filter(|id| in_pool.contains(id) && !known.contains(id)),
                );
                if ranked.is_empty() {
                    // Nothing matched, so there is nothing to narrow by: pack
                    // the branch as if no task had been given rather than
                    // obeying the task into an almost empty pack.
                    None
                } else {
                    let mut scores: HashMap<Ulid, f64> = ranked
                        .iter()
                        .enumerate()
                        .map(|(rank, id)| (*id, relevance_at(rank)))
                        .collect();
                    // A task narrows the pack; it must not empty it. Claims
                    // the search did not rank keep a floor, so a constraint
                    // nobody happens to phrase the way this task phrases it
                    // can still be packed when there is budget left for it.
                    for c in &pool {
                        scores.entry(c.id).or_insert(RELEVANCE_FLOOR);
                    }
                    Some(scores)
                }
            }
            _ => None,
        };
        let pinned: Vec<Ulid> = def
            .map(|d| {
                d.pins
                    .iter()
                    .filter_map(|p| self.find_one(p).ok())
                    .map(|c| c.id)
                    .collect()
            })
            .unwrap_or_default();

        Ok(ctx_pack::compile(&ctx_pack::Request {
            branch: branch.as_str(),
            pool: &pool,
            relevance: relevance.as_ref(),
            pinned: &pinned,
            budget,
            projection,
            weights: &self.weights,
            generation: self.store.generation()?,
            version: VERSION,
            docs: &self.doc_refs(&branch)?,
        }))
    }

    /// The ~250-token overview (first MCP response).
    pub fn index(&self) -> Result<String> {
        let mut entries: Vec<IndexEntry> = Vec::new();
        let summaries = self.store.branches()?;
        let mut seen = BTreeSet::new();
        for s in &summaries {
            let archived = BranchRef::new(&s.branch)
                .ok()
                .and_then(|b| self.branches.get(&b).map(|d| d.archived))
                .unwrap_or(false);
            if archived || s.active + s.proposed == 0 {
                continue;
            }
            seen.insert(s.branch.clone());
            entries.push(IndexEntry {
                branch: s.branch.clone(),
                active: s.active,
                proposed: s.proposed,
                last: s.last_tx.map(|t| t.format("%Y-%m-%d").to_string()),
                note: None,
            });
        }
        for b in self.branches.all() {
            if !seen.contains(b.as_str()) && !self.branches.get(&b).is_some_and(|d| d.archived) {
                entries.push(IndexEntry {
                    branch: b.to_string(),
                    active: 0,
                    proposed: 0,
                    last: None,
                    note: Some("empty".into()),
                });
            }
        }
        entries.sort_by(|a, b| a.branch.cmp(&b.branch));
        let active = self.default_branch().ok().map(|b| b.to_string());
        Ok(ctx_pack::render_index(&entries, active.as_deref(), 250))
    }

    /// Compare a Merkle root from elsewhere with this branch's claim set.
    pub fn verify(&self, branch: &BranchRef, root: &str) -> Result<VerifyResult> {
        let mut claims = self.pool(branch)?;
        claims.retain(|c| c.status == Status::Active);
        claims.sort_by_key(|c| c.id);
        let want = root.trim().to_ascii_lowercase();
        let matches = |cids: &[&str]| merkle::root(cids).starts_with(&want);
        let cids: Vec<&str> = claims.iter().map(|c| c.cid.as_str()).collect();
        if matches(&cids) {
            return Ok(VerifyResult::InSync);
        }
        // Walk back through recent states (prefixes in ULID order).
        for back in 1..=cids.len().min(500) {
            if matches(&cids[..cids.len() - back]) {
                return Ok(VerifyResult::Behind(back));
            }
        }
        Ok(VerifyResult::AheadOrDiverged)
    }

    /// State root of a branch: Merkle root over its active pool.
    pub fn state_root(&self, branch: &BranchRef) -> Result<String> {
        let claims = self.pool(branch)?;
        let cids: Vec<&str> = claims
            .iter()
            .filter(|c| c.status == Status::Active)
            .map(|c| c.cid.as_str())
            .collect();
        Ok(merkle::root(&cids))
    }

    // ---- packs on disk and sync -----------------------------------------------

    /// Write compiled packs to `packs/` for the Worker to serve to chat
    /// surfaces: a dossier per branch plus `index.md`. Only touches files
    /// whose content changed, so syncing an unchanged store commits nothing.
    pub fn write_packs(&self) -> Result<usize> {
        let dir = self.home.root().join("packs");
        let mut written = 0;
        let mut branches: BTreeSet<String> = self
            .branches
            .all()
            .into_iter()
            .filter(|b| !self.branches.get(b).is_some_and(|d| d.archived))
            .map(String::from)
            .collect();
        for s in self.store.branches()? {
            if s.active > 0 {
                branches.insert(s.branch);
            }
        }
        for b in &branches {
            // Browser surfaces get a generous budget regardless of the
            // branch's own (agent-sized) one: there the window, not cost, is
            // the constraint (spec §8.6).
            let pack = self.pack(&PackOpts {
                branch: Some(b.to_owned()),
                projection: Some(Projection::Dossier),
                budget: Some(Projection::Dossier.default_budget()),
                ..Default::default()
            })?;
            written += write_if_changed(&dir.join(format!("{b}.md")), &pack.markdown)? as usize;
        }
        written += write_if_changed(&dir.join("index.md"), &self.index()?)? as usize;
        // A pack is a rendering of the store, so a branch that is gone must
        // not leave one behind. Without this a deleted idea stayed in the
        // store's own packs/ directory, and in the git repository the chats
        // read, long after it had stopped appearing anywhere else.
        let wanted: BTreeSet<PathBuf> = branches
            .iter()
            .map(|b| dir.join(format!("{b}.md")))
            .collect();
        written += Self::prune_packs(&dir, &wanted)?;
        Ok(written)
    }

    /// Delete pack files for branches that no longer have one, and any
    /// project directory left empty by that. Returns how many were removed.
    fn prune_packs(dir: &Path, wanted: &BTreeSet<PathBuf>) -> Result<usize> {
        if !dir.is_dir() {
            return Ok(0);
        }
        let mut removed = 0;
        for project in fs::read_dir(dir)? {
            let project = project?.path();
            if !project.is_dir() {
                continue; // index.md, and anything else at the top level
            }
            for pack in fs::read_dir(&project)? {
                let pack = pack?.path();
                if pack.extension().is_some_and(|e| e == "md") && !wanted.contains(&pack) {
                    fs::remove_file(&pack)?;
                    removed += 1;
                }
            }
            if fs::read_dir(&project)?.next().is_none() {
                fs::remove_dir(&project)?;
            }
        }
        Ok(removed)
    }

    /// Refresh packs, commit, pull (rebase), push. Offline is fine.
    pub fn sync(&mut self) -> Result<sync::SyncReport> {
        self.write_packs()?;
        let report = sync::sync(&self.home, &format!("ctx: sync from {}", self.machine))?;
        if report.pulled {
            self.catch_up()?;
            // Pulled claims may change packs; commit those too (no push
            // needed urgently: the next sync carries them).
            if self.write_packs()? > 0 {
                sync::commit(&self.home, &format!("ctx: packs from {}", self.machine))?;
                let _ = sync::push(&self.home, None);
            }
        }
        Ok(report)
    }

    /// Best-effort pull used at session start: bounded by `timeout`, and
    /// failure is reported, never fatal (a session must always start).
    pub fn pull_quick(&mut self, timeout: Duration) -> Result<bool, String> {
        let pulled = sync::pull(&self.home, Some(timeout)).map_err(|e| e.to_string())?;
        if pulled {
            self.catch_up().map_err(|e| e.to_string())?;
        }
        Ok(pulled)
    }
}

pub fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

fn write_if_changed(path: &Path, content: &str) -> Result<bool> {
    if fs::read_to_string(path).is_ok_and(|old| old == content) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, content)?;
    Ok(true)
}

/// Parse `research:decision,constraint` (for `--inherits`).
pub fn parse_inherits(spec: &str) -> Result<(String, Vec<Kind>)> {
    let (branch, kinds) = spec
        .split_once(':')
        .with_context(|| format!("expected BRANCH:kind,kind, got `{spec}`"))?;
    let kinds = kinds
        .split(',')
        .map(|k| k.trim().parse::<Kind>().map_err(anyhow::Error::from))
        .collect::<Result<Vec<_>>>()?;
    Ok((branch.trim().to_owned(), kinds))
}
