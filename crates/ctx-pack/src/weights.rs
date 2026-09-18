//! Every tunable constant of the packer, in one serialisable place (spec §8.2).
//!
//! These values are informed guesses. `ctx eval` exists to tune them against
//! real questions; override per store in `refs/weights.yaml`.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use ctx_core::{Claim, Confidence, Kind, Status};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Weights {
    /// Relevance term weight.
    pub alpha: f64,
    /// Facility-location coverage weight (diversity across entities/kinds).
    pub beta: f64,
    /// Redundancy penalty weight.
    pub gamma: f64,
    /// Recency half-life. Constraints never decay.
    pub half_life_days: f64,
    /// τ(kind)
    pub kind: BTreeMap<Kind, f64>,
    /// κ(confidence)
    pub confidence: BTreeMap<Confidence, f64>,
    /// σ(status). Missing statuses weigh 0 and are never packed.
    pub status: BTreeMap<Status, f64>,
    /// Run the `why` pass only if more than this fraction of budget is left.
    pub why_pass_threshold: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Weights {
            alpha: 1.0,
            beta: 0.6,
            gamma: 0.4,
            half_life_days: 180.0,
            kind: BTreeMap::from([
                (Kind::Constraint, 1.0),
                (Kind::Decision, 1.0),
                (Kind::Rejected, 0.9),
                (Kind::Fact, 0.8),
                (Kind::Question, 0.7),
                (Kind::Claim, 0.6),
            ]),
            confidence: BTreeMap::from([
                (Confidence::High, 1.0),
                (Confidence::Medium, 0.8),
                (Confidence::Low, 0.5),
            ]),
            // Superseded is deliberately non-zero: "we used to think X, then
            // changed to Y" is half the value of a handoff.
            status: BTreeMap::from([
                (Status::Active, 1.0),
                (Status::Superseded, 0.15),
                (Status::Archived, 0.0),
            ]),
            why_pass_threshold: 0.15,
        }
    }
}

impl Weights {
    /// Load overrides from YAML; missing file means defaults.
    pub fn load(path: &Path) -> Result<Weights, String> {
        match std::fs::read_to_string(path) {
            Ok(text) if text.trim().is_empty() => Ok(Weights::default()),
            Ok(text) => serde_yaml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Weights::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// w(c) = τ(kind) · κ(confidence) · ρ(recency) · η(usefulness) · σ(status)
    pub fn weight(&self, c: &Claim, now: DateTime<Utc>) -> f64 {
        let tau = self.kind.get(&c.kind).copied().unwrap_or(0.5);
        let kappa = self.confidence.get(&c.confidence).copied().unwrap_or(0.8);
        let sigma = self.status.get(&c.status).copied().unwrap_or(0.0);
        let rho = if c.kind == Kind::Constraint || self.half_life_days <= 0.0 {
            1.0 // protocol limits don't decay
        } else {
            let age_days = (now - c.t_valid).num_seconds().max(0) as f64 / 86_400.0;
            (-(std::f64::consts::LN_2 / self.half_life_days) * age_days).exp()
        };
        let helpful = c.helpful_total() as f64;
        let harmful = c.harmful_total() as f64;
        let eta = (1.0 + helpful) / (1.0 + helpful + 2.0 * harmful);
        tau * kappa * sigma * rho * eta
    }
}
