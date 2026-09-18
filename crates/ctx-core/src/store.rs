//! The index abstraction.
//!
//! A `Store` is a *derived, disposable cache* over the log — never a source of
//! truth (spec invariant 1). It tracks how far into each shard it has read so
//! it can catch up incrementally after a local append or a `git pull`, and it
//! can always be thrown away and rebuilt from the log.
//!
//! Claims returned by a store carry their *effective* status and counters,
//! i.e. with all status and counter records merged in.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ulid::Ulid;

use crate::claim::{BranchRef, Claim, Kind, Status};
use crate::record::{Record, StatusChange};

/// Which claims a scan or search considers. Empty/`None` fields don't filter.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub branch: Option<BranchRef>,
    pub kinds: Vec<Kind>,
    pub statuses: Vec<Status>,
    /// Only claims recorded at or after this time.
    pub since: Option<DateTime<Utc>>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub claims: u64,
    pub by_kind: BTreeMap<String, u64>,
    pub by_branch: BTreeMap<String, u64>,
    pub shards: u64,
}

/// Per-branch summary for `ctx branch ls` and the index projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchSummary {
    pub branch: String,
    /// Claims counted by effective status.
    pub active: u64,
    pub proposed: u64,
    pub total: u64,
    pub last_tx: Option<DateTime<Utc>>,
}

pub trait Store {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Atomically index `records` read from `shard` and advance that shard's
    /// cursor to `next_offset`. Records already indexed are ignored, so
    /// re-reading is harmless.
    fn ingest(
        &mut self,
        shard: &str,
        records: &[Record],
        next_offset: u64,
    ) -> Result<(), Self::Error>;

    /// Byte offset up to which `shard` has been indexed (0 if never seen).
    fn cursor(&self, shard: &str) -> Result<u64, Self::Error>;

    fn get(&self, id: Ulid) -> Result<Option<Claim>, Self::Error>;

    /// Claims whose id (ULID text) or cid (hex, with or without `b3:`/`c:`)
    /// starts with `prefix`. Lets people type `c:7f2a` from a rendered pack.
    fn resolve(&self, prefix: &str) -> Result<Vec<Claim>, Self::Error>;

    /// An existing claim with this content on this branch, for dedup.
    fn find_by_cid(&self, cid: &str, branch: &BranchRef) -> Result<Option<Claim>, Self::Error>;

    /// Claims matching `filter`, oldest first (ULID order).
    fn scan(&self, filter: &Filter) -> Result<Vec<Claim>, Self::Error>;

    /// Full-text search, best match first.
    fn search(&self, query: &str, filter: &Filter) -> Result<Vec<Claim>, Self::Error>;

    /// Every status record that targets `claim`, oldest first.
    fn status_history(&self, claim: Ulid) -> Result<Vec<StatusChange>, Self::Error>;

    fn branches(&self) -> Result<Vec<BranchSummary>, Self::Error>;

    /// Largest record id seen: a cheap "generation" for the whole store.
    fn generation(&self) -> Result<Option<Ulid>, Self::Error>;

    /// Forget everything, including cursors, so the next catch-up re-reads
    /// the whole log.
    fn rebuild(&mut self) -> Result<(), Self::Error>;

    fn stats(&self) -> Result<Stats, Self::Error>;
}
