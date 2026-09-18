//! The `ctx-claims` fence protocol (docs/protocol.md §4): how browser chat
//! surfaces hand claims back. Depends only on markdown code fences, so it
//! survives chat UI redesigns.

use anyhow::{Context, Result, bail};
use ctx_core::Kind;
use serde::Deserialize;

/// One claim as supplied by an external surface (model output, extension,
/// MCP call) before it is validated and turned into a draft.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ClaimInput {
    pub kind: String,
    pub text: String,
    #[serde(default)]
    pub why: Option<String>,
    #[serde(default)]
    pub refs: Vec<String>,
    #[serde(default)]
    pub entities: Vec<String>,
}

impl ClaimInput {
    pub fn kind(&self) -> Result<Kind> {
        Ok(self.kind.parse::<Kind>()?)
    }
}

/// Extract claims from text containing a ```ctx-claims fence, or a bare JSON
/// array. If several fences exist, the last one wins (the model's final
/// answer). Invalid JSON is an error, never a silent partial save.
pub fn parse(text: &str) -> Result<Vec<ClaimInput>> {
    let body = last_fence(text).unwrap_or_else(|| text.trim());
    if body.is_empty() {
        bail!("no ctx-claims block found");
    }
    let claims: Vec<ClaimInput> = serde_json::from_str(body)
        .context("the ctx-claims block is not a JSON array of {kind, text, why, refs}")?;
    for c in &claims {
        c.kind()?;
    }
    Ok(claims)
}

fn last_fence(text: &str) -> Option<&str> {
    let mut found = None;
    let mut rest = text;
    let mut offset = 0;
    while let Some(start) = rest.find("```ctx-claims") {
        let after = &rest[start + "```ctx-claims".len()..];
        let body_start = after.find('\n').map(|i| i + 1)?;
        let body = &after[body_start..];
        let end = body.find("```")?;
        found = Some((offset + start, body[..end].trim()));
        let consumed = start + "```ctx-claims".len() + body_start + end + 3;
        offset += consumed;
        rest = &rest[consumed..];
    }
    found.map(|(_, b)| b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fenced_block() {
        let text = "Sure!\n\n```ctx-claims\n[{\"kind\":\"decision\",\"text\":\"Use a peg\",\"why\":\"fast\"}]\n```\nDone.";
        let c = parse(text).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].why.as_deref(), Some("fast"));
    }

    #[test]
    fn last_block_wins_and_bare_arrays_work() {
        let text = "```ctx-claims\n[{\"kind\":\"fact\",\"text\":\"a\"}]\n```\n```ctx-claims\n[{\"kind\":\"fact\",\"text\":\"b\"}]\n```";
        assert_eq!(parse(text).unwrap()[0].text, "b");
        assert_eq!(
            parse("[{\"kind\":\"question\",\"text\":\"q\"}]")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse("").is_err());
        assert!(parse("```ctx-claims\nnot json\n```").is_err());
        assert!(parse("[{\"kind\":\"opinion\",\"text\":\"x\"}]").is_err());
    }
}
