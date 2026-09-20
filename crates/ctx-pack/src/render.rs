//! Projections: the same selected set rendered for different readers
//! (spec §8.6). Renderers are pure: `(claims, projection) -> String`.

use std::fmt::Write as _;
use std::str::FromStr;

use ctx_core::{Claim, Kind, Status};

use crate::select::WhyMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Projection {
    /// Compact, imperative, for coding agents (AGENTS.md). Includes the
    /// operating-rules block.
    AgentsMd,
    /// Same shape as agents-md without the rules: for `ctx pack | ollama run`.
    Markdown,
    /// Thesis / evidence / contradictions / open questions, reasons always
    /// included. For browser chat surfaces where the window, not cost, is
    /// the constraint.
    Dossier,
    /// Handoff narrative for humans. Unlimited by default.
    Prose,
}

impl FromStr for Projection {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "agents-md" | "agents" => Ok(Projection::AgentsMd),
            "markdown" | "stdout" | "md" => Ok(Projection::Markdown),
            "dossier" => Ok(Projection::Dossier),
            "prose" | "handoff" => Ok(Projection::Prose),
            other => Err(format!(
                "unknown projection `{other}` (expected agents-md, markdown, dossier, prose/handoff)"
            )),
        }
    }
}

impl Projection {
    pub fn name(self) -> &'static str {
        match self {
            Projection::AgentsMd => "agents-md",
            Projection::Markdown => "markdown",
            Projection::Dossier => "dossier",
            Projection::Prose => "prose",
        }
    }

    /// One parameter, not a separate code path: browser and human surfaces
    /// get `why` from the start, agent surfaces only with leftover budget.
    pub fn why_mode(self) -> WhyMode {
        match self {
            Projection::AgentsMd | Projection::Markdown => WhyMode::Leftover,
            Projection::Dossier | Projection::Prose => WhyMode::Always,
        }
    }

    pub fn default_budget(self) -> u32 {
        match self {
            Projection::AgentsMd => 700,
            Projection::Markdown => 1500,
            Projection::Dossier => 4000,
            Projection::Prose => UNLIMITED,
        }
    }
}

/// Budget value meaning "no limit" (handoffs).
pub const UNLIMITED: u32 = u32::MAX / 4;

/// One selected claim as the renderer sees it.
#[derive(Debug, Clone)]
pub struct Item<'a> {
    pub claim: &'a Claim,
    pub with_why: bool,
    pub weight: f64,
}

/// A document visible from the branch being packed. Documents are listed,
/// never inlined: a spec is read whole, on demand, not squeezed into a
/// budget next to claims.
#[derive(Debug, Clone)]
pub struct DocRef {
    pub name: String,
    pub title: String,
    pub tokens: u32,
    /// Where the reader can get the full text, e.g. "`SPEC.md`".
    pub location: String,
}

pub struct Meta<'a> {
    pub branch: &'a str,
    pub version: &'a str,
    /// Newest record id in the store, for the footer.
    pub generation: Option<String>,
    pub budget: u32,
    pub docs: &'a [DocRef],
    /// Drop explanatory prose, the documents list and protocol text. Used
    /// only when the budget is too small to fit them at all.
    pub compact: bool,
}

const RULES: &str = "### Working with ctx
Active branch: {branch}. The context above is compiled; don't re-derive it.
Save only when I explicitly say so, plus one batched call at session end.
Use ctx_append(kind, text, why, refs). Don't log progress or summaries.
Cross-branch material: ctx_propose(target_branch, ...).
For anything else RecurOS does (new idea, spec, build, delete, sync, map, handoff), run the `ctx` CLI yourself; commands are in `.ctx/commands.md` (or `ctx commands`). Ask before deleting.
";

const CLAIMS_PROTOCOL: &str = "---
When I say \"ctx save\", reply with only one fenced code block tagged `ctx-claims` containing a JSON array of {\"kind\", \"text\", \"why\", \"refs\"} objects (kind is one of fact, decision, rejected, constraint, question, claim). Nothing else.
When I say \"ctx spec\", write the complete spec for what we discussed as markdown inside one fenced block opened with four backticks and the tag ctx-spec (````ctx-spec) and closed with four backticks, so code blocks inside it survive. Nothing else.
";

/// Section a claim belongs in, per projection: (order, title).
fn section(p: Projection, c: &Claim) -> (u8, &'static str) {
    if c.status == Status::Superseded {
        return match p {
            Projection::Dossier => (3, "Contradictions and rejected paths"),
            Projection::Prose => (4, "What changed along the way"),
            _ => (6, "Changed over time"),
        };
    }
    match p {
        Projection::AgentsMd | Projection::Markdown => match c.kind {
            Kind::Constraint => (0, "Constraints"),
            Kind::Decision => (1, "Decisions"),
            Kind::Rejected => (2, "Rejected: do not propose these again"),
            Kind::Fact => (3, "Facts"),
            Kind::Question => (4, "Open questions"),
            Kind::Claim => (5, "Claims"),
        },
        Projection::Dossier => match c.kind {
            Kind::Decision | Kind::Claim => (0, "Thesis"),
            Kind::Fact => (1, "Evidence"),
            Kind::Constraint => (2, "Constraints"),
            Kind::Rejected => (3, "Contradictions and rejected paths"),
            Kind::Question => (4, "Open questions"),
        },
        Projection::Prose => match c.kind {
            Kind::Decision | Kind::Fact | Kind::Claim => (0, "Where things stand"),
            Kind::Rejected => (1, "What we tried and rejected"),
            Kind::Constraint => (2, "Constraints you must respect"),
            Kind::Question => (3, "Open questions"),
        },
    }
}

fn visible_refs(c: &Claim) -> impl Iterator<Item = &String> {
    // `ctx:<id>` refs are merge bookkeeping, not something a reader follows.
    c.refs.iter().filter(|r| !r.starts_with("ctx:"))
}

/// Render one claim as a markdown bullet. Used both for output and, before
/// selection, to precompute each claim's exact token cost.
pub fn bullet(c: &Claim, with_why: bool, branch: &str) -> String {
    let mut s = String::from("- ");
    if c.status == Status::Superseded {
        s.push_str("Previously: ");
    }
    let mut lines = c.text.lines();
    s.push_str(lines.next().unwrap_or(""));
    for l in lines {
        s.push_str("\n  ");
        s.push_str(l);
    }
    let refs: Vec<String> = visible_refs(c).map(|r| format!("`{r}`")).collect();
    if !refs.is_empty() {
        let _ = write!(s, " ({})", refs.join(", "));
    }
    if c.branch.as_str() != branch {
        // Inherited: say where it came from, without the project prefix.
        let from = c
            .branch
            .as_str()
            .rsplit('/')
            .next()
            .unwrap_or(c.branch.as_str());
        let _ = write!(s, " _({from})_");
    }
    let _ = write!(s, " [c:{}]", c.short_cid());
    s.push('\n');
    if with_why && let Some(why) = &c.why {
        let _ = writeln!(s, "  - why: {}", why.replace('\n', " "));
    }
    s
}

/// Character counts of [`bullet`] without building it: `(without why,
/// extra for why)`. Precomputing costs for thousands of candidates by
/// rendering each one twice was the single largest cost of a compile.
/// Must mirror `bullet` exactly; a test enforces that.
pub fn bullet_chars(c: &Claim, branch: &str) -> (usize, usize) {
    // Each newline in the text is followed by a two-space continuation indent.
    let text_chars: usize = c.text.chars().count() + 2 * c.text.matches('\n').count();
    let mut n = 2 + text_chars;
    if c.status == Status::Superseded {
        n += "Previously: ".len();
    }
    let refs: Vec<&String> = visible_refs(c).collect();
    if !refs.is_empty() {
        n += 3 + refs.iter().map(|r| r.chars().count() + 2).sum::<usize>() + 2 * (refs.len() - 1);
    }
    if c.branch.as_str() != branch {
        let from = c
            .branch
            .as_str()
            .rsplit('/')
            .next()
            .unwrap_or(c.branch.as_str());
        n += from.chars().count() + 5;
    }
    n += 5 + c.short_cid().len() + 1;
    let why = c
        .why
        .as_ref()
        .map(|w| "  - why: ".len() + w.chars().count() + 1)
        .unwrap_or(0);
    (n, why)
}

fn header(p: Projection, branch: &str) -> String {
    match p {
        Projection::AgentsMd => format!(
            "## Project context: {branch}\n\nCompiled by RecurOS. These are settled; build on them.\n\n"
        ),
        Projection::Markdown => format!("# Context: {branch}\n\n"),
        Projection::Dossier => format!(
            "# Context dossier: {branch}\n\nWhat is known about this project, compiled by RecurOS. Treat it as background you already agreed with.\n\n"
        ),
        Projection::Prose => format!(
            "# Handoff: {branch}\n\nEverything recorded about this work: where it stands, why, what was ruled out, and where to start.\n\n"
        ),
    }
}

fn heading_level(p: Projection) -> &'static str {
    match p {
        Projection::AgentsMd => "###",
        _ => "##",
    }
}

/// The footer that makes a pack reproducible and debuggable (spec §8.5).
pub fn footer(meta: &Meta, n: usize, tokens: u32, root: &str) -> String {
    let root_short = &root[..root.len().min(11)];
    let generation = meta
        .generation
        .as_deref()
        .map(|g| &g[..g.len().min(8)])
        .unwrap_or("0");
    let budget = if meta.budget >= UNLIMITED {
        "inf".to_owned()
    } else {
        meta.budget.to_string()
    };
    format!(
        "<!-- ctx/1 b={} n={n} t={tokens}/{budget} root={root_short} gen={generation} v={} -->\n",
        meta.branch, meta.version
    )
}

/// Render the body (everything except the footer).
pub fn render_body(p: Projection, items: &[Item], meta: &Meta) -> String {
    let mut sorted: Vec<&Item> = items.iter().collect();
    sorted.sort_by(|a, b| {
        section(p, a.claim)
            .0
            .cmp(&section(p, b.claim).0)
            .then_with(|| b.weight.total_cmp(&a.weight))
            .then_with(|| a.claim.id.cmp(&b.claim.id))
    });

    let mut out = if meta.compact {
        format!("# Context: {}\n\n", meta.branch)
    } else {
        header(p, meta.branch)
    };
    if !meta.docs.is_empty() && !meta.compact {
        let _ = writeln!(out, "{} Documents\n", heading_level(p));
        for d in meta.docs {
            let _ = writeln!(
                out,
                "- `{}`: {} (~{} tokens, read it in {})",
                d.name, d.title, d.tokens, d.location
            );
        }
        out.push('\n');
    }
    if sorted.is_empty() {
        out.push_str("_No claims recorded yet. Save one with `ctx save \"...\"`._\n\n");
    }
    let mut current: Option<u8> = None;
    for item in &sorted {
        let (order, title) = section(p, item.claim);
        if current != Some(order) {
            if current.is_some() {
                out.push('\n');
            }
            let _ = writeln!(out, "{} {title}\n", heading_level(p));
            current = Some(order);
        }
        out.push_str(&bullet(item.claim, item.with_why, meta.branch));
    }
    if !sorted.is_empty() {
        out.push('\n');
    }

    if p == Projection::Prose {
        let mut refs: Vec<&String> = Vec::new();
        for item in &sorted {
            for r in visible_refs(item.claim) {
                if !refs.contains(&r) {
                    refs.push(r);
                }
            }
        }
        if !refs.is_empty() {
            out.push_str("## Where to start\n\n");
            for r in refs.iter().take(20) {
                let _ = writeln!(out, "- `{r}`");
            }
            out.push('\n');
        }
    }
    match p {
        _ if meta.compact => {}
        Projection::AgentsMd => out.push_str(&RULES.replace("{branch}", meta.branch)),
        Projection::Dossier => out.push_str(CLAIMS_PROTOCOL),
        _ => {}
    }
    out
}

/// A row of the index projection.
#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub branch: String,
    pub active: u64,
    pub proposed: u64,
    pub last: Option<String>,
    pub note: Option<String>,
}

/// The ~250-token overview returned first over MCP: what exists, pulled on
/// demand (progressive disclosure).
pub fn render_index(entries: &[IndexEntry], active: Option<&str>, budget: u32) -> String {
    let mut out = String::from("# RecurOS index\n\n");
    if let Some(a) = active {
        let _ = writeln!(out, "Current branch: `{a}`\n");
    }
    let tail = "\nTools: `ctx_pack(branch, task?)` compiles a branch's context; `ctx_search(query)` finds claims; `ctx_append` records a claim (only when asked); `ctx_propose` suggests a claim for another branch.\n";
    for (shown, e) in entries.iter().enumerate() {
        let mut line = format!("- `{}`: {} claims", e.branch, e.active);
        if e.proposed > 0 {
            let _ = write!(line, ", {} proposed", e.proposed);
        }
        if let Some(l) = &e.last {
            let _ = write!(line, ", updated {l}");
        }
        if let Some(n) = &e.note {
            let _ = write!(line, " ({n})");
        }
        line.push('\n');
        if ctx_core::tokens::estimate(&(out.clone() + &line + tail)) > budget {
            let _ = writeln!(out, "- … and {} more", entries.len() - shown);
            break;
        }
        out.push_str(&line);
    }
    if entries.is_empty() {
        out.push_str("_No claims yet._\n");
    }
    out.push_str(tail);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_core::{BranchRef, ClaimDraft, Kind, Status};
    use ulid::Ulid;

    #[test]
    fn bullet_chars_matches_bullet() {
        let now = chrono::Utc::now();
        let variants = [
            ("one line", None, vec![], "p/b", Status::Active),
            (
                "two\nlines\nthree",
                Some("because é"),
                vec!["a.rs", "ctx:01X"],
                "p/b",
                Status::Active,
            ),
            (
                "ünï",
                Some("x\ny"),
                vec!["a.rs", "b/c.rs:L9"],
                "p/other",
                Status::Superseded,
            ),
        ];
        for (text, why, refs, branch, status) in variants {
            let mut d = ClaimDraft::new(Kind::Fact, text);
            d.branch = BranchRef::new(branch).unwrap();
            d.why = why.map(str::to_owned);
            d.refs = refs.into_iter().map(str::to_owned).collect();
            let mut c = Claim::from_draft(d, Ulid::nil(), now).unwrap();
            c.status = status;
            let (base, why) = bullet_chars(&c, "p/b");
            assert_eq!(base, bullet(&c, false, "p/b").chars().count(), "{text}");
            assert_eq!(
                base + why,
                bullet(&c, true, "p/b").chars().count(),
                "{text}"
            );
        }
    }
}
