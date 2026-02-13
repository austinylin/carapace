use serde::Serialize;
use chrono::Utc;

/// Structured audit log entry
#[derive(Debug, Clone, Serialize)]
pub struct AuditLogEntry {
    pub timestamp: String,
    pub request_id: String,
    pub tool: String,
    pub action_type: String, // "cli" or "http"
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

/// Audit logging system with structured JSON output
pub struct AuditLogger {
    enabled: bool,
    log_argv: bool,
    #[allow(dead_code)]
    log_body: bool,
    redact_patterns: Vec<String>,
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
        }
    }

    pub fn with_config(enabled: bool, log_argv: bool, log_body: bool) -> Self {
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
            policy_result: if allowed { "allow".to_string() } else { "deny".to_string() },
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
            policy_result: if allowed { "allow".to_string() } else { "deny".to_string() },
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
    pub fn log_http_response(
        &self,
        request_id: &str,
        status: u16,
        latency_ms: u64,
    ) {
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

    /// Emit log entry as structured JSON to stdout/logs
    fn emit_log_entry(&self, entry: &AuditLogEntry) {
        if let Ok(json) = serde_json::to_string(entry) {
            tracing::info!("AUDIT: {}", json);
        }
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
        let logger = AuditLogger::with_config(false, false, false);

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
