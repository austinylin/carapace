use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::{ServerError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Policy configuration path
    pub policy_path: String,

    /// Audit logging configuration
    pub audit: AuditConfig,

    /// Rate limiting configuration
    pub rate_limiting: RateLimitingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    /// Enable audit logging
    #[serde(default)]
    pub enabled: bool,

    /// Audit log file path
    pub log_file: Option<String>,

    /// Max audit log size before rotation (bytes)
    #[serde(default = "default_max_log_size")]
    pub max_size_bytes: u64,

    /// Number of rotated logs to keep
    #[serde(default = "default_keep_logs")]
    pub keep_logs: u32,

    /// Log argv details
    #[serde(default)]
    pub log_argv: bool,

    /// Sensitive patterns to redact
    #[serde(default = "default_redact_patterns")]
    pub redact_patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitingConfig {
    /// Enable rate limiting
    #[serde(default)]
    pub enabled: bool,

    /// Default requests per window
    #[serde(default = "default_requests_per_window")]
    pub default_requests_per_window: u32,

    /// Default window size in seconds
    #[serde(default = "default_window_secs")]
    pub default_window_secs: u64,

    /// Per-tool limits (tool name -> (requests, window_secs))
    #[serde(default)]
    pub per_tool_limits: Vec<(String, u32, u64)>,
}

impl ServerConfig {
    /// Load config from file
    pub fn from_file(path: &str) -> Result<Self> {
        let path = Path::new(path);
        if !path.exists() {
            return Err(ServerError::ConfigError(format!(
                "Config file not found: {}",
                path.display()
            )));
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| ServerError::ConfigError(format!("Failed to read config: {}", e)))?;

        serde_yaml::from_str(&content)
            .map_err(|e| ServerError::ConfigError(format!("Invalid YAML: {}", e)))
    }

    /// Load config from environment or defaults
    pub fn from_env() -> Self {
        ServerConfig {
            policy_path: std::env::var("CARAPACE_POLICY_PATH")
                .unwrap_or_else(|_| "/etc/carapace/policies".to_string()),
            audit: AuditConfig {
                enabled: std::env::var("CARAPACE_AUDIT_ENABLED")
                    .ok()
                    .map(|v| v == "true")
                    .unwrap_or(true),
                log_file: std::env::var("CARAPACE_AUDIT_LOG").ok(),
                max_size_bytes: 100 * 1024 * 1024, // 100MB
                keep_logs: 10,
                log_argv: true,
                redact_patterns: default_redact_patterns(),
            },
            rate_limiting: RateLimitingConfig {
                enabled: std::env::var("CARAPACE_RATE_LIMIT_ENABLED")
                    .ok()
                    .map(|v| v == "true")
                    .unwrap_or(false),
                default_requests_per_window: 1000,
                default_window_secs: 60,
                per_tool_limits: vec![],
            },
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

// Defaults
fn default_max_log_size() -> u64 {
    100 * 1024 * 1024 // 100MB
}

fn default_keep_logs() -> u32 {
    10
}

fn default_requests_per_window() -> u32 {
    1000
}

fn default_window_secs() -> u64 {
    60
}

fn default_redact_patterns() -> Vec<String> {
    vec![
        "--token".to_string(),
        "--password".to_string(),
        "--secret".to_string(),
        "Authorization".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_from_env() {
        let cfg = ServerConfig::from_env();
        assert!(cfg.audit.enabled);
        assert!(!cfg.policy_path.is_empty());
    }

    #[test]
    fn test_config_defaults() {
        let cfg = ServerConfig::default();
        assert_eq!(cfg.rate_limiting.default_window_secs, 60);
        assert_eq!(cfg.audit.keep_logs, 10);
    }
}
