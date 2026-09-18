//! Built-in branch templates (`ctx branch new <name> --template <t>`).

use std::collections::BTreeMap;

use ctx_core::Kind::{self, *};

use crate::config::BranchDef;

pub const TEMPLATES: [&str; 4] = ["research", "code", "gtm", "writing"];

/// The definition for template `name`. `parent` is the branch it inherits
/// from, if any; the template decides which kinds flow down that edge.
pub fn template(name: &str, parent: Option<&str>) -> Option<BranchDef> {
    let (holds, from_parent, budget, compile, binds): (&[Kind], &[Kind], u32, &str, &[&str]) =
        match name {
            // Thinking space: every kind, generous budget. Constraints are
            // included because research is where most are discovered (API
            // limits, budgets, legal), and `code` inherits them from here.
            "research" => (
                &[Fact, Question, Claim, Rejected, Decision, Constraint],
                &[Decision, Fact],
                1200,
                "dossier",
                &[],
            ),
            // Building: conclusions only. Inherits decisions, constraints and
            // rejected approaches (so the builder doesn't propose them again),
            // not the open-ended churn of research.
            "code" => (
                &[Decision, Constraint, Rejected, Question, Fact],
                &[Decision, Constraint, Rejected],
                700,
                "agents-md",
                &["claude-code", "codex", "cursor", "gemini"],
            ),
            // Go-to-market: claims and positioning, grounded in decisions.
            "gtm" => (
                &[Claim, Decision, Rejected, Question],
                &[Decision, Claim, Constraint],
                900,
                "prose",
                &[],
            ),
            "writing" => (
                &[Claim, Fact, Decision, Rejected, Question],
                &[Decision, Fact, Claim],
                1500,
                "dossier",
                &[],
            ),
            _ => return None,
        };
    let mut inherits = BTreeMap::new();
    if let Some(p) = parent {
        inherits.insert(p.to_owned(), from_parent.to_vec());
    }
    Some(BranchDef {
        parent: parent.map(str::to_owned),
        holds: holds.to_vec(),
        inherits,
        budget: Some(budget),
        compile: Some(compile.to_owned()),
        binds: binds.iter().map(|s| (*s).to_owned()).collect(),
        pins: Vec::new(),
        archived: false,
    })
}
