use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("claim text is empty after normalisation")]
    EmptyText,
    #[error("unknown {what} `{value}` (expected one of: {expected})")]
    UnknownVariant {
        what: &'static str,
        value: String,
        expected: &'static str,
    },
    #[error("invalid branch name `{0}`: use lowercase letters, digits, '-', '_' and '/'")]
    InvalidBranch(String),
}
