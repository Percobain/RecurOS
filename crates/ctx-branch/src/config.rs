//! `refs/branches.yaml`: definition, validation and pool resolution.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ctx_core::{BranchRef, Kind};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BranchError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path}: invalid branches.yaml: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },
    #[error("invalid branch name `{0}`")]
    InvalidName(String),
    #[error("branch `{branch}` refers to unknown branch `{target}`")]
    UnknownTarget { branch: String, target: String },
    #[error("branch graph has a cycle through: {0}")]
    Cycle(String),
    #[error("branch `{0}` already exists")]
    Exists(String),
    #[error("unknown branch `{0}`")]
    Unknown(String),
    #[error("branch `{branch}` does not hold `{kind}` claims{}", suggestion.as_ref().map(|s| format!("; try --to {s}")).unwrap_or_default())]
    KindNotHeld {
        branch: String,
        kind: Kind,
        suggestion: Option<String>,
    },
    #[error("serialising branches.yaml: {0}")]
    Serialise(#[source] serde_yaml::Error),
}

/// One branch's definition. Every field is optional in YAML.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BranchDef {
    /// Ancestor edge: transitive, with kind-narrowing at each hop.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Kinds this branch accepts. Empty means all kinds.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub holds: Vec<Kind>,
    /// Kinds visible from other branches. The parent's entry scopes the
    /// ancestor edge; any other key is a lateral, non-transitive edge.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub inherits: BTreeMap<String, Vec<Kind>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<u32>,
    /// Default projection: agents-md | dossier | prose | index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compile: Option<String>,
    /// Agents whose sessions default to this branch.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub binds: Vec<String>,
    /// Claim ids always included in packs (charged to the budget).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pins: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
}

impl BranchDef {
    pub fn holds_kind(&self, kind: Kind) -> bool {
        self.holds.is_empty() || self.holds.contains(&kind)
    }
}

/// Where a branch's pool draws claims from: a source branch and the kinds
/// allowed through (`None` = all kinds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolSource {
    pub branch: BranchRef,
    pub kinds: Option<Vec<Kind>>,
}

/// The whole of `branches.yaml`: project → branch → definition.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BranchConfig {
    pub projects: BTreeMap<String, BTreeMap<String, BranchDef>>,
}

impl BranchConfig {
    /// Load and validate. A missing file is an empty config.
    pub fn load(path: &Path) -> Result<Self, BranchError> {
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(BranchError::Io {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        let cfg: BranchConfig =
            serde_yaml::from_str(&text).map_err(|source| BranchError::Parse {
                path: path.to_owned(),
                source,
            })?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Validate, then write atomically (temp file + rename), so a crash
    /// can't leave a half-written DAG behind.
    pub fn save(&self, path: &Path) -> Result<(), BranchError> {
        self.validate()?;
        let yaml = serde_yaml::to_string(self).map_err(BranchError::Serialise)?;
        let tmp = path.with_extension("yaml.tmp");
        let io_err = |source| BranchError::Io {
            path: path.to_owned(),
            source,
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(io_err)?;
        }
        fs::write(&tmp, yaml).map_err(io_err)?;
        fs::rename(&tmp, path).map_err(io_err)
    }

    /// Resolve a branch name as written in the config relative to `project`:
    /// `research` → `project/research`, `other/x` stays as is.
    fn qualify(project: &str, name: &str) -> String {
        if name.contains('/') {
            name.to_owned()
        } else {
            format!("{project}/{name}")
        }
    }

    pub fn get(&self, branch: &BranchRef) -> Option<&BranchDef> {
        let (project, name) = branch.as_str().split_once('/')?;
        self.projects.get(project)?.get(name)
    }

    pub fn contains(&self, branch: &BranchRef) -> bool {
        self.get(branch).is_some()
    }

    /// All defined branches, as full refs, sorted.
    pub fn all(&self) -> Vec<BranchRef> {
        self.projects
            .iter()
            .flat_map(|(p, bs)| bs.keys().map(move |b| format!("{p}/{b}")))
            .filter_map(|s| BranchRef::new(&s).ok())
            .collect()
    }

    /// Outgoing edges of `full` (parent + inherits keys), qualified.
    fn edges(&self, full: &str) -> Vec<String> {
        let Some((project, name)) = full.split_once('/') else {
            return Vec::new();
        };
        let Some(def) = self.projects.get(project).and_then(|b| b.get(name)) else {
            return Vec::new();
        };
        let mut out: BTreeSet<String> = def
            .inherits
            .keys()
            .map(|k| Self::qualify(project, k))
            .collect();
        if let Some(p) = &def.parent {
            out.insert(Self::qualify(project, p));
        }
        out.into_iter().collect()
    }

    /// Names are valid, every edge points at a defined branch, and the graph
    /// is acyclic (Kahn's algorithm). An undetected cycle would be an
    /// infinite compile, so this runs on every load and save.
    pub fn validate(&self) -> Result<(), BranchError> {
        let nodes: Vec<String> = self.all().into_iter().map(String::from).collect();
        for (project, branches) in &self.projects {
            for name in branches.keys() {
                let full = format!("{project}/{name}");
                if project.contains('/') || name.contains('/') || BranchRef::new(&full).is_err() {
                    return Err(BranchError::InvalidName(full));
                }
            }
        }
        let mut indegree: BTreeMap<&str, usize> = nodes.iter().map(|n| (n.as_str(), 0)).collect();
        let mut adjacency: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for n in &nodes {
            let targets = self.edges(n);
            for t in &targets {
                if t == n {
                    return Err(BranchError::Cycle(n.clone()));
                }
                match indegree.get_mut(t.as_str()) {
                    Some(d) => *d += 1,
                    None => {
                        return Err(BranchError::UnknownTarget {
                            branch: n.clone(),
                            target: t.clone(),
                        });
                    }
                }
            }
            adjacency.insert(n, targets);
        }
        let mut queue: VecDeque<&str> = indegree
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(n, _)| *n)
            .collect();
        let mut visited = 0;
        while let Some(n) = queue.pop_front() {
            visited += 1;
            for t in &adjacency[n] {
                let d = indegree.get_mut(t.as_str()).expect("validated above");
                *d -= 1;
                if *d == 0 {
                    queue.push_back(nodes.iter().find(|x| *x == t).expect("known node"));
                }
            }
        }
        if visited != nodes.len() {
            let stuck: Vec<&str> = indegree
                .iter()
                .filter(|(_, d)| **d > 0)
                .map(|(n, _)| *n)
                .collect();
            return Err(BranchError::Cycle(stuck.join(", ")));
        }
        Ok(())
    }

    /// Where `b`'s pool draws claims from (spec §7.3):
    ///
    /// ```text
    /// pool(b) = own(b)
    ///         ∪ ⋃ ancestors p: own(p) filtered to kinds allowed at EVERY hop
    ///         ∪ ⋃ laterals s:  own(s) filtered to inherits[s]   (non-transitive)
    /// ```
    ///
    /// A branch that isn't defined (e.g. `default`) sees only its own claims.
    pub fn pool_sources(&self, b: &BranchRef) -> Vec<PoolSource> {
        let mut sources: BTreeMap<String, Option<BTreeSet<Kind>>> = BTreeMap::new();
        sources.insert(b.as_str().to_owned(), None);

        let Some((project, _)) = b.as_str().split_once('/') else {
            return vec![PoolSource {
                branch: b.clone(),
                kinds: None,
            }];
        };
        let Some(def) = self.get(b) else {
            return vec![PoolSource {
                branch: b.clone(),
                kinds: None,
            }];
        };

        let add = |sources: &mut BTreeMap<String, Option<BTreeSet<Kind>>>,
                   name: String,
                   kinds: BTreeSet<Kind>| {
            if kinds.is_empty() {
                return;
            }
            match sources.entry(name).or_insert_with(|| Some(BTreeSet::new())) {
                None => {} // already sees all kinds (own branch)
                Some(set) => set.extend(kinds),
            }
        };

        // Laterals: every inherits key except the parent. Non-transitive.
        for (key, kinds) in &def.inherits {
            if Some(key) != def.parent.as_ref() {
                add(
                    &mut sources,
                    Self::qualify(project, key),
                    kinds.iter().copied().collect(),
                );
            }
        }

        // Ancestors: walk up, narrowing the allowed kinds at each hop.
        let all: BTreeSet<Kind> = Kind::ALL.into_iter().collect();
        let mut allowed: BTreeSet<Kind> = if def.holds.is_empty() {
            all.clone()
        } else {
            def.holds.iter().copied().collect()
        };
        let mut child_full = b.as_str().to_owned();
        let mut child = def;
        let mut seen = BTreeSet::from([child_full.clone()]);
        while let Some(parent) = &child.parent {
            let (child_project, _) = child_full.split_once('/').unwrap_or((project, ""));
            let parent_full = Self::qualify(child_project, parent);
            if !seen.insert(parent_full.clone()) {
                break; // defensive: validate() already rejects cycles
            }
            if let Some(scope) = child.inherits.get(parent) {
                allowed = allowed
                    .intersection(&scope.iter().copied().collect())
                    .copied()
                    .collect();
            }
            add(&mut sources, parent_full.clone(), allowed.clone());
            let Some(pdef) = BranchRef::new(&parent_full).ok().and_then(|r| self.get(&r)) else {
                break;
            };
            child = pdef;
            child_full = parent_full;
        }

        sources
            .into_iter()
            .filter_map(|(name, kinds)| {
                Some(PoolSource {
                    branch: BranchRef::new(&name).ok()?,
                    kinds: kinds.map(|k| k.into_iter().collect()),
                })
            })
            .collect()
    }

    /// Enforce `holds`: reject a kind the branch doesn't accept and suggest a
    /// sibling that does. This is what keeps branches clean.
    pub fn check_holds(&self, b: &BranchRef, kind: Kind) -> Result<(), BranchError> {
        let Some(def) = self.get(b) else {
            return Ok(()); // undefined branches (e.g. `default`) hold everything
        };
        if def.holds_kind(kind) {
            return Ok(());
        }
        let project = b
            .as_str()
            .split_once('/')
            .map(|(p, _)| p)
            .unwrap_or_default();
        let suggestion = self.projects.get(project).and_then(|bs| {
            bs.iter()
                .find(|(_, d)| !d.archived && d.holds_kind(kind))
                .map(|(name, _)| format!("{project}/{name}"))
        });
        Err(BranchError::KindNotHeld {
            branch: b.to_string(),
            kind,
            suggestion,
        })
    }

    /// Add a branch; fails if it exists or the result would be invalid.
    pub fn insert(&mut self, b: &BranchRef, def: BranchDef) -> Result<(), BranchError> {
        let (project, name) = b
            .as_str()
            .split_once('/')
            .filter(|(_, n)| !n.contains('/'))
            .ok_or_else(|| BranchError::InvalidName(b.to_string()))?;
        let branches = self.projects.entry(project.to_owned()).or_default();
        if branches.contains_key(name) {
            return Err(BranchError::Exists(b.to_string()));
        }
        branches.insert(name.to_owned(), def);
        if let Err(e) = self.validate() {
            if let Some(bs) = self.projects.get_mut(project) {
                bs.remove(name);
                if bs.is_empty() {
                    self.projects.remove(project);
                }
            }
            return Err(e);
        }
        Ok(())
    }

    pub fn get_mut(&mut self, b: &BranchRef) -> Option<&mut BranchDef> {
        let (project, name) = b.as_str().split_once('/')?;
        self.projects.get_mut(project)?.get_mut(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC_EXAMPLE: &str = "
sovereign:
  research:
    holds: [fact, question, claim, rejected, decision]
    budget: 1200
    compile: dossier
  code:
    parent: research
    holds: [decision, constraint, rejected, question]
    inherits:
      research: [decision, constraint]
    budget: 700
    compile: agents-md
    binds: [claude-code, codex, cursor]
  x-marketing:
    parent: research
    holds: [claim, decision, rejected, question]
    inherits:
      research: [decision, claim]
      code: [constraint]
    budget: 900
    compile: prose
";

    fn cfg(yaml: &str) -> Result<BranchConfig, BranchError> {
        let c: BranchConfig = serde_yaml::from_str(yaml).unwrap();
        c.validate().map(|_| c)
    }

    fn b(s: &str) -> BranchRef {
        BranchRef::new(s).unwrap()
    }

    fn source<'a>(p: &'a [PoolSource], name: &str) -> Option<&'a PoolSource> {
        p.iter().find(|s| s.branch.as_str() == name)
    }

    #[test]
    fn spec_example_resolves() {
        let c = cfg(SPEC_EXAMPLE).unwrap();
        let pool = c.pool_sources(&b("sovereign/code"));
        assert_eq!(source(&pool, "sovereign/code").unwrap().kinds, None);
        // code sees research's decisions and constraints, not its questions
        assert_eq!(
            source(&pool, "sovereign/research").unwrap().kinds,
            Some(vec![Kind::Decision, Kind::Constraint])
        );

        let pool = c.pool_sources(&b("sovereign/x-marketing"));
        assert_eq!(
            source(&pool, "sovereign/research").unwrap().kinds,
            Some(vec![Kind::Decision, Kind::Claim])
        );
        assert_eq!(
            source(&pool, "sovereign/code").unwrap().kinds,
            Some(vec![Kind::Constraint])
        );
    }

    #[test]
    fn ancestor_kinds_narrow_at_every_hop() {
        let c = cfg("
p:
  a: {}
  b: { parent: a, inherits: { a: [decision] } }
  c: { parent: b, inherits: { b: [decision, fact] } }
")
        .unwrap();
        let pool = c.pool_sources(&b("p/c"));
        assert_eq!(
            source(&pool, "p/b").unwrap().kinds,
            Some(vec![Kind::Fact, Kind::Decision])
        );
        // fact is dropped at the b→a hop
        assert_eq!(
            source(&pool, "p/a").unwrap().kinds,
            Some(vec![Kind::Decision])
        );
    }

    #[test]
    fn laterals_are_not_transitive() {
        let c = cfg("
p:
  a: {}
  b: { inherits: { a: [fact] } }
  c: { inherits: { b: [fact] } }
")
        .unwrap();
        let pool = c.pool_sources(&b("p/c"));
        assert!(source(&pool, "p/b").is_some());
        assert!(
            source(&pool, "p/a").is_none(),
            "lateral edges must not chain"
        );
    }

    #[test]
    fn cycles_are_rejected() {
        let err = cfg("
p:
  a: { parent: c }
  b: { parent: a }
  c: { parent: b }
")
        .unwrap_err();
        assert!(matches!(err, BranchError::Cycle(_)), "{err}");
        assert!(matches!(
            cfg("p:\n  a: { parent: a }\n").unwrap_err(),
            BranchError::Cycle(_)
        ));
        let lateral_cycle = cfg("
p:
  a: { inherits: { b: [fact] } }
  b: { inherits: { a: [fact] } }
");
        assert!(matches!(lateral_cycle.unwrap_err(), BranchError::Cycle(_)));
    }

    #[test]
    fn unknown_targets_are_rejected() {
        assert!(matches!(
            cfg("p:\n  a: { parent: nope }\n").unwrap_err(),
            BranchError::UnknownTarget { .. }
        ));
    }

    #[test]
    fn holds_suggests_a_sibling() {
        let c = cfg(SPEC_EXAMPLE).unwrap();
        c.check_holds(&b("sovereign/code"), Kind::Decision).unwrap();
        let err = c.check_holds(&b("sovereign/code"), Kind::Fact).unwrap_err();
        assert!(
            err.to_string().contains("try --to sovereign/research"),
            "{err}"
        );
        // Undefined branches hold everything.
        c.check_holds(&b("default"), Kind::Fact).unwrap();
    }

    #[test]
    fn insert_rolls_back_invalid() {
        let mut c = cfg(SPEC_EXAMPLE).unwrap();
        let bad = BranchDef {
            parent: Some("missing".into()),
            ..Default::default()
        };
        assert!(c.insert(&b("sovereign/new"), bad).is_err());
        assert!(!c.contains(&b("sovereign/new")));
        c.insert(&b("other/x"), BranchDef::default()).unwrap();
        assert!(matches!(
            c.insert(&b("other/x"), BranchDef::default()),
            Err(BranchError::Exists(_))
        ));
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refs/branches.yaml");
        let c = cfg(SPEC_EXAMPLE).unwrap();
        c.save(&path).unwrap();
        assert_eq!(BranchConfig::load(&path).unwrap(), c);
        assert_eq!(
            BranchConfig::load(&dir.path().join("none.yaml")).unwrap(),
            BranchConfig::default()
        );
    }
}
