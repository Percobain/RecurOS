//! Git sync for the store (spec §6.5), by shelling out to the `git` binary.
//!
//! We shell out rather than link libgit2: `git` is already installed wherever
//! a remote is useful, it honours the user's credential helpers and SSH
//! config for free, and it keeps the binary free of C build dependencies.
//!
//! Deviation from the spec: pulls use `--rebase`, not `--ff-only`. With two
//! machines each committing their own shard, histories diverge on every
//! round, so fast-forward-only would fail constantly. Because writers never
//! touch each other's files, a rebase can never conflict — if one does,
//! something wrote to a shard it doesn't own, and we abort and fail loudly,
//! which is the check the spec intended.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::home::CtxHome;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("git is not installed or not on PATH")]
    NoGit,
    #[error("git {args} failed: {stderr}")]
    Git { args: String, stderr: String },
    #[error("git {0} timed out")]
    Timeout(String),
    #[error(
        "pull could not be rebased cleanly, so another writer touched a file this machine owns. \
         The rebase was aborted and nothing was lost; inspect with `git -C {dir} log --all --stat`. \
         git said: {detail}"
    )]
    Conflict { dir: String, detail: String },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub committed: bool,
    pub pulled: bool,
    pub pushed: bool,
    /// `None` when no remote is configured: a normal, fully supported state.
    pub remote: Option<String>,
}

struct Output {
    ok: bool,
    stdout: String,
    stderr: String,
}

fn git(dir: &Path, args: &[&str], timeout: Option<Duration>) -> Result<Output, SyncError> {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(dir)
        // Never block on a credential prompt: sync runs from hooks.
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| SyncError::NoGit)?;

    let status = match timeout {
        None => child.wait().map_err(|_| SyncError::NoGit)?,
        Some(limit) => {
            let start = Instant::now();
            loop {
                if let Some(s) = child.try_wait().map_err(|_| SyncError::NoGit)? {
                    break s;
                }
                if start.elapsed() > limit {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(SyncError::Timeout(args.join(" ")));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut o) = child.stdout.take() {
        let _ = o.read_to_string(&mut stdout);
    }
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut stderr);
    }
    Ok(Output {
        ok: status.success(),
        stdout,
        stderr,
    })
}

fn git_ok(dir: &Path, args: &[&str], timeout: Option<Duration>) -> Result<String, SyncError> {
    let out = git(dir, args, timeout)?;
    if out.ok {
        Ok(out.stdout)
    } else {
        Err(SyncError::Git {
            args: args.join(" "),
            stderr: out.stderr.trim().to_owned(),
        })
    }
}

pub fn is_repo(home: &CtxHome) -> bool {
    home.root().join(".git").exists()
}

/// The URL of `origin` (or the first remote), if any.
pub fn remote_url(home: &CtxHome) -> Option<String> {
    if !is_repo(home) {
        return None;
    }
    let remotes = git_ok(home.root(), &["remote"], None).ok()?;
    let name = remotes
        .lines()
        .find(|r| *r == "origin")
        .or_else(|| remotes.lines().next())?
        .to_owned();
    git_ok(home.root(), &["remote", "get-url", &name], None)
        .ok()
        .map(|s| s.trim().to_owned())
}

fn remote_name(home: &CtxHome) -> Option<String> {
    let remotes = git_ok(home.root(), &["remote"], None).ok()?;
    remotes
        .lines()
        .find(|r| *r == "origin")
        .or_else(|| remotes.lines().next())
        .map(str::to_owned)
}

fn current_branch(home: &CtxHome) -> String {
    git_ok(home.root(), &["symbolic-ref", "--short", "HEAD"], None)
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|_| "main".to_owned())
}

/// Stage the store's tracked areas and commit if anything changed.
pub fn commit(home: &CtxHome, message: &str) -> Result<bool, SyncError> {
    if !is_repo(home) {
        return Ok(false);
    }
    let dir = home.root();
    let mut add = vec!["add", "-A", "--"];
    let candidates = ["log", "refs", "packs", ".gitattributes", ".gitignore"];
    for c in candidates {
        if dir.join(c).exists() {
            add.push(c);
        }
    }
    git_ok(dir, &add, None)?;
    let staged = git(dir, &["diff", "--cached", "--quiet"], None)?;
    if staged.ok {
        return Ok(false); // nothing to commit
    }
    let mut args = identity_args(dir);
    args.extend(["commit", "--quiet", "--no-verify", "-m", message]);
    git_ok(dir, &args, None)?;
    Ok(true)
}

/// Fallback identity for commands that create commits (commit, and the
/// rebase inside pull) when the user never configured one. The store is
/// theirs, and a missing identity must not block saving or syncing
/// history. Without this, a rebase on a fresh machine fails half way and
/// looks like a conflict.
fn identity_args(dir: &Path) -> Vec<&'static str> {
    let has_identity = git_ok(dir, &["config", "user.email"], None)
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if has_identity {
        Vec::new()
    } else {
        vec![
            "-c",
            "user.name=ContextOS",
            "-c",
            "user.email=ctx@localhost",
        ]
    }
}

/// Pull other machines' shards. Returns false (not an error) when there is
/// no remote or the remote is still empty.
pub fn pull(home: &CtxHome, timeout: Option<Duration>) -> Result<bool, SyncError> {
    if !is_repo(home) {
        return Ok(false);
    }
    let Some(remote) = remote_name(home) else {
        return Ok(false);
    };
    let branch = current_branch(home);
    let mut args = identity_args(home.root());
    args.extend([
        "pull",
        "--rebase",
        "--autostash",
        "--quiet",
        &remote,
        &branch,
    ]);
    let out = git(home.root(), &args, timeout)?;
    if out.ok {
        return Ok(true);
    }
    let err = out.stderr.to_lowercase();
    if err.contains("couldn't find remote ref") || err.contains("no such ref") {
        return Ok(false); // brand-new empty remote
    }
    if home.root().join(".git/rebase-merge").exists()
        || home.root().join(".git/rebase-apply").exists()
    {
        let _ = git(home.root(), &["rebase", "--abort"], None);
        return Err(SyncError::Conflict {
            dir: home.root().display().to_string(),
            detail: out.stderr.trim().to_owned(),
        });
    }
    Err(SyncError::Git {
        args: format!("pull --rebase {remote} {branch}"),
        stderr: out.stderr.trim().to_owned(),
    })
}

pub fn push(home: &CtxHome, timeout: Option<Duration>) -> Result<bool, SyncError> {
    if !is_repo(home) {
        return Ok(false);
    }
    let Some(remote) = remote_name(home) else {
        return Ok(false);
    };
    let branch = current_branch(home);
    let refspec = format!("HEAD:{branch}");
    git_ok(
        home.root(),
        &["push", "--quiet", "-u", &remote, &refspec],
        timeout,
    )?;
    Ok(true)
}

/// Commit local changes, pull others' (rebasing ours on top), push.
/// With no remote this just commits: offline is a normal state.
pub fn sync(home: &CtxHome, message: &str) -> Result<SyncReport, SyncError> {
    let committed = commit(home, message)?;
    let remote = remote_url(home);
    if remote.is_none() {
        return Ok(SyncReport {
            committed,
            ..Default::default()
        });
    }
    let pulled = pull(home, None)?;
    let pushed = push(home, None)?;
    Ok(SyncReport {
        committed,
        pulled,
        pushed,
        remote,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shard;
    use chrono::Utc;
    use ctx_core::{Claim, ClaimDraft, Kind, Record};
    use ulid::Ulid;

    fn has_git() -> bool {
        Command::new("git").arg("--version").output().is_ok()
    }

    fn save(home: &CtxHome, machine: &str, text: &str) {
        let now = Utc::now();
        let c = Claim::from_draft(ClaimDraft::new(Kind::Fact, text), Ulid::new(), now).unwrap();
        shard::append(home, machine, &Record::Claim(c), now).unwrap();
    }

    #[test]
    fn two_machines_converge_through_a_bare_remote() {
        if !has_git() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("remote.git");
        git_ok(
            dir.path(),
            &[
                "init",
                "--bare",
                "--quiet",
                "--initial-branch=main",
                bare.to_str().unwrap(),
            ],
            None,
        )
        .unwrap();

        let a = CtxHome::at(dir.path().join("a"));
        let b = CtxHome::at(dir.path().join("b"));
        for h in [&a, &b] {
            h.ensure().unwrap();
            git_ok(
                h.root(),
                &["remote", "add", "origin", bare.to_str().unwrap()],
                None,
            )
            .unwrap();
        }

        // No remote commits yet: sync must still succeed.
        save(&a, "a", "from a 1");
        let r = sync(&a, "a1").unwrap();
        assert!(r.committed && r.pushed);

        // Both write concurrently, then both sync: histories diverge and must
        // rebase cleanly because the shards are disjoint.
        save(&b, "b", "from b 1");
        save(&a, "a", "from a 2");
        sync(&a, "a2").unwrap();
        sync(&b, "b1").unwrap();
        sync(&a, "a3").unwrap();

        for h in [&a, &b] {
            let shards = shard::list_shards(h).unwrap();
            assert_eq!(shards.len(), 2, "{shards:?}");
            let n: usize = shards
                .iter()
                .map(|s| shard::read_from(h, s, 0).unwrap().records.len())
                .sum();
            assert_eq!(n, 3);
        }
    }

    #[test]
    fn no_remote_is_fine() {
        if !has_git() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let h = CtxHome::at(dir.path());
        h.ensure().unwrap();
        save(&h, "m", "x");
        let r = sync(&h, "msg").unwrap();
        assert!(r.committed);
        assert!(!r.pushed && r.remote.is_none());
        assert!(!sync(&h, "msg").unwrap().committed, "nothing new to commit");
    }
}
