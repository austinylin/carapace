use thiserror::Error;

#[derive(Error, Debug)]
pub enum AgentError {
    #[error("SSH connection lost: {0}")]
    SSHConnectionLost(String),

    #[error("SSH connection refused: {0}")]
    SSHConnectionRefused(String),

    #[error("Failed to reconnect after {attempts} attempts")]
    ReconnectionFailed { attempts: u32 },

    #[error("Request timeout: {0}")]
    RequestTimeout(String),

    #[error("Request ID not found: {0}")]
    RequestNotFound(String),

    #[error("Invalid message received")]
    InvalidMessage,

    #[error("Socket bind failed: {0}")]
    SocketBindFailed(String),

    #[error("Config error: {0}")]
    ConfigError(String),

    #[error("IO error: {0}")]
    IOError(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, AgentError>;
