//! Token estimation.
//!
//! The spec allows `chars/4` as a fallback to `tiktoken-rs`. We use the
//! estimate for now: the target models (Claude, GPT, Qwen) don't share a
//! tokenizer, so an exact count for one of them is still only an estimate for
//! the others, and this keeps `ctx-core` dependency-light. Budgets are
//! enforced against this same estimate, so packing is self-consistent.

/// Estimated tokens for `s`: one per four chars, rounded up.
pub fn estimate(s: &str) -> u32 {
    let chars = s.chars().count();
    u32::try_from(chars.div_ceil(4)).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    #[test]
    fn rounds_up() {
        assert_eq!(super::estimate(""), 0);
        assert_eq!(super::estimate("a"), 1);
        assert_eq!(super::estimate("abcd"), 1);
        assert_eq!(super::estimate("abcde"), 2);
        assert_eq!(super::estimate("éééé"), 1);
    }
}
