//! The on-disk side of RecurOS: the `~/ctx` layout and the sharded,
//! append-only log that is the only source of truth (spec §6).
//!
//! Sync design in one sentence: every writer owns exactly one shard directory
//! and nobody else ever writes to it, so git merges are structurally
//! conflict-free.

pub mod home;
pub mod shard;
pub mod sync;

pub use home::{CtxHome, HomeError};
pub use shard::{ReadOutcome, ShardError, append, list_shards, read_from, shard_path};
