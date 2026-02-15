use std::io;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ProtocolError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("JSON serialization error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Framing error: {0}")]
    FramingError(String),

    #[error("Frame too large: {0} bytes")]
    FrameTooLarge(u32),

    #[error("Invalid message format: {0}")]
    InvalidMessage(String),
}

#[derive(Error, Debug)]
pub enum FrameError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Invalid data: {0}")]
    InvalidData(String),
}
