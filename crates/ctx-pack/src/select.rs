//! Budgeted selection: cost-aware lazy greedy over a submodular objective
//! (spec §8.2–8.3).
//!
//! ```text
//! F(S) = α · Σ_{c∈S} rel(c,q) · w(c)            relevance
//!      + β · Σ_{e∈E} max_{c∈S} cov(c,e)         facility-location coverage
//!      − γ · Σ_{c,c'∈S} sim(c,c')               redundancy
//! ```
//!
//! `E` is the set of entity tags, `cov(c,e) = w(c)` when `c` is tagged `e`,
//! and `sim` is Jaccard similarity of content-word sets.
//!
//! Everything here is pure: no I/O, no clock, no randomness.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use ulid::Ulid;

/// Per-candidate inputs, all precomputed before the selection loop so the
/// loop never tokenises or allocates strings.
#[derive(Debug, Clone)]
pub struct Candidate<'a> {
    pub id: Ulid,
    /// w(c) from [`crate::Weights::weight`].
    pub weight: f64,
    /// rel(c, q) in [0, 1]; 1.0 when there is no task.
    pub relevance: f64,
    /// Tokens to render the claim without its `why`.
    pub cost: u32,
    /// Extra tokens to also render its `why` (0 if none).
    pub why_cost: u32,
    /// Hashed entity tags, for coverage.
    pub features: Vec<u64>,
    /// Source text for the similarity word set, which is computed lazily:
    /// most candidates are only ever scored against an empty selection, where
    /// similarity is irrelevant, so hashing every claim's words up front was
    /// the largest cost of a compile.
    pub text: &'a str,
    pub why: Option<&'a str>,
    pub pinned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhyMode {
    /// Pass one packs text only; pass two spends leftover budget on `why`.
    Leftover,
    /// Include `why` from the start (browser surfaces, handoffs).
    Always,
    Never,
}

#[derive(Debug, Clone, Copy)]
pub struct Params {
    pub alpha: f64,
    pub beta: f64,
    pub gamma: f64,
    pub budget: u32,
    pub why_mode: WhyMode,
    pub why_pass_threshold: f64,
    /// Keep adding claims even when their marginal gain is not positive.
    /// Used for unlimited budgets (handoffs): redundancy exists to spend a
    /// tight window well, not to hide history such as a superseded claim,
    /// which is inherently similar to the claim that replaced it.
    pub fill: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    /// Index into the candidate slice.
    pub index: usize,
    pub with_why: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub picked: Vec<Picked>,
    pub tokens: u32,
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a: tiny, fast, and fixed forever (unlike `DefaultHasher`, whose
/// algorithm std may change), which keeps packs byte-identical across
/// toolchains.
pub fn hash_str(s: &str) -> u64 {
    s.bytes().fold(FNV_OFFSET, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(FNV_PRIME)
    })
}

const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "are", "was", "were", "not", "but", "use",
    "uses", "used", "into", "its", "our", "their", "has", "have", "had", "can", "will", "should",
    "must", "all", "any", "only", "than", "then", "when", "which", "what", "why", "how", "because",
];

fn stopword_hashes() -> &'static [u64] {
    static HASHES: std::sync::OnceLock<Vec<u64>> = std::sync::OnceLock::new();
    HASHES.get_or_init(|| STOPWORDS.iter().map(|w| hash_str(w)).collect())
}

/// Content-word set used for redundancy: lowercase alphanumeric words of 3+
/// chars minus stopwords, hashed, sorted, deduplicated. Hashes case-folded
/// chars directly so no per-word string is allocated — this runs for every
/// candidate on every compile.
pub fn word_set(text: &str) -> Vec<u64> {
    let stop = stopword_hashes();
    let mut v: Vec<u64> = Vec::new();
    let mut h = FNV_OFFSET;
    let mut len = 0usize;
    let mut buf = [0u8; 4];
    let flush = |h: &mut u64, len: &mut usize, v: &mut Vec<u64>| {
        if *len >= 3 && !stop.contains(h) {
            v.push(*h);
        }
        *h = FNV_OFFSET;
        *len = 0;
    };
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            h = (h ^ u64::from(ch.to_ascii_lowercase() as u8)).wrapping_mul(FNV_PRIME);
            len += 1;
        } else if !ch.is_ascii() && ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                for b in lc.encode_utf8(&mut buf).bytes() {
                    h = (h ^ u64::from(b)).wrapping_mul(FNV_PRIME);
                }
            }
            len += 1;
        } else {
            flush(&mut h, &mut len, &mut v);
        }
    }
    flush(&mut h, &mut len, &mut v);
    v.sort_unstable();
    v.dedup();
    v
}

/// Jaccard similarity of two sorted, deduplicated sets.
pub fn jaccard(a: &[u64], b: &[u64]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let (mut i, mut j, mut inter) = (0, 0, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            Ordering::Less => i += 1,
            Ordering::Greater => j += 1,
            Ordering::Equal => {
                inter += 1;
                i += 1;
                j += 1;
            }
        }
    }
    inter as f64 / (a.len() + b.len() - inter) as f64
}

/// Mutable state of a partial selection S, kept so marginal gains are cheap.
struct State<'a> {
    cands: &'a [Candidate<'a>],
    words: Vec<std::cell::OnceCell<Vec<u64>>>,
    params: Params,
    /// max_{c∈S} cov(c, e) per feature.
    best_cov: HashMap<u64, f64>,
    chosen: Vec<usize>,
    /// Per candidate: Σ sim(c, s) over `chosen[..n]`, and that `n`. Lazy
    /// greedy re-evaluates near-tied candidates many times; caching the sum
    /// means each re-evaluation only compares against claims chosen since
    /// the last one. Summation order is unchanged, so results are identical.
    red: Vec<(f64, usize)>,
}

impl<'a> State<'a> {
    fn new(cands: &'a [Candidate<'a>], params: Params) -> Self {
        State {
            cands,
            words: vec![std::cell::OnceCell::new(); cands.len()],
            params,
            best_cov: HashMap::new(),
            chosen: Vec::new(),
            red: vec![(0.0, 0); cands.len()],
        }
    }

    /// `cov(c, e)`, the value of `c` as the representative of feature `e`.
    ///
    /// Relevance belongs here and not only in the relevance term. Coverage is
    /// a sum over a claim's tags, so a claim carrying four of them can score
    /// four times what the relevance term can ever award, and a pack asked to
    /// focus on a task came back looking almost exactly like one that had not
    /// been asked anything. Covering the tags of claims nobody asked about is
    /// not worth more than answering the question.
    fn cov(c: &Candidate) -> f64 {
        c.weight * c.relevance
    }

    /// Δ(c | S) = F(S ∪ {c}) − F(S).
    fn gain(&mut self, i: usize) -> f64 {
        let c = &self.cands[i];
        let rel = self.params.alpha * c.relevance * c.weight;
        let own = Self::cov(c);
        let cov: f64 = c
            .features
            .iter()
            .map(|f| (own - self.best_cov.get(f).copied().unwrap_or(0.0)).max(0.0))
            .sum();
        let (mut red, seen) = self.red[i];
        if seen < self.chosen.len() {
            let wi = self.words(i);
            for &j in &self.chosen[seen..] {
                red += jaccard(wi, self.words(j));
            }
        }
        self.red[i] = (red, self.chosen.len());
        rel + self.params.beta * cov - self.params.gamma * red
    }

    fn words(&self, i: usize) -> &[u64] {
        self.words[i].get_or_init(|| {
            let c = &self.cands[i];
            let mut w = word_set(c.text);
            if let Some(why) = c.why {
                w.extend(word_set(why));
                w.sort_unstable();
                w.dedup();
            }
            w
        })
    }

    fn add(&mut self, i: usize) {
        let c = &self.cands[i];
        let own = Self::cov(c);
        for f in &c.features {
            let e = self.best_cov.entry(*f).or_insert(0.0);
            if own > *e {
                *e = own;
            }
        }
        self.chosen.push(i);
    }
}

/// Marginal gain of candidate `i` given the already-chosen set `s`.
/// Exposed for tests of the submodularity property.
pub fn marginal_gain(cands: &[Candidate<'_>], params: Params, s: &[usize], i: usize) -> f64 {
    let mut st = State::new(cands, params);
    for &j in s {
        st.add(j);
    }
    st.gain(i)
}

/// Heap entry: an upper bound on a candidate's gain-per-token, tagged with the
/// round (|S|) it was computed in. Ordered by ratio, then by *smaller* id, so
/// ties resolve identically on every run.
struct Entry {
    ratio: f64,
    id: Ulid,
    index: usize,
    round: usize,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Entry {}
impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.ratio
            .total_cmp(&other.ratio)
            .then_with(|| other.id.cmp(&self.id))
    }
}

fn cost_of(c: &Candidate<'_>, mode: WhyMode) -> u32 {
    match mode {
        WhyMode::Always => c.cost + c.why_cost,
        _ => c.cost,
    }
}

/// Choose a subset maximising F under the token budget.
///
/// Lazy greedy (Minoux): because F is submodular, a candidate's gain can only
/// shrink as S grows, so a stale upper bound that still tops the heap after
/// recomputation is the true best choice. This turns O(n²) re-evaluation into
/// roughly O(n log n).
pub fn select(cands: &[Candidate<'_>], params: Params) -> Selection {
    let mut st = State::new(cands, params);
    let mut left = params.budget;
    let mut picked: Vec<Picked> = Vec::new();
    let with_why = params.why_mode == WhyMode::Always;

    // Pinned claims are always included and charged to the budget.
    let mut pinned: Vec<usize> = (0..cands.len()).filter(|&i| cands[i].pinned).collect();
    pinned.sort_by_key(|&i| cands[i].id);
    for i in pinned {
        let cost = cost_of(&cands[i], params.why_mode);
        left = left.saturating_sub(cost);
        st.add(i);
        picked.push(Picked {
            index: i,
            with_why: with_why && cands[i].why_cost > 0,
        });
    }

    let mut heap: BinaryHeap<Entry> = (0..cands.len())
        .filter(|&i| !cands[i].pinned)
        .map(|i| Entry {
            ratio: st.gain(i) / f64::from(cost_of(&cands[i], params.why_mode).max(1)),
            id: cands[i].id,
            index: i,
            round: 0,
        })
        .collect();

    while left > 0 {
        let Some(top) = heap.pop() else { break };
        let c = &cands[top.index];
        let cost = cost_of(c, params.why_mode);
        if cost > left {
            continue; // can never fit again: budget only shrinks
        }
        let round = st.chosen.len();
        if top.round != round {
            let ratio = st.gain(top.index) / f64::from(cost.max(1));
            heap.push(Entry {
                ratio,
                round,
                ..top
            });
            continue;
        }
        if top.ratio <= 0.0 && !params.fill {
            break; // every remaining bound is ≤ this one: nothing helps
        }
        st.add(top.index);
        left -= cost;
        picked.push(Picked {
            index: top.index,
            with_why: with_why && c.why_cost > 0,
        });
    }

    // Second pass: spend leftover budget on `why`, highest-weight first.
    if params.why_mode == WhyMode::Leftover
        && f64::from(left) > params.why_pass_threshold * f64::from(params.budget)
    {
        let mut order: Vec<usize> = (0..picked.len()).collect();
        order.sort_by(|&a, &b| {
            let (ca, cb) = (&cands[picked[a].index], &cands[picked[b].index]);
            cb.weight
                .total_cmp(&ca.weight)
                .then_with(|| ca.id.cmp(&cb.id))
        });
        for k in order {
            let c = &cands[picked[k].index];
            if c.why_cost > 0 && c.why_cost <= left {
                picked[k].with_why = true;
                left -= c.why_cost;
            }
        }
    }

    Selection {
        tokens: params.budget.saturating_sub(left),
        picked,
    }
}

/// Reciprocal Rank Fusion: `score(c) = Σ_rankers 1 / (60 + rank)`. Needs no
/// calibration between incomparable rankers (BM25 now, vectors later).
pub fn rrf(rankings: &[Vec<Ulid>]) -> HashMap<Ulid, f64> {
    let mut out: HashMap<Ulid, f64> = HashMap::new();
    for ranking in rankings {
        for (rank, id) in ranking.iter().enumerate() {
            *out.entry(*id).or_insert(0.0) += 1.0 / (60.0 + rank as f64 + 1.0);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_set_folds_case_and_drops_stopwords() {
        assert_eq!(
            word_set("The Settlement uses PEG"),
            word_set("settlement peg")
        );
        assert_eq!(word_set("Ünïcode ünïcode"), vec![hash_str("ünïcode")]);
        assert!(word_set("a an to").is_empty());
    }
}
