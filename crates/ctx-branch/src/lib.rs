//! Context branches (spec §7).
//!
//! A branch is a *field* on each claim plus an entry in `refs/branches.yaml`,
//! never a git branch — the whole point is working on `research` and `code`
//! at the same time. This crate owns the DAG: validating it, answering "which
//! claims can branch B see" (pool resolution), and routing a working directory
//! to a branch via `.ctx.yaml`.

mod binding;
mod config;
mod templates;

pub use binding::{BINDING_FILE, Binding};
pub use config::{BranchConfig, BranchDef, BranchError, PoolSource};
pub use templates::{TEMPLATES, template};
