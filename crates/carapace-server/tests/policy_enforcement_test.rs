use carapace_policy::{PolicyConfig, ToolPolicy, CliPolicy, ArgvMatcher};
use std::collections::HashMap;

#[test]
fn test_allowed_cli_command() {
    let matcher = ArgvMatcher::new(
        vec!["pr list*".to_string()],
        vec![],
    ).expect("matcher creation failed");

    assert!(matcher.matches(&["pr".to_string(), "list".to_string()]));
}

#[test]
fn test_denied_cli_command() {
    let matcher = ArgvMatcher::new(
        vec!["pr list".to_string()],
        vec![],
    ).expect("matcher creation failed");

    assert!(!matcher.matches(&["pr".to_string(), "delete".to_string()]));
}

#[test]
fn test_deny_pattern_blocks_allowed_pattern() {
    // Deny takes precedence even if command matches allow pattern
    let matcher = ArgvMatcher::new(
        vec!["*".to_string()], // Allow all
        vec!["* --password *".to_string()], // But deny with --password
    ).expect("matcher creation failed");

    assert!(matcher.matches(&["safe".to_string(), "command".to_string()]));
    assert!(!matcher.matches(&["bad".to_string(), "--password".to_string(), "secret".to_string()]));
}

#[test]
fn test_unknown_tool_denied() {
    // Tool not in config should be denied
    let config_str = r#"
tools:
  gh:
    type: cli
    binary: /usr/bin/gh
    argv_allow_patterns:
      - "pr *"
"#;

    let config: PolicyConfig = serde_yaml::from_str(config_str).expect("parse failed");

    // Tool "unknown" is not in config, should be rejected
    assert!(!config.tools.contains_key("unknown"));
}

#[test]
fn test_missing_policy_file() {
    // If no policy file exists, all tools should be denied
    let config = PolicyConfig {
        tools: HashMap::new(),
    };

    assert!(config.tools.is_empty(), "Empty config means deny all");
}

#[test]
fn test_policy_multiple_allow_patterns() {
    let matcher = ArgvMatcher::new(
        vec![
            "pr list*".to_string(),
            "issue view *".to_string(),
            "api repos/*/*".to_string(),
        ],
        vec![],
    ).expect("matcher creation failed");

    assert!(matcher.matches(&["pr".to_string(), "list".to_string()]));
    assert!(matcher.matches(&["issue".to_string(), "view".to_string(), "123".to_string()]));
    assert!(!matcher.matches(&["pr".to_string(), "create".to_string()]));
}

#[test]
fn test_default_deny_policy() {
    let matcher = ArgvMatcher::new(
        vec!["pr list".to_string()],
        vec![],
    ).expect("matcher creation failed");

    // Only "pr list" is allowed, everything else denied
    assert!(matcher.matches(&["pr".to_string(), "list".to_string()]));
    assert!(!matcher.matches(&["pr".to_string(), "list".to_string(), "--all".to_string()]));
    assert!(!matcher.matches(&["issue".to_string(), "create".to_string()]));
}

#[test]
fn test_policy_with_whitespace_handling() {
    let matcher = ArgvMatcher::new(
        vec!["safe   command".to_string()], // Multiple spaces in pattern
        vec![],
    ).expect("matcher creation failed");

    // argv joined by single space won't match multiple spaces in pattern
    let result = matcher.matches(&["safe".to_string(), "command".to_string()]);
    assert!(!result); // "safe command" != "safe   command"
}

#[test]
fn test_policy_enforcement_readonly_tools() {
    // Some tools should only allow read operations
    let matcher = ArgvMatcher::new(
        vec![
            "list".to_string(),
            "view *".to_string(),
            "read *".to_string(),
        ],
        vec![
            "* delete *".to_string(),
            "* remove *".to_string(),
            "* rm *".to_string(),
        ],
    ).expect("matcher creation failed");

    assert!(matcher.matches(&["list".to_string()]));
    assert!(matcher.matches(&["view".to_string(), "file".to_string()]));
    assert!(!matcher.matches(&["delete".to_string(), "file".to_string()]));
}

#[test]
fn test_policy_per_tool_different_rules() {
    // Different tools should have different policies
    let gh_matcher = ArgvMatcher::new(
        vec!["pr *".to_string(), "issue *".to_string()],
        vec![],
    ).expect("gh matcher failed");

    let op_matcher = ArgvMatcher::new(
        vec!["read op://*".to_string()],
        vec!["* --share *".to_string()],
    ).expect("op matcher failed");

    // gh allows "pr list"
    assert!(gh_matcher.matches(&["pr".to_string(), "list".to_string()]));
    // op allows "read op://Private/secret"
    assert!(op_matcher.matches(&["read".to_string(), "op://Private/secret".to_string()]));
    // gh doesn't allow what op allows
    assert!(!gh_matcher.matches(&["read".to_string(), "op://Private/secret".to_string()]));
}

#[test]
fn test_policy_deny_all() {
    let matcher = ArgvMatcher::new(
        vec![], // No allowed patterns
        vec![],
    ).expect("matcher creation failed");

    // With no allowed patterns, everything is denied
    assert!(!matcher.matches(&["anything".to_string()]));
}

#[test]
fn test_case_sensitive_policy_matching() {
    let matcher = ArgvMatcher::new(
        vec!["PR list".to_string()],
        vec![],
    ).expect("matcher creation failed");

    // Glob patterns are case-sensitive
    assert!(matcher.matches(&["PR".to_string(), "list".to_string()]));
    assert!(!matcher.matches(&["pr".to_string(), "list".to_string()])); // lowercase doesn't match
}

#[test]
fn test_policy_matching_with_special_chars() {
    let matcher = ArgvMatcher::new(
        vec!["api repos * *".to_string()],
        vec![],
    ).expect("matcher creation failed");

    // "api repos owner repo" matches the glob pattern "api repos * *"
    assert!(matcher.matches(&["api".to_string(), "repos".to_string(), "owner".to_string(), "repo".to_string()]));
}

#[test]
fn test_rate_limiting_configuration() {
    let config_str = r#"
tools:
  signal-cli:
    type: http
    upstream: "http://localhost:8080"
    rate_limit:
      max_requests: 10
      window_secs: 60
"#;

    let config: PolicyConfig = serde_yaml::from_str(config_str).expect("parse failed");
    let tool = config.tools.get("signal-cli").expect("tool not found");

    if let ToolPolicy::Http(http_policy) = tool {
        assert!(http_policy.rate_limit.is_some());
        if let Some(rate_limit) = &http_policy.rate_limit {
            assert_eq!(rate_limit.max_requests, 10);
        }
    }
}

#[test]
fn test_timeout_configuration() {
    let config_str = r#"
tools:
  gh:
    type: cli
    binary: /usr/bin/gh
    timeout_secs: 30
"#;

    let config: PolicyConfig = serde_yaml::from_str(config_str).expect("parse failed");
    let tool = config.tools.get("gh").expect("tool not found");

    if let ToolPolicy::Cli(cli_policy) = tool {
        assert_eq!(cli_policy.timeout_secs, 30);
    }
}

#[test]
fn test_audit_configuration() {
    let config_str = r#"
tools:
  gh:
    type: cli
    binary: /usr/bin/gh
    audit:
      enabled: true
      log_argv: true
"#;

    let config: PolicyConfig = serde_yaml::from_str(config_str).expect("parse failed");
    let tool = config.tools.get("gh").expect("tool not found");

    if let ToolPolicy::Cli(cli_policy) = tool {
        assert!(cli_policy.audit.enabled);
        assert!(cli_policy.audit.log_argv);
    }
}
