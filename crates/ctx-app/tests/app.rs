//! End-to-end behaviour of the service layer against a real on-disk store.

use std::path::Path;
use std::time::Instant;

use chrono::{DateTime, Utc};
use ctx_app::{App, PackOpts, SaveOutcome, VerifyResult};
use ctx_branch::Binding;
use ctx_core::{BranchRef, Claim, ClaimDraft, Filter, Kind, Record, Status, Store};
use ctx_git::{CtxHome, shard};
use ctx_pack::Projection;
use ulid::Ulid;

fn setup() -> (tempfile::TempDir, CtxHome) {
    let dir = tempfile::tempdir().unwrap();
    let home = CtxHome::at(dir.path().join("ctx"));
    (dir, home)
}

fn b(s: &str) -> BranchRef {
    BranchRef::new(s).unwrap()
}

fn draft(branch: &str, kind: Kind, text: &str) -> ClaimDraft {
    let mut d = ClaimDraft::new(kind, text);
    d.branch = b(branch);
    d
}

fn saved(o: SaveOutcome) -> Claim {
    match o {
        SaveOutcome::Saved(c) => c,
        SaveOutcome::Duplicate(c) => panic!("unexpected duplicate {}", c.id),
    }
}

/// A repo dir bound to sovereign/code, with the standard research → code DAG.
fn bound_app(dir: &Path, home: &CtxHome) -> App {
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    Binding {
        project: "sovereign".into(),
        branch: "code".into(),
        root: repo.clone(),
    }
    .write(&repo)
    .unwrap();
    let mut app = App::open(home.clone(), Some(&repo)).unwrap();
    app.ensure_project("sovereign", "code").unwrap();
    app
}

#[test]
fn routing_holds_and_scoped_inheritance() {
    let (dir, home) = setup();
    let mut app = bound_app(dir.path(), &home);

    assert_eq!(app.default_branch().unwrap().as_str(), "sovereign/code");
    assert_eq!(
        app.resolve_branch(Some("research")).unwrap().as_str(),
        "sovereign/research"
    );

    // code doesn't hold claims-of-opinion; the error points at research.
    let err = app
        .save(draft("sovereign/code", Kind::Claim, "Peg is the future"))
        .unwrap_err();
    assert!(
        err.to_string().contains("try --to sovereign/research"),
        "{err}"
    );

    saved(
        app.save(draft(
            "sovereign/research",
            Kind::Decision,
            "Settle via a federation peg",
        ))
        .unwrap(),
    );
    saved(
        app.save(draft(
            "sovereign/research",
            Kind::Question,
            "Is peg latency acceptable?",
        ))
        .unwrap(),
    );
    saved(
        app.save(draft(
            "sovereign/code",
            Kind::Constraint,
            "Never block the event loop",
        ))
        .unwrap(),
    );

    // code inherits research's decisions, not its open questions.
    let pack = app.pack(&PackOpts::default()).unwrap();
    assert!(
        pack.markdown.contains("Settle via a federation peg"),
        "{}",
        pack.markdown
    );
    assert!(pack.markdown.contains("_(research)_"));
    assert!(!pack.markdown.contains("peg latency"));
    assert!(pack.markdown.contains("Never block the event loop"));
    assert!(pack.tokens <= 700, "code branch budget");

    // research sees its own questions.
    let research = app
        .pack(&PackOpts {
            branch: Some("research".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(research.markdown.contains("peg latency"));
    assert!(
        research.markdown.starts_with("# Context dossier"),
        "research compiles to a dossier"
    );
}

#[test]
fn proposals_stay_out_of_packs_until_accepted_and_rejections_keep_reasons() {
    let (dir, home) = setup();
    let mut app = bound_app(dir.path(), &home);
    let p = saved(
        app.propose(draft(
            "sovereign/research",
            Kind::Decision,
            "Adopt covenants",
        ))
        .unwrap(),
    );
    let q = saved(
        app.propose(draft(
            "sovereign/research",
            Kind::Fact,
            "Peg confirms in 2 blocks",
        ))
        .unwrap(),
    );
    assert_eq!(app.proposals(None).unwrap().len(), 2);
    let research = PackOpts {
        branch: Some("research".into()),
        ..Default::default()
    };
    assert!(
        !app.pack(&research)
            .unwrap()
            .markdown
            .contains("Adopt covenants")
    );

    app.transition(&q, Status::Active, None, "test").unwrap();
    app.transition(
        &p,
        Status::Rejected,
        Some("too slow to confirm".into()),
        "test",
    )
    .unwrap();
    assert!(app.proposals(None).unwrap().is_empty());
    let md = app.pack(&research).unwrap().markdown;
    assert!(md.contains("Peg confirms in 2 blocks"));
    assert!(!md.contains("Adopt covenants"));

    let history = app.store.status_history(p.id).unwrap();
    assert_eq!(history[0].reason.as_deref(), Some("too slow to confirm"));
    assert_eq!(
        app.store.get(p.id).unwrap().unwrap().status,
        Status::Rejected
    );
}

#[test]
fn supersede_merge_archive() {
    let (dir, home) = setup();
    let mut app = bound_app(dir.path(), &home);
    let old = saved(
        app.save(draft("sovereign/code", Kind::Decision, "Use Postgres"))
            .unwrap(),
    );
    let mut d = draft("sovereign/code", Kind::Decision, "Use SQLite");
    d.supersedes = Some(old.id);
    saved(app.save(d).unwrap());
    assert_eq!(
        app.store.get(old.id).unwrap().unwrap().status,
        Status::Superseded
    );

    let handoff = app
        .pack(&PackOpts {
            projection: Some(Projection::Prose),
            ..Default::default()
        })
        .unwrap();
    assert!(
        handoff.markdown.contains("Previously: Use Postgres"),
        "{}",
        handoff.markdown
    );

    // Merge code → research appends copies; originals stay.
    let (merged, _) = app
        .merge(&b("sovereign/code"), &b("sovereign/research"))
        .unwrap();
    assert_eq!(merged, 1, "only the active decision");
    let (again, skipped) = app
        .merge(&b("sovereign/code"), &b("sovereign/research"))
        .unwrap();
    assert_eq!((again, skipped), (0, 1), "merge is idempotent");
    assert_eq!(
        app.store
            .scan(&Filter {
                branch: Some(b("sovereign/code")),
                statuses: vec![Status::Active],
                ..Default::default()
            })
            .unwrap()
            .len(),
        1
    );

    let n = app.branch_archive(&b("sovereign/research")).unwrap();
    assert_eq!(n, 1);
    assert!(app.branches.get(&b("sovereign/research")).unwrap().archived);
    assert_eq!(
        app.store.stats().unwrap().claims,
        3,
        "archive never deletes"
    );
}

#[test]
fn agents_md_is_refreshed_without_clobbering_user_content() {
    let (dir, home) = setup();
    let mut app = bound_app(dir.path(), &home);
    let agents = dir.path().join("repo/AGENTS.md");
    std::fs::write(&agents, "# House rules\n\nUse tabs.\n").unwrap();

    saved(
        app.save(draft(
            "sovereign/code",
            Kind::Constraint,
            "No unwrap in library code",
        ))
        .unwrap(),
    );
    let block = app.refresh_agents_md().unwrap().expect("changed");
    assert!(block.contains("No unwrap in library code"));
    let text = std::fs::read_to_string(&agents).unwrap();
    assert!(text.starts_with("# House rules\n\nUse tabs.\n"));
    assert!(text.contains("### Working with ctx"));
    assert!(
        app.refresh_agents_md().unwrap().is_none(),
        "unchanged the second time"
    );
}

#[test]
fn packs_are_written_where_the_worker_reads_them() {
    let (dir, home) = setup();
    let mut app = bound_app(dir.path(), &home);
    saved(
        app.save(draft(
            "sovereign/research",
            Kind::Fact,
            "Blocks are one minute",
        ))
        .unwrap(),
    );
    assert!(app.write_packs().unwrap() > 0);
    let research =
        std::fs::read_to_string(home.root().join("packs/sovereign/research.md")).unwrap();
    assert!(research.contains("Blocks are one minute"));
    assert!(
        research.contains("ctx-claims"),
        "dossiers carry the save protocol"
    );
    assert!(home.root().join("packs/index.md").exists());
    assert_eq!(
        app.write_packs().unwrap(),
        0,
        "unchanged packs are not rewritten"
    );
}

#[test]
fn verify_detects_behind() {
    let (dir, home) = setup();
    let mut app = bound_app(dir.path(), &home);
    let code = b("sovereign/code");
    saved(
        app.save(draft("sovereign/code", Kind::Decision, "a"))
            .unwrap(),
    );
    let old_root = app.state_root(&code).unwrap();
    saved(
        app.save(draft("sovereign/code", Kind::Decision, "b"))
            .unwrap(),
    );
    assert_eq!(
        app.verify(&code, &app.state_root(&code).unwrap()).unwrap(),
        VerifyResult::InSync
    );
    assert_eq!(
        app.verify(&code, &old_root).unwrap(),
        VerifyResult::Behind(1)
    );
    assert_eq!(
        app.verify(&code, "b3:ffff").unwrap(),
        VerifyResult::AheadOrDiverged
    );
}

#[test]
fn claims_from_another_machine_and_cache_loss() {
    let (_d, home) = setup();
    let mut app = App::open(home.clone(), None).unwrap();
    let first = saved(
        app.save(ClaimDraft::new(Kind::Decision, "Use SQLite"))
            .unwrap(),
    );
    assert!(matches!(
        app.save(ClaimDraft::new(Kind::Decision, "Use SQLite \r\n")).unwrap(),
        SaveOutcome::Duplicate(c) if c.id == first.id
    ));

    let now = Utc::now();
    let remote =
        Claim::from_draft(ClaimDraft::new(Kind::Fact, "from laptop"), Ulid::new(), now).unwrap();
    shard::append(&home, "laptop", &Record::Claim(remote.clone()), now).unwrap();
    app.catch_up().unwrap();
    assert_eq!(app.store.get(remote.id).unwrap().unwrap(), remote);
    drop(app);

    std::fs::remove_dir_all(home.cache_dir()).unwrap();
    let app = App::open(home, None).unwrap();
    assert_eq!(
        app.store.stats().unwrap().claims,
        2,
        "killing the cache loses nothing"
    );
}

#[test]
fn reindex_10k_claims_is_fast() {
    const N: usize = 10_000;
    let (_d, home) = setup();
    home.ensure().unwrap();
    // Build shards in memory and write each once: 10k fsync'd appends would
    // make the setup, not the thing under test, dominate the runtime.
    let mut shards: std::collections::BTreeMap<std::path::PathBuf, String> = Default::default();
    let t0 = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    for i in 0..N {
        let t = t0 + chrono::Duration::minutes(i as i64 * 30);
        let mut d = ClaimDraft::new(
            Kind::ALL[i % 6],
            format!("claim number {i} about topic {}", i % 97),
        );
        if i % 3 == 0 {
            d.why = Some(format!(
                "because reason {i} matters for the settlement design"
            ));
        }
        d.entities = vec![format!("topic{}", i % 13)];
        let c = Claim::from_draft(
            d,
            Ulid::from_parts(t.timestamp_millis() as u64, i as u128),
            t,
        )
        .unwrap();
        let line = serde_json::to_string(&Record::Claim(c)).unwrap();
        let buf = shards
            .entry(shard::shard_path(&home, &format!("m{}", i % 3), t))
            .or_default();
        buf.push_str(&line);
        buf.push('\n');
    }
    for (path, body) in &shards {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    let mut app = App::open(home, None).unwrap();
    let start = Instant::now();
    app.reindex().unwrap();
    let elapsed = start.elapsed();
    assert_eq!(app.store.stats().unwrap().claims, N as u64);
    // Spec §15.1: reindex under 3s at 10k claims. That budget is for the
    // shipped (release) binary; CI runs this with --release. Debug builds
    // only get a loose bound against pathologies.
    let budget = if cfg!(debug_assertions) { 20.0 } else { 3.0 };
    eprintln!("reindex of {N} claims: {elapsed:?}");
    assert!(elapsed.as_secs_f64() < budget, "reindex took {elapsed:?}");
}
