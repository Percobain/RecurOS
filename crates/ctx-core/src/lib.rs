//! Core data model for ContextOS.
//!
//! Everything else in the workspace depends on this crate, so it stays small:
//! the claim type, the canonical byte form that content addresses are computed
//! over, and the log record envelope. No I/O lives here.

pub mod canonical;
pub mod claim;
pub mod error;
pub mod jcs;
pub mod merkle;
pub mod record;
pub mod store;
pub mod tokens;

pub use canonical::{CanonicalContent, canonical_bytes, cid};
pub use claim::{BranchRef, Claim, ClaimDraft, Confidence, Kind, Status};
pub use error::CoreError;
pub use record::{CounterUpdate, Doc, Record, StatusChange};
pub use store::{BranchSummary, Filter, Stats, Store};
