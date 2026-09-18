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

use crate::claim::{Claim, Status};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "rec", rename_all = "snake_case")]
// Nearly every record is a claim, so boxing it would add an allocation per
// log line to save space only in the rare small variants.
#[allow(clippy::large_enum_variant)]
pub enum Record {
    Claim(Claim),
    Status(StatusChange),
    Counter(CounterUpdate),
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
