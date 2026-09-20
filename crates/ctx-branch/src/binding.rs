//! `.ctx.yaml`: the committed file that binds a code repo to a branch.
//!
//! Routing is declared, never inferred (spec invariant 5): the model is told
//! which branch it is in, it is never asked.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ctx_core::BranchRef;
use serde::{Deserialize, Serialize};

use crate::config::BranchError;

/// Everything ctx owns inside a repository lives in this directory.
pub const CTX_DIR: &str = ".ctx";
/// The binding, relative to the repository root.
pub const BINDING_FILE: &str = ".ctx/config.yaml";
/// Where the binding lived before `.ctx/`; still read, and migrated on write.
pub const LEGACY_BINDING_FILE: &str = ".ctx.yaml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub project: String,
    pub branch: String,
    /// Repository root: the directory containing `.ctx/` (not serialised).
    #[serde(skip)]
    pub root: PathBuf,
    /// Found at the legacy `.ctx.yaml` path and not yet migrated.
    #[serde(skip)]
    pub legacy: bool,
}

impl Binding {
    /// Find `.ctx.yaml` in `start` or the nearest ancestor directory.
    pub fn discover(start: &Path) -> Result<Option<Binding>, BranchError> {
        for dir in start.ancestors() {
            let mut legacy = false;
            let mut path = dir.join(BINDING_FILE);
            let text = match fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    path = dir.join(LEGACY_BINDING_FILE);
                    match fs::read_to_string(&path) {
                        Ok(t) => {
                            legacy = true;
                            t
                        }
                        Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                        Err(source) => return Err(BranchError::Io { path, source }),
                    }
                }
                Err(source) => return Err(BranchError::Io { path, source }),
            };
            let mut b: Binding =
                serde_yaml::from_str(&text).map_err(|source| BranchError::Parse {
                    path: path.clone(),
                    source,
                })?;
            b.root = dir.to_owned();
            b.legacy = legacy;
            b.branch_ref()?;
            return Ok(Some(b));
        }
        Ok(None)
    }

    pub fn branch_ref(&self) -> Result<BranchRef, BranchError> {
        let full = format!("{}/{}", self.project, self.branch);
        BranchRef::new(&full).map_err(|_| BranchError::InvalidName(full))
    }

    /// Write `.ctx/config.yaml`, removing a legacy `.ctx.yaml` if present
    /// (that is the whole migration: the content is identical).
    pub fn write(&self, dir: &Path) -> Result<(), BranchError> {
        let ctx_dir = dir.join(CTX_DIR);
        fs::create_dir_all(&ctx_dir).map_err(|source| BranchError::Io {
            path: ctx_dir.clone(),
            source,
        })?;
        let _ = fs::remove_file(dir.join(LEGACY_BINDING_FILE));
        let path = dir.join(BINDING_FILE);
        let body = format!(
            "# RecurOS binding: which context branch this repo's agents use.\n\
             # Commit this file. See `ctx --help`.\n\
             project: {}\nbranch: {}\n",
            self.project, self.branch
        );
        fs::write(&path, body).map_err(|source| BranchError::Io { path, source })
    }

    /// Qualify a user-typed branch name in this binding's project:
    /// `research` → `sovereign/research`; `other/x` stays.
    pub fn qualify(&self, name: &str) -> String {
        if name.contains('/') {
            name.to_owned()
        } else {
            format!("{}/{}", self.project, name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_in_ancestors() {
        let dir = tempfile::tempdir().unwrap();
        let b = Binding {
            project: "sovereign".into(),
            branch: "code".into(),
            root: PathBuf::new(),
            legacy: false,
        };
        b.write(dir.path()).unwrap();
        let deep = dir.path().join("src/a/b");
        fs::create_dir_all(&deep).unwrap();
        let found = Binding::discover(&deep).unwrap().unwrap();
        assert_eq!(found.branch_ref().unwrap().as_str(), "sovereign/code");
        assert_eq!(found.root, dir.path());
        assert_eq!(found.qualify("research"), "sovereign/research");
    }
}
