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

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use chrono::{DateTime, SecondsFormat, Utc};
use ctx_core::{
    BranchRef, BranchSummary, Claim, CounterUpdate, Doc, Filter, Record, Stats, Status,
    StatusChange, Store,
};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use thiserror::Error;
use ulid::Ulid;

/// Bump whenever the schema changes; a mismatch triggers a full rebuild.
const SCHEMA_VERSION: i64 = 4;

const SCHEMA: &str = "
CREATE TABLE claims (
  id TEXT PRIMARY KEY, cid TEXT NOT NULL, kind TEXT NOT NULL, branch TEXT NOT NULL,
  status TEXT NOT NULL, base_status TEXT NOT NULL,
  tokens INTEGER NOT NULL, confidence TEXT NOT NULL, src TEXT NOT NULL,
  t_valid TEXT NOT NULL, t_tx TEXT NOT NULL, last_used TEXT,
  helpful INTEGER NOT NULL DEFAULT 0, harmful INTEGER NOT NULL DEFAULT 0,
  text TEXT NOT NULL, why TEXT, entities TEXT NOT NULL, refs TEXT NOT NULL DEFAULT '',
  body TEXT NOT NULL
);
CREATE INDEX idx_branch_status ON claims(branch, status);
CREATE INDEX idx_kind ON claims(kind);
CREATE INDEX idx_cid_branch ON claims(cid, branch);

CREATE VIRTUAL TABLE claims_fts USING fts5(
  text, why, entities, refs,
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

-- Long-form documents (specs). Every version is kept; the newest id per
-- (branch, name) is current.
CREATE TABLE docs (
  id TEXT PRIMARY KEY, branch TEXT NOT NULL, name TEXT NOT NULL, title TEXT NOT NULL,
  body TEXT NOT NULL, cid TEXT NOT NULL, src TEXT NOT NULL, t_tx TEXT NOT NULL
);
CREATE INDEX idx_docs_branch_name ON docs(branch, name, id);

CREATE TABLE cursors (shard TEXT PRIMARY KEY, offset INTEGER NOT NULL);
";

const DROP: &str = "
DROP TABLE IF EXISTS claims_fts;
DROP TABLE IF EXISTS claims;
DROP TABLE IF EXISTS edges;
DROP TABLE IF EXISTS status_changes;
DROP TABLE IF EXISTS counters;
DROP TABLE IF EXISTS docs;
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
    /// One FTS5 pass, returning claim ids best-first.
    fn fts_ids(
        &self,
        query: &str,
        filter: &Filter,
        limit: usize,
    ) -> Result<Vec<String>, SqliteError> {
        let mut params = vec![Value::Text(query.to_owned())];
        let where_ = filter_sql(filter, &mut params);
        // bm25 column weights: `entities` are curated tags and `refs` are the
        // files a claim is about, so a hit there is a deliberate pointer and
        // counts most; `why` is supporting prose and counts least. Ties break
        // towards the newer claim.
        let sql = format!(
            "SELECT c.id FROM claims_fts f JOIN claims c ON c.rowid = f.rowid
             WHERE claims_fts MATCH ?{where_}
             ORDER BY bm25(claims_fts, 1.0, 0.5, 2.0, 1.5), c.id DESC LIMIT {limit}"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |r| {
                r.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

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

/// Returns true if the claim was new. Its status and counters are
/// reconciled once per batch by `reconcile_late_records`.
fn insert_claim(tx: &Transaction, c: &Claim) -> Result<bool, SqliteError> {
    let entities = c.entities.join(" ");
    // Refs are file paths and symbols: the most specific handle a claim has.
    // The unicode61 tokeniser splits `crates/ctx-pack/src/lib.rs` on its
    // punctuation, so indexing the joined list is enough for "pack" or
    // "lib.rs" to find the claims attached to that file.
    let refs = c.refs.join(" ");
    let inserted = tx
        .prepare_cached(
            "INSERT OR IGNORE INTO claims
         (id, cid, kind, branch, status, base_status, tokens, confidence, src, t_valid, t_tx,
          last_used, text, why, entities, refs, body)
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
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
            refs,
            serde_json::to_string(c)?,
        ])?;
    if inserted == 0 {
        return Ok(false); // already indexed (duplicate line)
    }
    tx.prepare_cached(
        "INSERT INTO claims_fts(rowid, text, why, entities, refs) VALUES (?,?,?,?,?)",
    )?
    .execute(params![
        tx.last_insert_rowid(),
        c.text,
        c.why,
        entities,
        refs
    ])?;
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
    Ok(true)
}

/// Status and counter records can arrive before the claim they refer to
/// (different shards are read in any order). Those claims were inserted
/// with their base status, so fix them up here. Checking the small set of
/// claims that have such records once per batch, instead of querying for
/// every inserted claim, keeps a 10k-claim reindex several times faster.
fn reconcile_late_records(tx: &Transaction, inserted: &[String]) -> Result<(), SqliteError> {
    if inserted.is_empty() {
        return Ok(());
    }
    let ids_in = |table: &str| -> Result<HashSet<String>, SqliteError> {
        let mut stmt = tx.prepare_cached(&format!("SELECT DISTINCT claim FROM {table}"))?;
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<HashSet<_>, _>>()?;
        Ok(ids)
    };
    let with_status = ids_in("status_changes")?;
    let with_counters = ids_in("counters")?;
    for id in inserted {
        if with_status.contains(id) {
            refresh_status(tx, id)?;
        }
        if with_counters.contains(id) {
            refresh_counters(tx, id)?;
        }
    }
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

fn insert_doc(tx: &Transaction, d: &Doc) -> Result<(), SqliteError> {
    tx.prepare_cached(
        "INSERT OR IGNORE INTO docs(id, branch, name, title, body, cid, src, t_tx)
         VALUES (?,?,?,?,?,?,?,?)",
    )?
    .execute(params![
        d.id.to_string(),
        d.branch.as_str(),
        d.name,
        d.title,
        d.body,
        d.cid,
        d.src,
        ts(d.t_tx),
    ])?;
    Ok(())
}

/// A `docs` row: id, branch, name, title, body, cid, src, t_tx.
type DocRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
);

fn row_to_doc(r: &rusqlite::Row) -> rusqlite::Result<DocRow> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
    ))
}

fn to_doc((id, branch, name, title, body, cid, src, t): DocRow) -> Option<Doc> {
    Some(Doc {
        id: Ulid::from_string(&id).ok()?,
        branch: BranchRef::new(&branch).ok()?,
        name,
        title,
        body,
        cid,
        src,
        t_tx: DateTime::parse_from_rfc3339(&t).ok()?.with_timezone(&Utc),
    })
}

const DOC_COLS: &str = "id, branch, name, title, body, cid, src, t_tx";

/// Documents are hidden by a status record that targets their id, the same
/// way claims are archived: nothing is ever erased from the log.
const DOC_VISIBLE: &str =
    "id NOT IN (SELECT claim FROM status_changes WHERE to_status = 'archived')";

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

/// Function words. They appear in nearly every claim and nearly every task
/// description, so OR-ing them in makes bm25 rank on noise, and AND-ing them
/// in throws away good matches over a word that carries no meaning.
const QUERY_STOPWORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "for", "with", "that", "this", "these", "those", "from",
    "into", "is", "are", "was", "were", "be", "been", "being", "it", "its", "of", "on", "in", "at",
    "to", "we", "us", "our", "you", "your", "i", "my", "me", "they", "their", "do", "does", "did",
    "how", "what", "why", "when", "where", "which", "who", "can", "could", "should", "would",
    "will", "shall", "may", "might", "must", "not", "no", "have", "has", "had", "use", "using",
    "used", "about", "than", "then", "there", "here", "if", "so", "as", "by", "just", "please",
];

/// A long task description is mostly scene-setting; past a couple of dozen
/// words the extra terms only broaden the OR pass.
const MAX_TERMS: usize = 24;

/// The two lexical passes over one piece of free text.
struct FtsQuery {
    /// Every meaningful word, OR-ed: broad, ranked by bm25.
    any: String,
    /// The same words AND-ed: only claims containing all of them. `None` for
    /// a single word, where it would be the same query as `any`.
    all: Option<String>,
}

/// Split a camelCase or PascalCase word into its parts, lowercased.
/// `syncNow` gives `["sync", "now"]`; a word with no internal capital gives
/// nothing. FTS5's unicode61 tokeniser keeps `syncNow` as a single token, so
/// without this a claim that says "sync" is invisible to that query.
fn camel_parts(word: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in word.chars() {
        if ch.is_uppercase() && !cur.is_empty() {
            parts.push(std::mem::take(&mut cur));
        }
        cur.extend(ch.to_lowercase());
    }
    parts.push(cur);
    if parts.len() < 2 {
        return Vec::new();
    }
    parts.retain(|p| p.chars().count() > 1);
    parts
}

/// Turn free text into the two safe FTS5 queries. Every word becomes a quoted
/// prefix term, so user input can never be parsed as FTS5 syntax, and so
/// "sync" still matches "syncing".
fn fts_query(query: &str) -> Option<FtsQuery> {
    let mut words: Vec<String> = Vec::new();
    let push = |w: String, words: &mut Vec<String>| {
        if !words.contains(&w) {
            words.push(w);
        }
    };
    for raw in query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
    {
        for part in camel_parts(raw) {
            push(part, &mut words);
        }
        push(raw.to_lowercase(), &mut words);
    }
    if words.is_empty() {
        return None;
    }
    let meaningful: Vec<String> = words
        .iter()
        .filter(|w| w.chars().count() > 1 && !QUERY_STOPWORDS.contains(&w.as_str()))
        .cloned()
        .collect();
    // A query made entirely of function words is still a query: search for
    // what was asked rather than for nothing at all.
    let kept = if meaningful.is_empty() {
        words
    } else {
        meaningful
    };
    let quoted: Vec<String> = kept
        .iter()
        .take(MAX_TERMS)
        .map(|t| format!("\"{t}\"*"))
        .collect();
    Some(FtsQuery {
        any: quoted.join(" OR "),
        all: (quoted.len() > 1).then(|| quoted.join(" AND ")),
    })
}

/// Reciprocal rank fusion: score each id by `1 / (k + rank)` summed over the
/// lists it appears in, highest first. Ranks rather than scores are fused, so
/// the passes need no common scale and neither can starve the other. `k = 60`
/// is the constant from the original paper, and the one the packer uses. Ties
/// break towards the newer claim.
fn fuse(lists: &[Vec<String>], limit: usize) -> Vec<String> {
    let mut score: HashMap<&str, f64> = HashMap::new();
    let mut seen: Vec<&str> = Vec::new();
    for list in lists {
        for (rank, id) in list.iter().enumerate() {
            let entry = score.entry(id.as_str()).or_insert_with(|| {
                seen.push(id.as_str());
                0.0
            });
            *entry += 1.0 / (60.0 + rank as f64 + 1.0);
        }
    }
    seen.sort_by(|a, b| {
        score[b]
            .partial_cmp(&score[a])
            .unwrap_or(Ordering::Equal)
            .then_with(|| b.cmp(a))
    });
    seen.into_iter().take(limit).map(str::to_owned).collect()
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
        let mut inserted: Vec<String> = Vec::new();
        for r in records {
            match r {
                Record::Claim(c) => {
                    if insert_claim(&tx, c)? {
                        inserted.push(c.id.to_string());
                    }
                }
                Record::Status(s) => insert_status(&tx, s)?,
                Record::Counter(u) => insert_counter(&tx, u)?,
                Record::Doc(d) => insert_doc(&tx, d)?,
            }
        }
        reconcile_late_records(&tx, &inserted)?;
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

    /// Hybrid lexical search: a precise pass (every meaningful word present)
    /// and a broad one (any of them), fused by reciprocal rank. The precise
    /// pass alone returns nothing the moment one word is missing; the broad
    /// pass alone lets a claim sharing one common word with many others
    /// outrank the claim that answers the question. Fusing them keeps the
    /// recall of the second and the ordering of the first.
    fn search(&self, query: &str, filter: &Filter) -> Result<Vec<Claim>, SqliteError> {
        let Some(q) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let limit = filter.limit.unwrap_or(50);
        // Fuse over more than was asked for: a claim ranked 60th in one pass
        // and 3rd in the other belongs near the top of the fused list.
        let deep = limit.saturating_mul(4).max(50);
        let mut lists: Vec<Vec<String>> = Vec::new();
        if let Some(all) = &q.all {
            lists.push(self.fts_ids(all, filter, deep)?);
        }
        lists.push(self.fts_ids(&q.any, filter, deep)?);
        let ids = fuse(&lists, limit);
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let rank: HashMap<&str, usize> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        let holes = vec!["?"; ids.len()].join(",");
        let params: Vec<Value> = ids.iter().map(|id| Value::Text(id.clone())).collect();
        let mut claims = self.query_claims(
            &format!("SELECT body, status FROM claims WHERE id IN ({holes})"),
            &params,
        )?;
        claims.sort_by_key(|c| rank[c.id.to_string().as_str()]);
        Ok(claims)
    }

    fn neighbors(
        &self,
        seeds: &[Ulid],
        filter: &Filter,
        limit: usize,
    ) -> Result<Vec<Ulid>, SqliteError> {
        if seeds.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let seed_ids: HashSet<String> = seeds.iter().map(ToString::to_string).collect();
        let holes = vec!["?"; seeds.len()].join(",");
        let mut params: Vec<Value> = seeds
            .iter()
            .map(|s| Value::Text(s.to_string()))
            .collect::<Vec<_>>();
        params.extend(params.clone());
        let mut out: Vec<String> = Vec::new();

        // Supersession first: the history of a hit is the strongest kind of
        // neighbour, and the one a lexical pass is least likely to find,
        // because a replacement is usually phrased differently.
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT dst FROM edges WHERE kind = 'supersedes' AND src IN ({holes})
             UNION
             SELECT src FROM edges WHERE kind = 'supersedes' AND dst IN ({holes})"
        ))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |r| {
                r.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for id in rows {
            if !seed_ids.contains(&id) && !out.contains(&id) {
                out.push(id);
            }
        }

        // Then the tags and files the seeds point at. One FTS pass restricted
        // to those two columns, so prose that happens to mention a filename
        // does not count as pointing at it.
        let mut tags: Vec<String> = Vec::new();
        let seed_params: Vec<Value> = seeds.iter().map(|s| Value::Text(s.to_string())).collect();
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT entities, refs FROM claims WHERE id IN ({holes})"
        ))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(seed_params.iter()), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (entities, refs) in rows {
            for tag in entities.split_whitespace().chain(refs.split_whitespace()) {
                let tag = tag.to_lowercase();
                if tag.chars().count() > 1 && !tags.contains(&tag) {
                    tags.push(tag);
                }
            }
        }
        if !tags.is_empty() && out.len() < limit {
            let terms: Vec<String> = tags
                .iter()
                .take(MAX_TERMS)
                .map(|t| format!("\"{t}\""))
                .collect();
            let query = format!("{{entities refs}} : ({})", terms.join(" OR "));
            for id in self.fts_ids(&query, filter, limit * 2)? {
                if !seed_ids.contains(&id) && !out.contains(&id) {
                    out.push(id);
                }
            }
        }
        out.truncate(limit);
        Ok(out
            .iter()
            .filter_map(|id| Ulid::from_string(id).ok())
            .collect())
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
               UNION ALL SELECT max(rec_id) FROM counters
               UNION ALL SELECT max(id) FROM docs)",
            [],
            |r| r.get(0),
        )?;
        Ok(max.and_then(|s| Ulid::from_string(&s).ok()))
    }

    fn docs(&self, branch: Option<&BranchRef>) -> Result<Vec<Doc>, SqliteError> {
        let sql = format!(
            "SELECT {DOC_COLS} FROM docs d
             WHERE id = (SELECT max(id) FROM docs WHERE branch = d.branch AND name = d.name
                         AND {DOC_VISIBLE})
               AND (?1 IS NULL OR branch = ?1)
             ORDER BY branch, name"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt
            .query_map([branch.map(|b| b.as_str().to_owned())], row_to_doc)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().filter_map(to_doc).collect())
    }

    fn doc(&self, branch: &BranchRef, name: &str) -> Result<Option<Doc>, SqliteError> {
        let sql = format!(
            "SELECT {DOC_COLS} FROM docs WHERE branch = ?1 AND name = ?2 AND {DOC_VISIBLE}
             ORDER BY id DESC LIMIT 1"
        );
        Ok(self
            .conn
            .query_row(&sql, params![branch.as_str(), name], row_to_doc)
            .optional()?
            .and_then(to_doc))
    }

    fn doc_versions(&self, branch: &BranchRef) -> Result<Vec<Doc>, SqliteError> {
        let sql =
            format!("SELECT {DOC_COLS} FROM docs WHERE branch = ?1 AND {DOC_VISIBLE} ORDER BY id");
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt
            .query_map([branch.as_str()], row_to_doc)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().filter_map(to_doc).collect())
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

    fn tagged(kind: Kind, text: &str, refs: &[&str], entities: &[&str], t: &str) -> Claim {
        let mut d = ClaimDraft::new(kind, text);
        d.refs = refs.iter().map(|s| (*s).to_owned()).collect();
        d.entities = entities.iter().map(|s| (*s).to_owned()).collect();
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

    #[test]
    fn search_finds_claims_by_the_files_they_are_about() {
        let s = store_with(&[
            tagged(
                Kind::Decision,
                "Budgets are enforced with a compact fallback",
                &["crates/ctx-pack/src/lib.rs"],
                &["packer"],
                "2026-09-01T00:00:00Z",
            ),
            claim(
                Kind::Fact,
                "The registry is sqlite",
                None,
                "2026-09-02T00:00:00Z",
            ),
        ]);
        // The path is in `refs`, not in the text: without refs in the index
        // there is nothing here to match.
        for q in ["ctx-pack", "lib.rs", "crates/ctx-pack/src/lib.rs", "packer"] {
            let hits = s.search(q, &Filter::default()).unwrap();
            assert_eq!(hits.len(), 1, "{q} found {} claims", hits.len());
            assert!(hits[0].text.starts_with("Budgets"), "{q}");
        }
    }

    #[test]
    fn search_ranks_a_claim_with_every_word_above_one_with_a_single_word() {
        let s = store_with(&[
            claim(
                Kind::Fact,
                "The daemon listens on loopback only",
                None,
                "2026-09-01T00:00:00Z",
            ),
            claim(
                Kind::Decision,
                "Pull with rebase so the daemon never rewrites a shard",
                None,
                "2026-09-02T00:00:00Z",
            ),
            claim(
                Kind::Fact,
                "Rebase keeps the log append only",
                None,
                "2026-09-03T00:00:00Z",
            ),
        ]);
        // Only the middle claim has both words. The OR pass alone would let
        // either single-word claim outrank it on term frequency.
        let hits = s.search("daemon rebase", &Filter::default()).unwrap();
        assert!(hits[0].text.starts_with("Pull with rebase"), "{hits:#?}");
        assert_eq!(hits.len(), 3, "the other two are still reachable");
    }

    #[test]
    fn search_splits_camel_case_and_ignores_function_words() {
        let s = store_with(&[
            claim(
                Kind::Fact,
                "Sync is debounced by thirty seconds",
                None,
                "2026-09-01T00:00:00Z",
            ),
            claim(
                Kind::Fact,
                "The index is a disposable cache",
                None,
                "2026-09-02T00:00:00Z",
            ),
        ]);
        let hits = s.search("syncNow", &Filter::default()).unwrap();
        assert_eq!(hits.len(), 1, "camelCase should split into sync + now");
        // "is a the" are all function words; dropping them must not turn the
        // query into a match on every claim that contains "the".
        assert!(
            s.search("is a the", &Filter::default()).unwrap().len() <= 2,
            "a query of only function words falls back to searching them"
        );
        assert_eq!(
            s.search("what is the sync about", &Filter::default())
                .unwrap()[0]
                .text
                .starts_with("Sync"),
            true
        );
    }

    #[test]
    fn neighbors_follow_supersession_and_shared_refs() {
        let old = tagged(
            Kind::Decision,
            "Pull with fast forward only",
            &["crates/ctx-git/src/sync.rs"],
            &[],
            "2026-09-01T00:00:00Z",
        );
        let mut d = ClaimDraft::new(Kind::Decision, "Pull with rebase");
        d.supersedes = Some(old.id);
        let now = at("2026-09-02T00:00:00Z");
        let new =
            Claim::from_draft(d, Ulid::from_parts(now.timestamp_millis() as u64, 1), now).unwrap();
        let sibling = tagged(
            Kind::Constraint,
            "A shard has exactly one writer",
            &["crates/ctx-git/src/sync.rs"],
            &[],
            "2026-09-03T00:00:00Z",
        );
        let unrelated = claim(
            Kind::Fact,
            "The registry is sqlite",
            None,
            "2026-09-04T00:00:00Z",
        );
        let s = store_with(&[old.clone(), new.clone(), sibling.clone(), unrelated.clone()]);

        // From the replacement: the claim it replaced, even though the two
        // share no searchable word beyond "pull with".
        let n = s.neighbors(&[new.id], &Filter::default(), 10).unwrap();
        assert!(n.contains(&old.id), "{n:?}");
        // From the old claim: its replacement, and the constraint on the same
        // file, but not a claim that shares nothing.
        let n = s.neighbors(&[old.id], &Filter::default(), 10).unwrap();
        assert!(n.contains(&new.id) && n.contains(&sibling.id), "{n:?}");
        assert!(!n.contains(&unrelated.id), "{n:?}");
        assert!(!n.contains(&old.id), "a seed is not its own neighbour");
        assert!(s.neighbors(&[], &Filter::default(), 10).unwrap().is_empty());
    }

    #[test]
    fn fts_query_splits_and_filters() {
        assert!(fts_query("").is_none());
        assert!(fts_query("***").is_none());
        let q = fts_query("the daemon").unwrap();
        assert_eq!(q.any, "\"daemon\"*");
        assert!(q.all.is_none(), "one meaningful word needs no AND pass");
        let q = fts_query("readFile and writeFile").unwrap();
        // read, file, readfile, write, writefile: "and" is dropped, "file" once.
        assert_eq!(q.all.unwrap().matches(" AND ").count(), 4);
        assert_eq!(camel_parts("syncNow"), vec!["sync", "now"]);
        assert!(camel_parts("sync").is_empty());
    }
}
