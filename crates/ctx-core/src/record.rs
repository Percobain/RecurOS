//! The envelope for one line of a shard file (see `docs/protocol.md` §2).
//!
//! The log is permanent, so lines are tagged: new record types can be added
//! without a format break, and readers skip tags they don't understand.
//! Claims are immutable; everything that "changes" about a claim (status,
//! usefulness counters) is a separate record that references it, merged by
//! an order-independent rule so any interleaving of shards converges.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::canonical::normalize_text;
use crate::claim::{BranchRef, Claim, Status};
use crate::error::CoreError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "rec", rename_all = "snake_case")]
// Nearly every record is a claim, so boxing it would add an allocation per
// log line to save space only in the rare small variants.
#[allow(clippy::large_enum_variant)]
pub enum Record {
    Claim(Claim),
    Status(StatusChange),
    Counter(CounterUpdate),
    Doc(Doc),
}

/// Moves a claim along the status lattice. Never "moves it back": the
/// effective status is the max of all transitions, so this can only raise it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusChange {
    pub id: Ulid,
    pub claim: Ulid,
    pub to: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub src: String,
    pub t_tx: DateTime<Utc>,
}

/// One machine's absolute helpful/harmful counts for a claim (a G-counter
/// entry). Merge is per-machine max; the total is the sum over machines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CounterUpdate {
    pub id: Ulid,
    pub claim: Ulid,
    pub machine: String,
    pub helpful: u32,
    pub harmful: u32,
    pub t_tx: DateTime<Utc>,
}

/// A long-form document attached to a branch, such as the spec a research
/// phase ends with. Documents are not claims: a claim is one short, atomic
/// statement the packer can select or drop, while a spec is read whole.
/// Keeping them as a separate record type preserves the six claim kinds.
///
/// Versions are records too: the newest `id` for a `(branch, name)` pair is
/// the current version, and older versions stay in the log as history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Doc {
    pub id: Ulid,
    pub branch: BranchRef,
    /// Short identifier, e.g. `spec`. Lowercase letters, digits and `-`.
    pub name: String,
    pub title: String,
    /// Markdown, normalised like claim text (NFC, LF, no trailing spaces).
    pub body: String,
    /// `b3:` + BLAKE3 of the normalised body plus one newline.
    pub cid: String,
    pub src: String,
    pub t_tx: DateTime<Utc>,
}

impl Doc {
    /// Build a document, normalising the body and deriving the title from
    /// the first markdown heading when none is given.
    pub fn new(
        id: Ulid,
        branch: BranchRef,
        name: &str,
        title: Option<&str>,
        body: &str,
        src: &str,
        t_tx: DateTime<Utc>,
    ) -> Result<Doc, CoreError> {
        let name = name.trim().to_ascii_lowercase();
        let valid = !name.is_empty()
            && name.len() <= 64
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !valid {
            return Err(CoreError::InvalidDocName(name));
        }
        let body = normalize_text(body);
        if body.trim().is_empty() {
            return Err(CoreError::EmptyText);
        }
        let title = title
            .map(|t| normalize_text(t).trim().to_owned())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| derive_title(&body, &name));
        Ok(Doc {
            id,
            branch,
            cid: doc_cid(&body),
            name,
            title,
            body,
            src: src.to_owned(),
            t_tx,
        })
    }

    pub fn verify_cid(&self) -> bool {
        doc_cid(&normalize_text(&self.body)) == self.cid
    }
}

/// Content address of a document body (already normalised).
pub fn doc_cid(body: &str) -> String {
    let mut bytes = body.as_bytes().to_vec();
    bytes.push(b'\n');
    format!("b3:{}", blake3::hash(&bytes).to_hex())
}

fn derive_title(body: &str, name: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| {
            l.trim_start_matches('#')
                .trim()
                .chars()
                .take(120)
                .collect::<String>()
        })
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| name.to_owned())
}

impl Status {
    /// Position in the merge lattice. Effective status is the max rank seen,
    /// which is commutative and idempotent, so merges converge regardless of
    /// the order shards are read in.
    pub fn rank(self) -> u8 {
        match self {
            Status::Proposed => 0,
            Status::Active => 1,
            Status::Superseded => 2,
            Status::Rejected => 3,
            Status::Archived => 4,
        }
    }

    pub fn join(self, other: Status) -> Status {
        if other.rank() > self.rank() {
            other
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_normalises_and_derives_title() {
        let t = DateTime::<Utc>::UNIX_EPOCH;
        let b = BranchRef::default_branch;
        let raw = "# My Idea  \r\n\nBuild it.\n\n";
        let d = Doc::new(Ulid::nil(), b(), "Spec", None, raw, "cli", t).unwrap();
        assert_eq!(d.name, "spec");
        assert_eq!(d.title, "My Idea");
        assert_eq!(d.body, "# My Idea\n\nBuild it.");
        assert!(d.verify_cid());
        assert!(Doc::new(Ulid::nil(), b(), "a b", None, "x", "cli", t).is_err());
        assert!(Doc::new(Ulid::nil(), b(), "spec", None, " \n", "cli", t).is_err());
    }

    #[test]
    fn join_is_order_independent() {
        use Status::*;
        let all = [Proposed, Active, Superseded, Rejected, Archived];
        for a in all {
            for b in all {
                assert_eq!(a.join(b), b.join(a));
                for c in all {
                    assert_eq!(a.join(b).join(c), a.join(b.join(c)));
                }
            }
            assert_eq!(a.join(a), a);
        }
    }
}
