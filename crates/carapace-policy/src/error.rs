use thiserror::Error;

#[derive(Error, Debug)]
pub enum PolicyError {
    #[error("Invalid glob pattern: {0}")]
    InvalidPattern(String),

    #[error("Invalid regex: {0}")]
    RegexError(String),

    #[error("Config error: {0}")]
    ConfigError(String),

    #[error("Policy violation: {0}")]
    Violation(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML error: {0}")]
    YamlError(String),

    #[error("Tool not configured: {0}")]
    UnknownTool(String),
}
