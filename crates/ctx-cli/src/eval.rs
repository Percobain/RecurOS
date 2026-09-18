//! `ctx eval` (spec §15.2): the harness that makes the packer's weights
//! falsifiable.
//!
//! A fixture file lists questions about a real project with the key facts a
//! good answer must contain. For each budget, eval compiles a pack and scores
//! it in one of two modes:
//!
//! * `--model M`: ask a local Ollama model the question with the pack as
//!   context and check the answer for the expected facts.
//! * no model: check the pack itself for the facts (retrieval recall). Free,
//!   instant, and an upper bound on what any reader could answer.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use ctx_app::{App, PackOpts};
use ctx_pack::Projection;
use serde::Deserialize;

use crate::http;

#[derive(Debug, Deserialize)]
pub struct Fixtures {
    /// Branch used for questions that don't name their own.
    #[serde(default)]
    pub branch: Option<String>,
    pub questions: Vec<Question>,
}

#[derive(Debug, Deserialize)]
pub struct Question {
    pub q: String,
    /// Substrings (case-insensitive) a correct answer must contain.
    pub expect: Vec<String>,
    #[serde(default)]
    pub branch: Option<String>,
}

pub const EXAMPLE: &str = r#"# ctx eval fixtures: questions about your project and the facts a good
# answer must mention (case-insensitive substrings).
branch: myproject/research
questions:
  - q: Why don't we settle with covenants?
    expect: [too slow, federation peg]
  - q: Which database backs the index?
    expect: [sqlite]
"#;

pub struct Options<'a> {
    pub budgets: &'a [u32],
    pub model: Option<&'a str>,
    pub ollama: &'a str,
    pub projection: Projection,
}

fn score(text: &str, expect: &[String]) -> (usize, Vec<String>) {
    let lower = text.to_lowercase();
    let missing: Vec<String> = expect
        .iter()
        .filter(|e| !lower.contains(&e.to_lowercase()))
        .cloned()
        .collect();
    (expect.len() - missing.len(), missing)
}

fn ask_ollama(base: &str, model: &str, prompt: &str) -> Result<String> {
    let body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
        "options": {"temperature": 0}
    });
    let url = format!("{}/api/generate", base.trim_end_matches('/'));
    let (status, text) = http::request(
        "POST",
        &url,
        Some(&body.to_string()),
        Duration::from_secs(300),
    )
    .with_context(|| format!("calling Ollama at {base} (is `ollama serve` running?)"))?;
    if status != 200 {
        bail!("Ollama returned HTTP {status}: {}", text.trim());
    }
    let v: serde_json::Value = serde_json::from_str(&text).context("parsing Ollama response")?;
    Ok(v["response"].as_str().unwrap_or_default().to_owned())
}

pub fn run(app: &App, fixtures_path: &Path, opts: &Options) -> Result<()> {
    let text = std::fs::read_to_string(fixtures_path).with_context(|| {
        format!(
            "reading {} (create it; example:\n\n{EXAMPLE})",
            fixtures_path.display()
        )
    })?;
    let fixtures: Fixtures = serde_yaml::from_str(&text).context("parsing eval fixtures")?;
    if fixtures.questions.is_empty() {
        bail!("no questions in {}", fixtures_path.display());
    }
    let mode = match opts.model {
        Some(m) => format!("answers from {m}"),
        None => "facts present in the pack (no model; pass --model to ask one)".into(),
    };
    println!(
        "ctx eval: {} questions, scoring {mode}\n",
        fixtures.questions.len()
    );

    for &budget in opts.budgets {
        let (mut got, mut total, mut tokens) = (0usize, 0usize, 0u64);
        println!("budget {budget}:");
        for question in &fixtures.questions {
            let branch = question.branch.clone().or_else(|| fixtures.branch.clone());
            let pack = app.pack(&PackOpts {
                branch,
                task: Some(question.q.clone()),
                budget: Some(budget),
                projection: Some(opts.projection),
            })?;
            tokens += u64::from(pack.tokens);
            let answer = match opts.model {
                Some(model) => ask_ollama(
                    opts.ollama,
                    model,
                    &format!(
                        "{}\n\nUsing only the context above, answer briefly.\nQuestion: {}",
                        pack.markdown, question.q
                    ),
                )?,
                None => pack.markdown.clone(),
            };
            let (hit, missing) = score(&answer, &question.expect);
            got += hit;
            total += question.expect.len();
            let mark = if missing.is_empty() { "✓" } else { "✗" };
            print!("  {mark} {}/{}  {}", hit, question.expect.len(), question.q);
            if !missing.is_empty() {
                print!("   (missing: {})", missing.join(", "));
            }
            println!();
        }
        let pct = if total == 0 {
            0.0
        } else {
            100.0 * got as f64 / total as f64
        };
        println!(
            "  score {got}/{total} = {pct:.0}%   avg pack {} tokens\n",
            tokens / fixtures.questions.len() as u64
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_fixture_parses_and_scores() {
        let f: Fixtures = serde_yaml::from_str(EXAMPLE).unwrap();
        assert_eq!(f.questions.len(), 2);
        let (hit, missing) = score("We use SQLite.", &f.questions[1].expect);
        assert_eq!((hit, missing.len()), (1, 0));
    }
}
