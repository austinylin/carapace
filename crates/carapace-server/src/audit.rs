use chrono::Utc;
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Structured audit log entry
#[derive(Debug, Clone, Serialize)]
pub struct AuditLogEntry {
    pub timestamp: String,
    pub request_id: String,
    pub tool: String,
    pub action_type: String,   // "cli" or "http"
    pub policy_result: String, // "allow" or "deny"
    pub reason: Option<String>,
    pub argv: Option<Vec<String>>,
    pub method: Option<String>,
    pub path: Option<String>,
    pub exit_code: Option<i32>,
    pub stdout_length: Option<usize>,
    pub stderr_length: Option<usize>,
    pub latency_ms: Option<u64>,
}

/// Audit logging system with structured JSON output and persistence
pub struct AuditLogger {
    enabled: bool,
    log_argv: bool,
    #[allow(dead_code)]
    log_body: bool,
    redact_patterns: Vec<String>,
    log_file: Option<String>,
    max_size_bytes: u64,
    keep_logs: u32,
    #[allow(dead_code)]
    current_size: Arc<Mutex<u64>>,
}

impl AuditLogger {
    pub fn new() -> Self {
        AuditLogger {
            enabled: true,
            log_argv: true,
            log_body: false,
            redact_patterns: vec![
                "--token".to_string(),
                "--password".to_string(),
                "--secret".to_string(),
                "Authorization".to_string(),
            ],
            log_file: None,
            max_size_bytes: 100 * 1024 * 1024, // 100MB
            keep_logs: 10,
            current_size: Arc::new(Mutex::new(0)),
        }
    }

    pub fn with_config(
        enabled: bool,
        log_argv: bool,
        log_body: bool,
        log_file: Option<String>,
        max_size_bytes: u64,
        keep_logs: u32,
    ) -> Self {
        AuditLogger {
            enabled,
            log_argv,
            log_body,
            redact_patterns: vec![
                "--token".to_string(),
                "--password".to_string(),
                "--secret".to_string(),
                "Authorization".to_string(),
            ],
            log_file,
            max_size_bytes,
            keep_logs,
            current_size: Arc::new(Mutex::new(0)),
        }
    }

    /// Log a CLI request
    pub fn log_cli_request(
        &self,
        request_id: &str,
        tool: &str,
        argv: &[String],
        allowed: bool,
        reason: Option<&str>,
    ) {
        if !self.enabled {
            return;
        }

        let redacted_argv = if allowed && self.log_argv {
            Some(self.redact_sensitive_args(argv))
        } else {
            None
        };

        let entry = AuditLogEntry {
            timestamp: Utc::now().to_rfc3339(),
            request_id: request_id.to_string(),
            tool: tool.to_string(),
            action_type: "cli".to_string(),
            policy_result: if allowed {
                "allow".to_string()
            } else {
                "deny".to_string()
            },
            reason: reason.map(|s| s.to_string()),
            argv: redacted_argv,
            method: None,
            path: None,
            exit_code: None,
            stdout_length: None,
            stderr_length: None,
            latency_ms: None,
        };

        self.emit_log_entry(&entry);
    }

    /// Log a CLI response
    pub fn log_cli_response(
        &self,
        request_id: &str,
        exit_code: i32,
        stdout_len: usize,
        stderr_len: usize,
        latency_ms: u64,
    ) {
        if !self.enabled {
            return;
        }

        let entry = AuditLogEntry {
            timestamp: Utc::now().to_rfc3339(),
            request_id: request_id.to_string(),
            tool: String::new(),
            action_type: "cli_response".to_string(),
            policy_result: String::new(),
            reason: None,
            argv: None,
            method: None,
            path: None,
            exit_code: Some(exit_code),
            stdout_length: Some(stdout_len),
            stderr_length: Some(stderr_len),
            latency_ms: Some(latency_ms),
        };

        self.emit_log_entry(&entry);
    }

    /// Log an HTTP request
    pub fn log_http_request(
        &self,
        request_id: &str,
        tool: &str,
        method: &str,
        path: &str,
        allowed: bool,
        reason: Option<&str>,
    ) {
        if !self.enabled {
            return;
        }

        let entry = AuditLogEntry {
            timestamp: Utc::now().to_rfc3339(),
            request_id: request_id.to_string(),
            tool: tool.to_string(),
            action_type: "http".to_string(),
            policy_result: if allowed {
                "allow".to_string()
            } else {
                "deny".to_string()
            },
            reason: reason.map(|s| s.to_string()),
            argv: None,
            method: Some(method.to_string()),
            path: Some(path.to_string()),
            exit_code: None,
            stdout_length: None,
            stderr_length: None,
            latency_ms: None,
        };

        self.emit_log_entry(&entry);
    }

    /// Log an HTTP response
    pub fn log_http_response(&self, request_id: &str, status: u16, latency_ms: u64) {
        if !self.enabled {
            return;
        }

        let entry = AuditLogEntry {
            timestamp: Utc::now().to_rfc3339(),
            request_id: request_id.to_string(),
            tool: String::new(),
            action_type: "http_response".to_string(),
            policy_result: format!("status_{}", status),
            reason: None,
            argv: None,
            method: None,
            path: None,
            exit_code: Some(status as i32),
            stdout_length: None,
            stderr_length: None,
            latency_ms: Some(latency_ms),
        };

        self.emit_log_entry(&entry);
    }

    /// Redact sensitive arguments (tokens, passwords, etc.)
    fn redact_sensitive_args(&self, argv: &[String]) -> Vec<String> {
        let mut result = Vec::new();
        let mut skip_next = false;

        for (i, arg) in argv.iter().enumerate() {
            if skip_next {
                result.push("[REDACTED]".to_string());
                skip_next = false;
                continue;
            }

            // Check if this is a sensitive flag
            let mut is_sensitive = false;
            for pattern in &self.redact_patterns {
                if arg.contains(pattern.as_str()) {
                    is_sensitive = true;
                    break;
                }
            }

            if is_sensitive {
                result.push(arg.clone());
                // Next arg is likely the value, mark for redaction
                if i + 1 < argv.len() && !argv[i + 1].starts_with('-') {
                    skip_next = true;
                }
            } else {
                result.push(arg.clone());
            }
        }

        result
    }

    /// Emit log entry as structured JSON to stdout/logs and optionally to file
    fn emit_log_entry(&self, entry: &AuditLogEntry) {
        if let Ok(json) = serde_json::to_string(entry) {
            // Always log to tracing
            tracing::info!("AUDIT: {}", json);

            // Also write to file if configured
            if let Some(log_file) = &self.log_file {
                let _ = self.write_to_file(&json, log_file);
            }
        }
    }

    /// Write log entry to file with rotation
    fn write_to_file(&self, json: &str, log_file: &str) -> std::io::Result<()> {
        let log_entry = format!("{}\n", json);
        let entry_size = log_entry.len() as u64;

        // Check if we need to rotate
        let current_size = std::fs::metadata(log_file)
            .ok()
            .map(|m| m.len())
            .unwrap_or(0);

        if current_size + entry_size > self.max_size_bytes {
            self.rotate_logs(log_file)?;
        }

        // Append to log file
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file)?;

        file.write_all(log_entry.as_bytes())?;
        Ok(())
    }

    /// Rotate logs when max size is exceeded
    fn rotate_logs(&self, log_file: &str) -> std::io::Result<()> {
        let path = Path::new(log_file);

        // Rotate existing logs: shift .1 → .2, .0 → .1, current → .0
        for i in (0..self.keep_logs).rev() {
            let old_name = if i == 0 {
                log_file.to_string()
            } else {
                format!("{}.{}", path.with_extension("").to_string_lossy(), i)
            };

            let new_name = format!("{}.{}", path.with_extension("").to_string_lossy(), i + 1);

            if Path::new(&old_name).exists() {
                if i + 1 < self.keep_logs {
                    fs::rename(&old_name, &new_name).ok();
                } else {
                    // Delete oldest log
                    fs::remove_file(&old_name).ok();
                }
            }
        }

        // Clear current log file
        fs::write(log_file, "")?;
        Ok(())
    }
}

impl Default for AuditLogger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_logger_creation() {
        let _logger = AuditLogger::new();
    }

    #[test]
    fn test_redact_token() {
        let logger = AuditLogger::new();
        let argv = vec![
            "gh".to_string(),
            "--token".to_string(),
            "ghp_secret123".to_string(),
            "pr".to_string(),
            "list".to_string(),
        ];

        let redacted = logger.redact_sensitive_args(&argv);
        assert_eq!(redacted[0], "gh");
        assert_eq!(redacted[1], "--token");
        assert_eq!(redacted[2], "[REDACTED]");
        assert_eq!(redacted[3], "pr");
        assert_eq!(redacted[4], "list");
    }

    #[test]
    fn test_no_redaction_when_disabled() {
        let logger = AuditLogger::with_config(false, false, false, None, 100 * 1024 * 1024, 10);

        // When logging is disabled, we don't log at all
        assert!(!logger.enabled);
    }

    #[test]
    fn test_audit_entry_serialization() {
        let entry = AuditLogEntry {
            timestamp: "2026-02-12T10:00:00Z".to_string(),
            request_id: "req-1".to_string(),
            tool: "gh".to_string(),
            action_type: "cli".to_string(),
            policy_result: "allow".to_string(),
            reason: None,
            argv: Some(vec!["pr".to_string(), "list".to_string()]),
            method: None,
            path: None,
            exit_code: None,
            stdout_length: None,
            stderr_length: None,
            latency_ms: None,
        };

        let json = serde_json::to_string(&entry).expect("serialization failed");
        assert!(json.contains("\"tool\":\"gh\""));
        assert!(json.contains("\"policy_result\":\"allow\""));
    }
}
