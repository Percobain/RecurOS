//! The claim: the single record type everything in RecurOS is made of.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::canonical::{CanonicalContent, WHITESPACE};
use crate::error::CoreError;
use crate::tokens;

/// Which of the six things a claim is. Exactly six — new distinctions belong in
/// `entities`, not here, because every kind multiplies renderer and weight logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Fact,
    Decision,
    Rejected,
    Constraint,
    Question,
    Claim,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Fact,
        Kind::Decision,
        Kind::Rejected,
        Kind::Constraint,
        Kind::Question,
        Kind::Claim,
    ];

    /// The spelling used in the canonical form and the log. Changing any of
    /// these strings changes every CID, so they are frozen.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Fact => "fact",
            Kind::Decision => "decision",
            Kind::Rejected => "rejected",
            Kind::Constraint => "constraint",
            Kind::Question => "question",
            Kind::Claim => "claim",
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

impl FromStr for Kind {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let lower = s.to_ascii_lowercase();
        Kind::ALL
            .into_iter()
            .find(|k| k.as_str() == lower)
            .ok_or_else(|| CoreError::UnknownVariant {
                what: "kind",
                value: s.to_owned(),
                expected: "fact, decision, rejected, constraint, question, claim",
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Active,
    Superseded,
    Proposed,
    Rejected,
    Archived,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Active => "active",
            Status::Superseded => "superseded",
            Status::Proposed => "proposed",
            Status::Rejected => "rejected",
            Status::Archived => "archived",
        }
    }
}

impl FromStr for Status {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        [
            Status::Active,
            Status::Superseded,
            Status::Proposed,
            Status::Rejected,
            Status::Archived,
        ]
        .into_iter()
        .find(|st| st.as_str() == s)
        .ok_or_else(|| CoreError::UnknownVariant {
            what: "status",
            value: s.to_owned(),
            expected: "active, superseded, proposed, rejected, archived",
        })
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    High,
    #[default]
    Medium,
    Low,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::High => "high",
            Confidence::Medium => "medium",
            Confidence::Low => "low",
        }
    }
}

impl fmt::Display for Confidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

impl FromStr for Confidence {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "high" => Ok(Confidence::High),
            "medium" | "med" => Ok(Confidence::Medium),
            "low" => Ok(Confidence::Low),
            _ => Err(CoreError::UnknownVariant {
                what: "confidence",
                value: s.to_owned(),
                expected: "high, medium, low",
            }),
        }
    }
}

/// A context branch name such as `sovereign/code`. Branches are a field on the
/// claim, not a git branch (spec §7.1), so the name only needs to be a stable,
/// path-safe identifier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BranchRef(String);

impl BranchRef {
    /// Where claims land before branches exist (M1) or when none is bound.
    pub const DEFAULT: &'static str = "default";

    pub fn new(s: &str) -> Result<Self, CoreError> {
        let valid = !s.is_empty()
            && !s.starts_with('/')
            && !s.ends_with('/')
            && !s.contains("//")
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_/".contains(&b));
        if valid {
            Ok(BranchRef(s.to_owned()))
        } else {
            Err(CoreError::InvalidBranch(s.to_owned()))
        }
    }

    pub fn default_branch() -> Self {
        BranchRef(Self::DEFAULT.to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BranchRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

impl TryFrom<String> for BranchRef {
    type Error = CoreError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        BranchRef::new(&s)
    }
}

impl From<BranchRef> for String {
    fn from(b: BranchRef) -> String {
        b.0
    }
}

/// One immutable record of something known about a project. See spec §5.1;
/// every field is load-bearing. Claims are never mutated once written —
/// corrections are new claims that `supersede` old ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    pub id: Ulid,
    pub cid: String,
    pub kind: Kind,
    pub branch: BranchRef,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<Ulid>,
    pub status: Status,
    pub confidence: Confidence,
    pub src: String,
    /// G-counter keyed by machine id: merged by per-key max, summed on read.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub helpful: BTreeMap<String, u32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub harmful: BTreeMap<String, u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<DateTime<Utc>>,
    /// When this became true in the world.
    pub t_valid: DateTime<Utc>,
    /// When this was recorded.
    pub t_tx: DateTime<Utc>,
    /// Estimated tokens to render `text` alone, precomputed so the packer
    /// never tokenises inside its selection loop.
    pub tokens: u32,
}

/// User-supplied fields for a new claim, before normalisation and before an
/// id, CID or timestamps are assigned.
#[derive(Debug, Clone)]
pub struct ClaimDraft {
    pub kind: Kind,
    pub branch: BranchRef,
    pub text: String,
    pub why: Option<String>,
    pub refs: Vec<String>,
    pub entities: Vec<String>,
    pub supersedes: Option<Ulid>,
    pub confidence: Confidence,
    pub src: String,
    pub t_valid: Option<DateTime<Utc>>,
}

impl ClaimDraft {
    pub fn new(kind: Kind, text: impl Into<String>) -> Self {
        ClaimDraft {
            kind,
            branch: BranchRef::default_branch(),
            text: text.into(),
            why: None,
            refs: Vec::new(),
            entities: Vec::new(),
            supersedes: None,
            confidence: Confidence::default(),
            src: "cli".to_owned(),
            t_valid: None,
        }
    }
}

impl Claim {
    /// Build a claim from a draft. The id and clock are passed in rather than
    /// generated here so this stays pure and testable. Stored content fields
    /// are the *normalised* ones, so what is on disk is exactly what was hashed.
    pub fn from_draft(draft: ClaimDraft, id: Ulid, now: DateTime<Utc>) -> Result<Claim, CoreError> {
        let content = CanonicalContent::new(
            draft.kind,
            &draft.text,
            draft.why.as_deref(),
            &draft.refs,
            &draft.entities,
        );
        if content.text.trim_matches(WHITESPACE).is_empty() {
            return Err(CoreError::EmptyText);
        }
        let cid = content.cid();
        let tokens = tokens::estimate(&content.text);
        Ok(Claim {
            id,
            cid,
            kind: content.kind,
            branch: draft.branch,
            text: content.text,
            why: content.why,
            refs: content.refs,
            entities: content.entities,
            supersedes: draft.supersedes,
            status: Status::Active,
            confidence: draft.confidence,
            src: draft.src,
            helpful: BTreeMap::new(),
            harmful: BTreeMap::new(),
            last_used: None,
            t_valid: draft.t_valid.unwrap_or(now),
            t_tx: now,
            tokens,
        })
    }

    /// The canonical content of this claim, for re-verifying its CID.
    pub fn content(&self) -> CanonicalContent {
        CanonicalContent::new(
            self.kind,
            &self.text,
            self.why.as_deref(),
            &self.refs,
            &self.entities,
        )
    }

    /// True if the stored CID matches the content. A mismatch means the log
    /// line was edited by hand or corrupted.
    pub fn verify_cid(&self) -> bool {
        self.content().cid() == self.cid
    }

    pub fn helpful_total(&self) -> u64 {
        self.helpful.values().map(|&v| u64::from(v)).sum()
    }

    pub fn harmful_total(&self) -> u64 {
        self.harmful.values().map(|&v| u64::from(v)).sum()
    }

    /// Short CID tag used in rendered packs, e.g. `7f2a` for `[c:7f2a]`.
    pub fn short_cid(&self) -> &str {
        let hex = self.cid.strip_prefix("b3:").unwrap_or(&self.cid);
        &hex[..hex.len().min(4)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn from_draft_stores_normalised_content() {
        let mut d = ClaimDraft::new(Kind::Decision, "Use SQLite  \r\n");
        d.entities = vec!["Storage".into(), " storage".into()];
        let c = Claim::from_draft(d, Ulid::nil(), at("2026-09-18T00:00:00Z")).unwrap();
        assert_eq!(c.text, "Use SQLite");
        assert_eq!(c.entities, vec!["storage"]);
        assert!(c.verify_cid());
    }

    #[test]
    fn empty_text_is_rejected() {
        let d = ClaimDraft::new(Kind::Fact, " \r\n\u{00A0}\n");
        assert!(matches!(
            Claim::from_draft(d, Ulid::nil(), at("2026-09-18T00:00:00Z")),
            Err(CoreError::EmptyText)
        ));
    }

    #[test]
    fn branch_names_are_validated() {
        assert!(BranchRef::new("sovereign/code").is_ok());
        assert!(BranchRef::new("x-marketing_2").is_ok());
        for bad in ["", "/a", "a/", "a//b", "Code", "a b", "a.b"] {
            assert!(BranchRef::new(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn serde_round_trip() {
        let mut d = ClaimDraft::new(Kind::Constraint, "Never base64");
        d.why = Some("models cannot decode it".into());
        let c = Claim::from_draft(d, Ulid::nil(), at("2026-09-18T00:00:00Z")).unwrap();
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Claim>(&json).unwrap(), c);
    }
}
