//! SQLite implementation of [`ctx_core::Store`].
//!
//! This is the only crate in the workspace allowed to contain SQL. The database
//! is a cache: every row is recoverable from the log, so on any schema change
//! we simply drop everything and let the caller re-ingest.
//!
//! Claims are immutable, but their *effective* status and counters are
//! materialised here: `status` is the lattice join of the claim's own status
//! and every status record targeting it (including implied `superseded` from
//! a later claim's `supersedes`). Because join is commutative, records may
//! arrive in any order — a transition read before its claim is simply applied
//! when the claim shows up.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use chrono::{DateTime, SecondsFormat, Utc};
use ctx_core::{
    BranchRef, BranchSummary, Claim, CounterUpdate, Filter, Record, Stats, Status, StatusChange,
    Store,
};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use thiserror::Error;
use ulid::Ulid;

/// Bump whenever the schema changes; a mismatch triggers a full rebuild.
const SCHEMA_VERSION: i64 = 2;

const SCHEMA: &str = "
CREATE TABLE claims (
  id TEXT PRIMARY KEY, cid TEXT NOT NULL, kind TEXT NOT NULL, branch TEXT NOT NULL,
  status TEXT NOT NULL, base_status TEXT NOT NULL,
  tokens INTEGER NOT NULL, confidence TEXT NOT NULL, src TEXT NOT NULL,
  t_valid TEXT NOT NULL, t_tx TEXT NOT NULL, last_used TEXT,
  helpful INTEGER NOT NULL DEFAULT 0, harmful INTEGER NOT NULL DEFAULT 0,
  text TEXT NOT NULL, why TEXT, entities TEXT NOT NULL,
  body TEXT NOT NULL
);
CREATE INDEX idx_branch_status ON claims(branch, status);
CREATE INDEX idx_kind ON claims(kind);
CREATE INDEX idx_cid_branch ON claims(cid, branch);

CREATE VIRTUAL TABLE claims_fts USING fts5(
  text, why, entities,
  content='claims', content_rowid='rowid',
  tokenize='porter unicode61 remove_diacritics 2'
);

-- supersedes | merged-from
CREATE TABLE edges (src TEXT NOT NULL, dst TEXT NOT NULL, kind TEXT NOT NULL);

-- Real status records, plus synthetic 'sup:<id>' rows implied by supersedes.
CREATE TABLE status_changes (
  id TEXT PRIMARY KEY, claim TEXT NOT NULL, to_status TEXT NOT NULL,
  reason TEXT, src TEXT NOT NULL, t_tx TEXT NOT NULL
);
CREATE INDEX idx_status_claim ON status_changes(claim);

CREATE TABLE counters (
  claim TEXT NOT NULL, machine TEXT NOT NULL, rec_id TEXT NOT NULL,
  helpful INTEGER NOT NULL, harmful INTEGER NOT NULL,
  PRIMARY KEY (claim, machine)
);

CREATE TABLE cursors (shard TEXT PRIMARY KEY, offset INTEGER NOT NULL);
";

const DROP: &str = "
DROP TABLE IF EXISTS claims_fts;
DROP TABLE IF EXISTS claims;
DROP TABLE IF EXISTS edges;
DROP TABLE IF EXISTS status_changes;
DROP TABLE IF EXISTS counters;
DROP TABLE IF EXISTS cursors;
";

#[derive(Debug, Error)]
pub enum SqliteError {
    #[error("sqlite: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("corrupt index row: {0}")]
    Body(#[from] serde_json::Error),
    #[error("corrupt index row: {0}")]
    Value(#[from] ctx_core::CoreError),
}

pub struct SqliteStore {
    conn: Connection,
}

impl SqliteStore {
    /// Open (creating if needed) the index at `path`. If the file was built by
    /// a different schema version it is wiped; callers then catch up from the
    /// log as they would on a fresh index.
    pub fn open(path: &Path) -> Result<Self, SqliteError> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, SqliteError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, SqliteError> {
        // It's a cache: durability is the log's job, so favour speed.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut store = SqliteStore { conn };
        let version: i64 = store
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != SCHEMA_VERSION {
            store.reset()?;
        }
        Ok(store)
    }

    fn reset(&mut self) -> Result<(), SqliteError> {
        let tx = self.conn.transaction()?;
        tx.execute_batch(DROP)?;
        tx.execute_batch(SCHEMA)?;
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
        Ok(())
    }

    /// Run a query selecting `(body, status)` rows and rehydrate claims with
    /// their effective status and counters.
    fn query_claims(&self, sql: &str, params: &[Value]) -> Result<Vec<Claim>, SqliteError> {
        let mut stmt = self.conn.prepare_cached(sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let counters = self.counters_for(rows.len())?;
        rows.into_iter()
            .map(|(body, status)| {
                let mut c: Claim = serde_json::from_str(&body)?;
                c.status = status.parse()?;
                if let Some(per_machine) = counters.get(&c.id.to_string()) {
                    for (machine, (h, x)) in per_machine {
                        c.helpful.insert(machine.clone(), *h);
                        c.harmful.insert(machine.clone(), *x);
                    }
                }
                Ok(c)
            })
            .collect()
    }

    /// Counters are rare (only explicit `ctx rate`), so load them all at once
    /// rather than one query per claim.
    #[allow(clippy::type_complexity)]
    fn counters_for(
        &self,
        n_rows: usize,
    ) -> Result<HashMap<String, BTreeMap<String, (u32, u32)>>, SqliteError> {
        let mut out: HashMap<String, BTreeMap<String, (u32, u32)>> = HashMap::new();
        if n_rows == 0 {
            return Ok(out);
        }
        let mut stmt = self
            .conn
            .prepare_cached("SELECT claim, machine, helpful, harmful FROM counters")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u32>(2)?,
                r.get::<_, u32>(3)?,
            ))
        })?;
        for row in rows {
            let (claim, machine, h, x) = row?;
            out.entry(claim).or_default().insert(machine, (h, x));
        }
        Ok(out)
    }
}

/// Fixed-width timestamp so string comparison in SQL is chronological.
fn ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

fn insert_claim(tx: &Transaction, c: &Claim) -> Result<(), SqliteError> {
    let entities = c.entities.join(" ");
    let inserted = tx
        .prepare_cached(
            "INSERT OR IGNORE INTO claims
         (id, cid, kind, branch, status, base_status, tokens, confidence, src, t_valid, t_tx,
          last_used, text, why, entities, body)
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )?
        .execute(params![
            c.id.to_string(),
            c.cid,
            c.kind.as_str(),
            c.branch.as_str(),
            c.status.as_str(),
            c.status.as_str(),
            c.tokens,
            c.confidence.as_str(),
            c.src,
            ts(c.t_valid),
            ts(c.t_tx),
            c.last_used.map(ts),
            c.text,
            c.why,
            entities,
            serde_json::to_string(c)?,
        ])?;
    if inserted == 0 {
        return Ok(()); // already indexed (duplicate line)
    }
    tx.prepare_cached("INSERT INTO claims_fts(rowid, text, why, entities) VALUES (?,?,?,?)")?
        .execute(params![tx.last_insert_rowid(), c.text, c.why, entities])?;
    if let Some(old) = c.supersedes {
        tx.prepare_cached("INSERT INTO edges(src, dst, kind) VALUES (?,?,'supersedes')")?
            .execute(params![c.id.to_string(), old.to_string()])?;
        tx.prepare_cached(
            "INSERT OR IGNORE INTO status_changes(id, claim, to_status, reason, src, t_tx)
             VALUES (?, ?, 'superseded', ?, ?, ?)",
        )?
        .execute(params![
            format!("sup:{}", c.id),
            old.to_string(),
            format!("superseded by {}", c.id),
            c.src,
            ts(c.t_tx),
        ])?;
        refresh_status(tx, &old.to_string())?;
    }
    for r in &c.refs {
        if let Some(src) = r.strip_prefix("ctx:") {
            tx.prepare_cached("INSERT INTO edges(src, dst, kind) VALUES (?,?,'merged-from')")?
                .execute(params![c.id.to_string(), src])?;
        }
    }
    refresh_status(tx, &c.id.to_string())?;
    refresh_counters(tx, &c.id.to_string())?;
    Ok(())
}

fn insert_status(tx: &Transaction, s: &StatusChange) -> Result<(), SqliteError> {
    tx.prepare_cached(
        "INSERT OR IGNORE INTO status_changes(id, claim, to_status, reason, src, t_tx)
         VALUES (?,?,?,?,?,?)",
    )?
    .execute(params![
        s.id.to_string(),
        s.claim.to_string(),
        s.to.as_str(),
        s.reason,
        s.src,
        ts(s.t_tx),
    ])?;
    refresh_status(tx, &s.claim.to_string())
}

fn insert_counter(tx: &Transaction, u: &CounterUpdate) -> Result<(), SqliteError> {
    // G-counter merge: per-machine max, independently for each field.
    tx.prepare_cached(
        "INSERT INTO counters(claim, machine, rec_id, helpful, harmful) VALUES (?,?,?,?,?)
         ON CONFLICT(claim, machine) DO UPDATE SET
           helpful = max(helpful, excluded.helpful),
           harmful = max(harmful, excluded.harmful),
           rec_id = max(rec_id, excluded.rec_id)",
    )?
    .execute(params![
        u.claim.to_string(),
        u.machine,
        u.id.to_string(),
        u.helpful,
        u.harmful
    ])?;
    refresh_counters(tx, &u.claim.to_string())
}

/// Recompute a claim's effective status as the join of its own status and
/// all transitions targeting it. No-op if the claim hasn't arrived yet.
fn refresh_status(tx: &Transaction, id: &str) -> Result<(), SqliteError> {
    let base: Option<String> = tx
        .prepare_cached("SELECT base_status FROM claims WHERE id = ?")?
        .query_row([id], |r| r.get(0))
        .optional()?;
    let Some(base) = base else {
        return Ok(());
    };
    let mut status: Status = base.parse()?;
    let mut stmt = tx.prepare_cached("SELECT to_status FROM status_changes WHERE claim = ?")?;
    for to in stmt.query_map([id], |r| r.get::<_, String>(0))? {
        status = status.join(to?.parse()?);
    }
    tx.prepare_cached("UPDATE claims SET status = ? WHERE id = ?")?
        .execute(params![status.as_str(), id])?;
    Ok(())
}

fn refresh_counters(tx: &Transaction, id: &str) -> Result<(), SqliteError> {
    tx.prepare_cached(
        "UPDATE claims SET
           helpful = coalesce((SELECT sum(helpful) FROM counters WHERE claim = ?1), 0),
           harmful = coalesce((SELECT sum(harmful) FROM counters WHERE claim = ?1), 0)
         WHERE id = ?1",
    )?
    .execute([id])?;
    Ok(())
}

/// Append WHERE clauses for `filter` (table alias `c`). Returns the SQL
/// fragment (starting with " AND ...") and pushes bind values.
fn filter_sql(filter: &Filter, params: &mut Vec<Value>) -> String {
    let mut sql = String::new();
    if let Some(b) = &filter.branch {
        sql.push_str(" AND c.branch = ?");
        params.push(Value::Text(b.as_str().to_owned()));
    }
    if !filter.kinds.is_empty() {
        sql.push_str(&format!(
            " AND c.kind IN ({})",
            vec!["?"; filter.kinds.len()].join(",")
        ));
        params.extend(
            filter
                .kinds
                .iter()
                .map(|k| Value::Text(k.as_str().to_owned())),
        );
    }
    if !filter.statuses.is_empty() {
        sql.push_str(&format!(
            " AND c.status IN ({})",
            vec!["?"; filter.statuses.len()].join(",")
        ));
        params.extend(
            filter
                .statuses
                .iter()
                .map(|s| Value::Text(s.as_str().to_owned())),
        );
    }
    if let Some(since) = filter.since {
        sql.push_str(" AND c.t_tx >= ?");
        params.push(Value::Text(ts(since)));
    }
    sql
}

/// Turn free text into a safe FTS5 query: each word becomes a quoted prefix
/// term, OR-ed together so bm25 ranks claims matching more words higher.
/// Quoting means user input can never be parsed as FTS5 syntax.
fn fts_query(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\"*"))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" OR "))
}

impl Store for SqliteStore {
    type Error = SqliteError;

    fn ingest(
        &mut self,
        shard: &str,
        records: &[Record],
        next_offset: u64,
    ) -> Result<(), SqliteError> {
        let tx = self.conn.transaction()?;
        for r in records {
            match r {
                Record::Claim(c) => insert_claim(&tx, c)?,
                Record::Status(s) => insert_status(&tx, s)?,
                Record::Counter(u) => insert_counter(&tx, u)?,
            }
        }
        tx.execute(
            "INSERT INTO cursors(shard, offset) VALUES (?1, ?2)
             ON CONFLICT(shard) DO UPDATE SET offset = excluded.offset",
            params![shard, next_offset as i64],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn cursor(&self, shard: &str) -> Result<u64, SqliteError> {
        let off: Option<i64> = self
            .conn
            .query_row("SELECT offset FROM cursors WHERE shard = ?", [shard], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(off.unwrap_or(0) as u64)
    }

    fn get(&self, id: Ulid) -> Result<Option<Claim>, SqliteError> {
        Ok(self
            .query_claims(
                "SELECT body, status FROM claims WHERE id = ?",
                &[Value::Text(id.to_string())],
            )?
            .pop())
    }

    fn resolve(&self, prefix: &str) -> Result<Vec<Claim>, SqliteError> {
        let p = prefix.trim();
        let hex = p
            .strip_prefix("c:")
            .or_else(|| p.strip_prefix("b3:"))
            .unwrap_or(p)
            .to_ascii_lowercase();
        if hex.is_empty() {
            return Ok(Vec::new());
        }
        let id_prefix = p.to_ascii_uppercase();
        self.query_claims(
            "SELECT body, status FROM claims
             WHERE substr(id, 1, length(?1)) = ?1 OR substr(cid, 1, length(?2)) = ?2
             ORDER BY id LIMIT 20",
            &[Value::Text(id_prefix), Value::Text(format!("b3:{hex}"))],
        )
    }

    fn find_by_cid(&self, cid: &str, branch: &BranchRef) -> Result<Option<Claim>, SqliteError> {
        Ok(self
            .query_claims(
                "SELECT body, status FROM claims WHERE cid = ? AND branch = ? ORDER BY id LIMIT 1",
                &[
                    Value::Text(cid.to_owned()),
                    Value::Text(branch.as_str().to_owned()),
                ],
            )?
            .pop())
    }

    fn scan(&self, filter: &Filter) -> Result<Vec<Claim>, SqliteError> {
        let mut params = Vec::new();
        let where_ = filter_sql(filter, &mut params);
        // With a limit, keep the most recent N but still return oldest first.
        let sql = match filter.limit {
            Some(n) => format!(
                "SELECT body, status FROM (SELECT c.id, c.body, c.status FROM claims c
                 WHERE 1=1{where_} ORDER BY c.id DESC LIMIT {n}) ORDER BY id ASC"
            ),
            None => {
                format!("SELECT c.body, c.status FROM claims c WHERE 1=1{where_} ORDER BY c.id ASC")
            }
        };
        self.query_claims(&sql, &params)
    }

    fn search(&self, query: &str, filter: &Filter) -> Result<Vec<Claim>, SqliteError> {
        let Some(q) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let mut params = vec![Value::Text(q)];
        let where_ = filter_sql(filter, &mut params);
        let limit = filter.limit.unwrap_or(50);
        // bm25 column weights: entities are curated tags, so a tag hit counts
        // most; `why` is supporting prose, so it counts least.
        let sql = format!(
            "SELECT c.body, c.status FROM claims_fts f JOIN claims c ON c.rowid = f.rowid
             WHERE claims_fts MATCH ?{where_}
             ORDER BY bm25(claims_fts, 1.0, 0.5, 2.0), c.id LIMIT {limit}"
        );
        self.query_claims(&sql, &params)
    }

    fn status_history(&self, claim: Ulid) -> Result<Vec<StatusChange>, SqliteError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, to_status, reason, src, t_tx FROM status_changes
             WHERE claim = ? AND id NOT LIKE 'sup:%' ORDER BY id",
        )?;
        let rows = stmt
            .query_map([claim.to_string()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .filter_map(|(id, to, reason, src, t)| {
                let id = Ulid::from_string(&id).ok()?;
                let t_tx = DateTime::parse_from_rfc3339(&t).ok()?.with_timezone(&Utc);
                Some(to.parse().map(|to| StatusChange {
                    id,
                    claim,
                    to,
                    reason,
                    src,
                    t_tx,
                }))
            })
            .collect::<Result<_, _>>()
            .map_err(SqliteError::from)
    }

    fn branches(&self) -> Result<Vec<BranchSummary>, SqliteError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT branch, sum(status = 'active'), sum(status = 'proposed'), count(*), max(t_tx)
             FROM claims GROUP BY branch ORDER BY branch",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (branch, active, proposed, total, last) = row?;
            out.push(BranchSummary {
                branch,
                active: active as u64,
                proposed: proposed as u64,
                total: total as u64,
                last_tx: last
                    .and_then(|t| DateTime::parse_from_rfc3339(&t).ok())
                    .map(|t| t.with_timezone(&Utc)),
            });
        }
        Ok(out)
    }

    fn generation(&self) -> Result<Option<Ulid>, SqliteError> {
        let max: Option<String> = self.conn.query_row(
            "SELECT max(m) FROM (
               SELECT max(id) AS m FROM claims
               UNION ALL SELECT max(id) FROM status_changes WHERE id NOT LIKE 'sup:%'
               UNION ALL SELECT max(rec_id) FROM counters)",
            [],
            |r| r.get(0),
        )?;
        Ok(max.and_then(|s| Ulid::from_string(&s).ok()))
    }

    fn rebuild(&mut self) -> Result<(), SqliteError> {
        self.reset()
    }

    fn stats(&self) -> Result<Stats, SqliteError> {
        let mut stats = Stats {
            claims: self
                .conn
                .query_row("SELECT count(*) FROM claims", [], |r| r.get::<_, i64>(0))?
                as u64,
            shards: self
                .conn
                .query_row("SELECT count(*) FROM cursors", [], |r| r.get::<_, i64>(0))?
                as u64,
            ..Default::default()
        };
        for (col, map) in [
            ("kind", &mut stats.by_kind),
            ("branch", &mut stats.by_branch),
        ] {
            let mut stmt = self.conn.prepare(&format!(
                "SELECT {col}, count(*) FROM claims GROUP BY {col}"
            ))?;
            for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
                let (k, n) = row?;
                map.insert(k, n as u64);
            }
        }
        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_core::{ClaimDraft, Kind};

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn claim(kind: Kind, text: &str, why: Option<&str>, t: &str) -> Claim {
        let mut d = ClaimDraft::new(kind, text);
        d.why = why.map(str::to_owned);
        let now = at(t);
        Claim::from_draft(d, Ulid::from_parts(now.timestamp_millis() as u64, 1), now).unwrap()
    }

    fn recs(claims: &[Claim]) -> Vec<Record> {
        claims.iter().cloned().map(Record::Claim).collect()
    }

    fn store_with(claims: &[Claim]) -> SqliteStore {
        let mut s = SqliteStore::open_in_memory().unwrap();
        s.ingest("log/m/2026-09.jsonl", &recs(claims), 123).unwrap();
        s
    }

    fn status(id: Ulid, claim: Ulid, to: Status) -> Record {
        Record::Status(StatusChange {
            id,
            claim,
            to,
            reason: Some("because".into()),
            src: "cli".into(),
            t_tx: at("2026-09-10T00:00:00Z"),
        })
    }

    #[test]
    fn ingest_is_idempotent_and_advances_cursor() {
        let c = claim(Kind::Fact, "alpha", None, "2026-09-01T00:00:00Z");
        let mut s = store_with(std::slice::from_ref(&c));
        s.ingest("log/m/2026-09.jsonl", &recs(std::slice::from_ref(&c)), 456)
            .unwrap();
        assert_eq!(s.stats().unwrap().claims, 1);
        assert_eq!(s.cursor("log/m/2026-09.jsonl").unwrap(), 456);
        assert_eq!(s.cursor("other").unwrap(), 0);
        assert_eq!(s.get(c.id).unwrap().unwrap(), c);
    }

    #[test]
    fn status_converges_in_any_order() {
        let c = claim(Kind::Fact, "alpha", None, "2026-09-01T00:00:00Z");
        let archive = status(Ulid::from_parts(5, 5), c.id, Status::Archived);
        let supersede = status(Ulid::from_parts(6, 6), c.id, Status::Superseded);
        let orders = [
            vec![Record::Claim(c.clone()), archive.clone(), supersede.clone()],
            vec![supersede.clone(), archive.clone(), Record::Claim(c.clone())],
            vec![archive.clone(), Record::Claim(c.clone()), supersede.clone()],
        ];
        for order in orders {
            let mut s = SqliteStore::open_in_memory().unwrap();
            // Each record in its own ingest, as if from different shards.
            for (i, r) in order.iter().enumerate() {
                s.ingest(&format!("shard{i}"), std::slice::from_ref(r), 1)
                    .unwrap();
            }
            assert_eq!(s.get(c.id).unwrap().unwrap().status, Status::Archived);
            assert_eq!(s.status_history(c.id).unwrap().len(), 2);
        }
    }

    #[test]
    fn supersedes_marks_old_claim() {
        let old = claim(Kind::Decision, "use x", None, "2026-09-01T00:00:00Z");
        let mut d = ClaimDraft::new(Kind::Decision, "use y");
        d.supersedes = Some(old.id);
        let t = at("2026-09-02T00:00:00Z");
        let new =
            Claim::from_draft(d, Ulid::from_parts(t.timestamp_millis() as u64, 2), t).unwrap();
        // New claim arrives first: old one must still end up superseded.
        let s = store_with(&[new.clone(), old.clone()]);
        assert_eq!(s.get(old.id).unwrap().unwrap().status, Status::Superseded);
        assert_eq!(s.get(new.id).unwrap().unwrap().status, Status::Active);
    }

    #[test]
    fn counters_merge_by_per_machine_max() {
        let c = claim(Kind::Fact, "alpha", None, "2026-09-01T00:00:00Z");
        let mut s = store_with(std::slice::from_ref(&c));
        let upd = |id: u128, machine: &str, h: u32| {
            Record::Counter(CounterUpdate {
                id: Ulid::from_parts(9, id),
                claim: c.id,
                machine: machine.into(),
                helpful: h,
                harmful: 0,
                t_tx: at("2026-09-03T00:00:00Z"),
            })
        };
        s.ingest("x", &[upd(1, "a", 3), upd(2, "a", 1), upd(3, "b", 2)], 1)
            .unwrap();
        let got = s.get(c.id).unwrap().unwrap();
        assert_eq!(got.helpful_total(), 5);
    }

    #[test]
    fn resolve_by_id_or_cid_prefix() {
        let c = claim(Kind::Fact, "alpha", None, "2026-09-01T00:00:00Z");
        let s = store_with(std::slice::from_ref(&c));
        let id = c.id.to_string();
        assert_eq!(s.resolve(&id[..10].to_lowercase()).unwrap().len(), 1);
        assert_eq!(s.resolve(&format!("c:{}", c.short_cid())).unwrap().len(), 1);
        assert!(s.resolve("zzzz").unwrap().is_empty());
    }

    #[test]
    fn search_ranks_and_survives_fts_syntax() {
        let s = store_with(&[
            claim(
                Kind::Decision,
                "Settlement uses a federation peg",
                Some("covenants are too slow"),
                "2026-09-01T00:00:00Z",
            ),
            claim(
                Kind::Fact,
                "The registry is sqlite",
                None,
                "2026-09-02T00:00:00Z",
            ),
        ]);
        assert_eq!(s.search("settlement", &Filter::default()).unwrap().len(), 1);
        // matches `why`, and porter stemming: covenant ~ covenants
        assert_eq!(s.search("covenant", &Filter::default()).unwrap().len(), 1);
        for q in ["\"unbalanced", "a AND OR NOT", "col:x", "*", "NEAR(", ""] {
            s.search(q, &Filter::default()).unwrap();
        }
    }

    #[test]
    fn scan_filters_and_limits_keep_most_recent() {
        let s = store_with(&[
            claim(Kind::Fact, "one", None, "2026-09-01T00:00:00Z"),
            claim(Kind::Decision, "two", None, "2026-09-02T00:00:00Z"),
            claim(Kind::Fact, "three", None, "2026-09-03T00:00:00.5Z"),
        ]);
        let texts = |f: Filter| {
            s.scan(&f)
                .unwrap()
                .into_iter()
                .map(|c| c.text)
                .collect::<Vec<_>>()
        };
        assert_eq!(texts(Filter::default()), ["one", "two", "three"]);
        assert_eq!(
            texts(Filter {
                limit: Some(2),
                ..Default::default()
            }),
            ["two", "three"]
        );
        assert_eq!(
            texts(Filter {
                kinds: vec![Kind::Fact],
                ..Default::default()
            }),
            ["one", "three"]
        );
        assert_eq!(
            texts(Filter {
                since: Some(at("2026-09-02T00:00:00Z")),
                ..Default::default()
            }),
            ["two", "three"]
        );
        assert_eq!(s.branches().unwrap()[0].active, 3);
        assert!(s.generation().unwrap().is_some());
    }

    #[test]
    fn find_by_cid_is_per_branch() {
        let c = claim(Kind::Fact, "x", None, "2026-09-01T00:00:00Z");
        let s = store_with(std::slice::from_ref(&c));
        assert!(
            s.find_by_cid(&c.cid, &BranchRef::default_branch())
                .unwrap()
                .is_some()
        );
        assert!(
            s.find_by_cid(&c.cid, &BranchRef::new("other").unwrap())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn schema_mismatch_wipes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ctx.db");
        {
            let mut s = SqliteStore::open(&path).unwrap();
            s.ingest(
                "x",
                &recs(&[claim(Kind::Fact, "a", None, "2026-09-01T00:00:00Z")]),
                1,
            )
            .unwrap();
            s.conn.pragma_update(None, "user_version", 999).unwrap();
        }
        let s = SqliteStore::open(&path).unwrap();
        assert_eq!(s.stats().unwrap().claims, 0);
        assert_eq!(s.cursor("x").unwrap(), 0);
    }
}
