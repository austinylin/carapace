use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::{AgentError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// TCP server connection details
    pub server: TcpServerConfig,

    /// Unix socket for CLI handlers
    pub cli_socket: String,

    /// HTTP proxy ports
    pub http: HttpConfig,

    /// Logging configuration
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TcpServerConfig {
    /// Server hostname (e.g., austin-ubuntu-desktop.orca-puffin.ts.net)
    pub host: String,

    /// Server TCP port
    pub port: u16,

    /// Reconnection attempts
    #[serde(default = "default_reconnect_attempts")]
    pub reconnect_attempts: u32,

    /// Reconnection backoff in ms
    #[serde(default = "default_reconnect_backoff_ms")]
    pub reconnect_backoff_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpConfig {
    /// HTTP proxy listen port
    pub port: u16,

    /// Listen address (default: 127.0.0.1)
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,

    /// Request timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,

    /// Max request body size in bytes
    #[serde(default = "default_max_body_size")]
    pub max_body_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level (trace, debug, info, warn, error)
    #[serde(default = "default_log_level")]
    pub level: String,

    /// Log to file
    pub log_file: Option<String>,

    /// JSON structured logging
    #[serde(default)]
    pub json: bool,
}

impl AgentConfig {
    /// Load config from file
    pub fn from_file(path: &str) -> Result<Self> {
        let path = Path::new(path);
        if !path.exists() {
            return Err(AgentError::ConfigError(format!(
                "Config file not found: {}",
                path.display()
            )));
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| AgentError::ConfigError(format!("Failed to read config: {}", e)))?;

        serde_yaml::from_str(&content)
            .map_err(|e| AgentError::ConfigError(format!("Invalid YAML: {}", e)))
    }

    /// Load config from environment or defaults
    pub fn from_env() -> Self {
        AgentConfig {
            server: TcpServerConfig {
                host: std::env::var("CARAPACE_SERVER_HOST")
                    .unwrap_or_else(|_| "localhost".to_string()),
                port: std::env::var("CARAPACE_SERVER_PORT")
                    .ok()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(8765),
                reconnect_attempts: 5,
                reconnect_backoff_ms: 100,
            },
            cli_socket: std::env::var("CARAPACE_CLI_SOCKET")
                .unwrap_or_else(|_| "/tmp/carapace-agent.sock".to_string()),
            http: HttpConfig {
                port: std::env::var("CARAPACE_HTTP_PORT")
                    .ok()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(8080),
                listen_addr: "127.0.0.1".to_string(),
                timeout_secs: 30,
                max_body_size: 100 * 1024 * 1024, // 100MB
            },
            logging: LoggingConfig {
                level: std::env::var("CARAPACE_LOG_LEVEL").unwrap_or_else(|_| "info".to_string()),
                log_file: std::env::var("CARAPACE_LOG_FILE").ok(),
                json: std::env::var("CARAPACE_LOG_JSON")
                    .ok()
                    .map(|v| v == "true")
                    .unwrap_or(true),
            },
        }
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

// Defaults
pub fn default_timeout() -> u64 {
    30
}

pub fn default_reconnect_attempts() -> u32 {
    5
}

pub fn default_reconnect_backoff_ms() -> u64 {
    100
}

fn default_listen_addr() -> String {
    "127.0.0.1".to_string()
}

fn default_max_body_size() -> usize {
    100 * 1024 * 1024 // 100MB
}

fn default_log_level() -> String {
    "info".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_from_env() {
        let cfg = AgentConfig::from_env();
        assert_eq!(cfg.server.port, 8765);
        assert_eq!(cfg.http.port, 8080);
    }

    #[test]
    fn test_config_defaults() {
        let cfg = AgentConfig::default();
        assert!(!cfg.server.host.is_empty());
        assert!(!cfg.cli_socket.is_empty());
    }
}
