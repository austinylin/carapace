use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConfig {
    pub tools: HashMap<String, ToolPolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolPolicy {
    #[serde(rename = "cli")]
    Cli(CliPolicy),
    #[serde(rename = "http")]
    Http(HttpPolicy),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliPolicy {
    pub binary: String,

    #[serde(default)]
    pub argv_allow_patterns: Vec<String>,

    #[serde(default)]
    pub argv_deny_patterns: Vec<String>,

    #[serde(default)]
    pub env_inject: HashMap<String, String>,

    #[serde(default)]
    pub cwd_allowed: Option<Vec<String>>,

    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,

    #[serde(default)]
    pub audit: AuditConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpPolicy {
    pub upstream: String,

    #[serde(default)]
    pub jsonrpc_allow_methods: Vec<String>,

    #[serde(default)]
    pub jsonrpc_deny_methods: Vec<String>,

    #[serde(default)]
    pub rate_limit: Option<RateLimit>,

    #[serde(default)]
    pub timeout_secs: Option<u64>,

    #[serde(default)]
    pub audit: AuditConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RateLimit {
    pub max_requests: u32,
    pub window_secs: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditConfig {
    #[serde(default = "default_audit_enabled")]
    pub enabled: bool,

    #[serde(default)]
    pub log_argv: bool,

    #[serde(default)]
    pub log_body: bool,

    #[serde(default)]
    pub redact_patterns: Vec<String>,
}

fn default_timeout() -> u64 {
    30
}

fn default_audit_enabled() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_policy_yaml_parse() {
        let yaml = r#"
tools:
  gh:
    type: cli
    binary: /usr/bin/gh
    argv_allow_patterns:
      - "pr list*"
      - "issue view *"
    argv_deny_patterns:
      - "* --token *"
    env_inject:
      GH_TOKEN: "{{ vault.gh_token }}"
    timeout_secs: 30
    audit:
      enabled: true
      log_argv: true
"#;

        let config: PolicyConfig = serde_yaml::from_str(yaml).expect("parse failed");
        assert!(config.tools.contains_key("gh"));
    }

    #[test]
    fn test_http_policy_yaml_parse() {
        let yaml = r#"
tools:
  signal-cli:
    type: http
    upstream: "http://127.0.0.1:18080"
    jsonrpc_allow_methods:
      - send
      - receive
    rate_limit:
      max_requests: 100
      window_secs: 60
    audit:
      enabled: true
      log_body: false
"#;

        let config: PolicyConfig = serde_yaml::from_str(yaml).expect("parse failed");
        assert!(config.tools.contains_key("signal-cli"));
    }

    #[test]
    fn test_missing_required_fields() {
        let yaml = r#"
tools:
  incomplete:
    type: cli
"#;

        let result: Result<PolicyConfig, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err(), "Should fail with missing required fields");
    }

    #[test]
    fn test_invalid_tool_type() {
        let yaml = r#"
tools:
  invalid:
    type: unknown
"#;

        let result: Result<PolicyConfig, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err(), "Should fail with invalid tool type");
    }

    #[test]
    fn test_malformed_yaml() {
        let yaml = r#"
tools:
  bad yaml: [unclosed
"#;

        let result: Result<PolicyConfig, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err(), "Should fail to parse malformed YAML");
    }

    #[test]
    fn test_empty_config() {
        let yaml = r#"
tools: {}
"#;

        let config: PolicyConfig = serde_yaml::from_str(yaml).expect("parse failed");
        assert_eq!(config.tools.len(), 0);
    }

    #[test]
    fn test_rate_limit_edge_cases() {
        let yaml = r#"
tools:
  test:
    type: http
    upstream: "http://localhost:8000"
    rate_limit:
      max_requests: 0
      window_secs: 1
"#;

        let config: PolicyConfig = serde_yaml::from_str(yaml).expect("parse failed");
        if let ToolPolicy::Http(http_policy) = config.tools.get("test").unwrap() {
            assert_eq!(http_policy.rate_limit.as_ref().unwrap().max_requests, 0);
        }
    }

    #[test]
    fn test_circular_env_var_reference() {
        // YAML itself doesn't prevent circular references - that's handled at runtime
        let yaml = r#"
tools:
  test:
    type: cli
    binary: /usr/bin/test
    env_inject:
      VAR_A: "{{ VAR_B }}"
      VAR_B: "{{ VAR_A }}"
"#;

        let config: PolicyConfig = serde_yaml::from_str(yaml).expect("parse failed");
        // Config should parse fine; the issue is runtime resolution
        assert!(config.tools.contains_key("test"));
    }
}
