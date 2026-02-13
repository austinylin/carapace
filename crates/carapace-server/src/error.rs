use thiserror::Error;

#[derive(Error, Debug)]
pub enum ServerError {
    #[error("Policy violation: {0}")]
    PolicyViolation(String),

    #[error("Shell injection detected: {0}")]
    ShellInjectionDetected(String),

    #[error("Binary path invalid: {0}")]
    InvalidBinaryPath(String),

    #[error("Process execution failed with code {code}: {message}")]
    ProcessExecutionFailed { code: i32, message: String },

    #[error("Request timeout")]
    RequestTimeout,

    #[error("Rate limit exceeded for tool {tool}")]
    RateLimitExceeded { tool: String },

    #[error("Tool not found in policy: {0}")]
    ToolNotFound(String),

    #[error("Invalid tool type for request: {0}")]
    InvalidToolType(String),

    #[error("Config error: {0}")]
    ConfigError(String),

    #[error("IO error: {0}")]
    IOError(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, ServerError>;
