use carapace_policy::PolicyConfig;
/// Entry Point and Initialization Tests
///
/// Tests for server startup, configuration loading, signal handling, and
/// graceful shutdown. These test the critical initialization paths.
use carapace_server::{CliDispatcher, HttpDispatcher, Listener};
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use tempfile::NamedTempFile;

#[test]
fn test_policy_config_loading_from_file() {
    // Create a temporary policy file
    let mut file = NamedTempFile::new().expect("Failed to create temp file");
    let policy_yaml = r#"
tools:
  gh:
    type: cli
    binary: /usr/bin/gh
    argv_allow_patterns:
      - "pr list"
      - "issue *"
    argv_deny_patterns: []
    timeout_secs: 30
    audit:
      enabled: true
      log_argv: true
"#;
    file.write_all(policy_yaml.as_bytes())
        .expect("Failed to write policy");
    file.flush().expect("Failed to flush");

    // Load the policy
    let policy = PolicyConfig::from_file(file.path().to_str().expect("Path is not UTF-8"))
        .expect("Failed to load policy");

    // Verify it loaded correctly
    assert_eq!(policy.tools.len(), 1);
    assert!(policy.tools.contains_key("gh"));
}

#[test]
fn test_policy_config_missing_file() {
    // Try to load a non-existent file
    let result = PolicyConfig::from_file("/nonexistent/policy/file.yaml");

    assert!(result.is_err(), "Should error when file doesn't exist");
}

#[test]
fn test_policy_config_invalid_yaml() {
    // Create a temporary file with invalid YAML
    let mut file = NamedTempFile::new().expect("Failed to create temp file");
    let invalid_yaml = r#"
tools:
  gh:
    type: cli
    binary: /usr/bin/gh
    invalid indentation here
      - this breaks yaml
"#;
    file.write_all(invalid_yaml.as_bytes())
        .expect("Failed to write invalid YAML");
    file.flush().expect("Failed to flush");

    // Try to load it
    let result = PolicyConfig::from_file(file.path().to_str().expect("Path is not UTF-8"));

    assert!(result.is_err(), "Should error on invalid YAML");
}

#[test]
fn test_cli_dispatcher_creation_with_empty_policy() {
    let policy = PolicyConfig {
        tools: HashMap::new(),
    };

    let _dispatcher = CliDispatcher::with_policy(policy);

    // Should create successfully even with no tools
}

#[test]
fn test_http_dispatcher_creation_with_empty_policy() {
    let policy = PolicyConfig {
        tools: HashMap::new(),
    };

    let _dispatcher = HttpDispatcher::with_policy(policy);

    // Should create successfully even with no tools
}

#[test]
fn test_listener_creation() {
    let policy = PolicyConfig {
        tools: HashMap::new(),
    };

    let cli_dispatcher = Arc::new(CliDispatcher::with_policy(policy.clone()));
    let http_dispatcher = Arc::new(HttpDispatcher::with_policy(policy));

    let listener = Listener::new(cli_dispatcher, http_dispatcher);

    // Listener should be created
    let _ = listener;
}

#[test]
fn test_policy_config_partial_fields() {
    // Test that missing optional fields don't break loading
    let mut file = NamedTempFile::new().expect("Failed to create temp file");
    let minimal_yaml = r#"
tools:
  test:
    type: cli
    binary: /usr/bin/test
"#;
    file.write_all(minimal_yaml.as_bytes())
        .expect("Failed to write policy");
    file.flush().expect("Failed to flush");

    let result = PolicyConfig::from_file(file.path().to_str().expect("Path is not UTF-8"));

    // Should load successfully with defaults
    assert!(result.is_ok(), "Should load policy with minimal fields");
}

#[test]
fn test_policy_config_with_env_inject() {
    let mut file = NamedTempFile::new().expect("Failed to create temp file");
    let policy_yaml = r#"
tools:
  op:
    type: cli
    binary: /usr/bin/op
    argv_allow_patterns: ["*"]
    env_inject:
      OP_SERVICE_ACCOUNT_TOKEN: "secret-token-here"
      CUSTOM_VAR: "custom-value"
    timeout_secs: 30
"#;
    file.write_all(policy_yaml.as_bytes())
        .expect("Failed to write policy");
    file.flush().expect("Failed to flush");

    let policy = PolicyConfig::from_file(file.path().to_str().expect("Path is not UTF-8"))
        .expect("Failed to load policy");

    assert_eq!(policy.tools.len(), 1);
    assert!(policy.tools.contains_key("op"));
}

#[test]
fn test_policy_config_with_http_policy() {
    let mut file = NamedTempFile::new().expect("Failed to create temp file");
    let policy_yaml = r#"
tools:
  signal-cli:
    type: http
    upstream: "http://127.0.0.1:18080"
    jsonrpc_allow_methods:
      - send
      - receive
    jsonrpc_deny_methods: []
    timeout_secs: 30
    audit:
      enabled: true
"#;
    file.write_all(policy_yaml.as_bytes())
        .expect("Failed to write policy");
    file.flush().expect("Failed to flush");

    let policy = PolicyConfig::from_file(file.path().to_str().expect("Path is not UTF-8"))
        .expect("Failed to load policy");

    assert_eq!(policy.tools.len(), 1);
    assert!(policy.tools.contains_key("signal-cli"));
}

#[test]
fn test_policy_config_mixed_cli_and_http() {
    let mut file = NamedTempFile::new().expect("Failed to create temp file");
    let policy_yaml = r#"
tools:
  gh:
    type: cli
    binary: /usr/bin/gh
    argv_allow_patterns: ["*"]
    timeout_secs: 30
  signal-cli:
    type: http
    upstream: "http://127.0.0.1:18080"
    jsonrpc_allow_methods: ["send"]
    timeout_secs: 30
"#;
    file.write_all(policy_yaml.as_bytes())
        .expect("Failed to write policy");
    file.flush().expect("Failed to flush");

    let policy = PolicyConfig::from_file(file.path().to_str().expect("Path is not UTF-8"))
        .expect("Failed to load policy");

    assert_eq!(policy.tools.len(), 2);
    assert!(policy.tools.contains_key("gh"));
    assert!(policy.tools.contains_key("signal-cli"));
}

#[test]
fn test_policy_config_with_rate_limiting() {
    let mut file = NamedTempFile::new().expect("Failed to create temp file");
    let policy_yaml = r#"
tools:
  gh:
    type: cli
    binary: /usr/bin/gh
    argv_allow_patterns: ["*"]
    rate_limit:
      max_requests: 100
      window_secs: 60
    timeout_secs: 30
"#;
    file.write_all(policy_yaml.as_bytes())
        .expect("Failed to write policy");
    file.flush().expect("Failed to flush");

    let policy = PolicyConfig::from_file(file.path().to_str().expect("Path is not UTF-8"))
        .expect("Failed to load policy");

    assert_eq!(policy.tools.len(), 1);
}

#[test]
fn test_policy_config_large_number_of_tools() {
    let mut file = NamedTempFile::new().expect("Failed to create temp file");

    let mut yaml = String::from("tools:\n");
    for i in 0..100 {
        yaml.push_str(&format!(
            "  tool{}:\n    type: cli\n    binary: /usr/bin/tool{}\n",
            i, i
        ));
    }

    file.write_all(yaml.as_bytes())
        .expect("Failed to write policy");
    file.flush().expect("Failed to flush");

    let policy = PolicyConfig::from_file(file.path().to_str().expect("Path is not UTF-8"))
        .expect("Failed to load policy");

    assert_eq!(policy.tools.len(), 100);
}

#[test]
fn test_policy_config_special_characters_in_paths() {
    let mut file = NamedTempFile::new().expect("Failed to create temp file");
    let policy_yaml = r#"
tools:
  my-tool:
    type: cli
    binary: /usr/bin/my-tool-with-dashes
    argv_allow_patterns:
      - "arg with spaces"
      - "arg-with-dashes"
      - "arg_with_underscores"
    timeout_secs: 30
"#;
    file.write_all(policy_yaml.as_bytes())
        .expect("Failed to write policy");
    file.flush().expect("Failed to flush");

    let policy = PolicyConfig::from_file(file.path().to_str().expect("Path is not UTF-8"))
        .expect("Failed to load policy");

    assert_eq!(policy.tools.len(), 1);
    assert!(policy.tools.contains_key("my-tool"));
}

#[test]
fn test_dispatcher_creation_with_empty_policy() {
    // Policy with no tools
    let policy = PolicyConfig {
        tools: HashMap::new(),
    };

    // Creating dispatchers should work
    let _cli = CliDispatcher::with_policy(policy.clone());
    let _http = HttpDispatcher::with_policy(policy);

    // Both should create successfully
}
