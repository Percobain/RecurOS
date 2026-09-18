//! The ContextOS compiler (spec §8): compile a `(branch, task, budget)` triple
//! into a projection.
//!
//! This crate is pure. `compile` takes the candidate pool, optional retrieval
//! scores, and weights, and returns markdown. No I/O, no clock (recency is
//! measured against the newest claim in the pool), no randomness — identical
//! inputs produce byte-identical output.

pub mod render;
pub mod select;
pub mod weights;

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use ctx_core::{Claim, merkle, tokens};
use ulid::Ulid;

pub use render::{DocRef, IndexEntry, Projection, UNLIMITED, render_index};
pub use select::{Candidate, Params, WhyMode, rrf};
pub use weights::Weights;

/// Everything `compile` needs. Retrieval and pool resolution happen in the
/// caller; this struct is the whole contract.
pub struct Request<'a> {
    /// Full branch name being compiled, e.g. `sovereign/code`.
    pub branch: &'a str,
    /// Candidate claims (the resolved pool), with effective statuses.
    pub pool: &'a [Claim],
    /// RRF scores from retrieval for the task, if a task was given. Claims
    /// absent from the map are excluded (unless pinned). An empty map is
    /// treated as "no task" so an unmatched task still yields a useful pack.
    pub relevance: Option<&'a HashMap<Ulid, f64>>,
    pub pinned: &'a [Ulid],
    pub budget: u32,
    pub projection: Projection,
    pub weights: &'a Weights,
    pub generation: Option<Ulid>,
    pub version: &'a str,
    /// Documents visible from the branch, listed (not inlined) in the pack.
    pub docs: &'a [DocRef],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pack {
    pub markdown: String,
    /// Selected claim ids, in rendered order is not guaranteed; sorted by id.
    pub claims: Vec<Ulid>,
    /// Estimated tokens of the full output, footer included.
    pub tokens: u32,
    /// Merkle root over the selected claims' CIDs.
    pub root: String,
}

/// Coverage features: the claim's entity tags (spec §8.2's `E`).
///
/// Kind is deliberately *not* a feature: kind balance already comes from
/// τ(kind), and a feature shared by a sixth of all claims invalidates that
/// many lazy-greedy bounds on every pick, which tripled compile time.
fn features(c: &Claim) -> Vec<u64> {
    c.entities.iter().map(|e| select::hash_str(e)).collect()
}

/// Compile a pack. Guarantees `tokens <= budget` (by the chars/4 estimate)
/// unless the pinned claims alone exceed it.
pub fn compile(req: &Request) -> Pack {
    let now: DateTime<Utc> = req
        .pool
        .iter()
        .map(|c| c.t_tx)
        .max()
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH);

    let relevance = req.relevance.filter(|m| !m.is_empty());
    let max_rel = relevance
        .map(|m| m.values().copied().fold(0.0_f64, f64::max))
        .unwrap_or(1.0)
        .max(f64::MIN_POSITIVE);
    let pinned: HashSet<Ulid> = req.pinned.iter().copied().collect();

    // Deterministic candidate order regardless of how the pool was fetched.
    let mut pool: Vec<&Claim> = req.pool.iter().collect();
    pool.sort_by_key(|c| c.id);
    pool.dedup_by_key(|c| c.id);

    let mut claims: Vec<&Claim> = Vec::new();
    let mut cands: Vec<Candidate> = Vec::new();
    for c in pool {
        let weight = req.weights.weight(c, now);
        let is_pinned = pinned.contains(&c.id);
        if weight <= 0.0 && !is_pinned {
            continue;
        }
        let relevance = match relevance {
            Some(m) => match m.get(&c.id) {
                Some(score) => score / max_rel,
                None if is_pinned => 1.0,
                None => continue,
            },
            None => 1.0,
        };
        let (base_chars, why_chars) = render::bullet_chars(c, req.branch);
        let base = base_chars.div_ceil(4) as u32;
        let full = (base_chars + why_chars).div_ceil(4) as u32;
        cands.push(Candidate {
            id: c.id,
            weight: weight.max(0.0),
            relevance,
            cost: base,
            why_cost: full.saturating_sub(base),
            features: features(c),
            text: &c.text,
            why: c.why.as_deref(),
            pinned: is_pinned,
        });
        claims.push(c);
    }

    let mut meta = render::Meta {
        branch: req.branch,
        version: req.version,
        generation: req.generation.map(|g| g.to_string()),
        budget: req.budget,
        docs: req.docs,
        compact: false,
    };

    // Fixed overhead (header, rules, section headings, footer) is charged up
    // front; section headings are approximated generously and the loop below
    // makes the guarantee exact.
    let empty_body = render::render_body(req.projection, &[], &meta);
    let overhead = tokens::estimate(&empty_body) + 40 + 8 * 7;
    let params = Params {
        alpha: req.weights.alpha,
        beta: req.weights.beta,
        gamma: req.weights.gamma,
        budget: req.budget.saturating_sub(overhead),
        why_mode: req.projection.why_mode(),
        why_pass_threshold: req.weights.why_pass_threshold,
        fill: req.budget >= UNLIMITED,
    };
    let selection = select::select(&cands, params);

    let mut items: Vec<render::Item> = selection
        .picked
        .iter()
        .map(|p| render::Item {
            claim: claims[p.index],
            with_why: p.with_why,
            weight: cands[p.index].weight,
        })
        .collect();

    loop {
        let (markdown, total, root) = finish(req.projection, &items, &meta);
        if total <= req.budget || req.budget >= UNLIMITED {
            let mut ids: Vec<Ulid> = items.iter().map(|i| i.claim.id).collect();
            ids.sort();
            return Pack {
                markdown,
                claims: ids,
                tokens: total,
                root,
            };
        }
        // Over budget (heading estimate was short): shed the least valuable
        // detail first — a `why`, then a whole claim. Pinned claims stay.
        let victim = items
            .iter()
            .enumerate()
            .filter(|(_, i)| !pinned.contains(&i.claim.id))
            .min_by(|(_, a), (_, b)| {
                a.weight
                    .total_cmp(&b.weight)
                    .then_with(|| b.claim.id.cmp(&a.claim.id))
            })
            .map(|(k, _)| k);
        match victim {
            Some(k) if items[k].with_why => items[k].with_why = false,
            Some(k) => {
                items.remove(k);
            }
            None if !meta.compact => {
                // Even the fixed text (header, rules, protocol) doesn't fit:
                // fall back to the bare form and start again with every
                // selected claim.
                meta.compact = true;
                items = selection
                    .picked
                    .iter()
                    .map(|p| render::Item {
                        claim: claims[p.index],
                        with_why: false,
                        weight: cands[p.index].weight,
                    })
                    .collect();
            }
            None => {
                let mut ids: Vec<Ulid> = items.iter().map(|i| i.claim.id).collect();
                ids.sort();
                return Pack {
                    markdown,
                    claims: ids,
                    tokens: total,
                    root,
                };
            }
        }
    }
}

fn finish(p: Projection, items: &[render::Item], meta: &render::Meta) -> (String, u32, String) {
    let body = render::render_body(p, items, meta);
    let cids: Vec<&str> = items.iter().map(|i| i.claim.cid.as_str()).collect();
    let root = merkle::root(&cids);
    // The footer contains its own total, which changes the total: iterate
    // until the number printed equals the estimate of the final string
    // (converges in one or two steps since only a few digits can change).
    let mut total =
        tokens::estimate(&body) + tokens::estimate(&render::footer(meta, items.len(), 0, &root));
    let mut markdown = String::new();
    for _ in 0..4 {
        markdown = body.clone() + &render::footer(meta, items.len(), total, &root);
        let actual = tokens::estimate(&markdown);
        if actual == total {
            break;
        }
        total = actual;
    }
    (markdown, total, root)
}
