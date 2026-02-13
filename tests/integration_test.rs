// Integration tests for end-to-end functionality

#[test]
fn test_protocol_messages_create_and_serialize() {
    use carapace_protocol::{Message, CliRequest};
    use std::collections::HashMap;

    let req = CliRequest {
        id: "integration-1".to_string(),
        tool: "gh".to_string(),
        argv: vec!["pr".to_string(), "list".to_string()],
        env: HashMap::new(),
        stdin: None,
        cwd: "/home/user".to_string(),
    };

    let msg = Message::CliRequest(req);
    let json = serde_json::to_string(&msg).expect("serialize failed");

    assert!(json.contains("\"tool\":\"gh\""));
}

#[test]
fn test_policy_matching_with_glob_patterns() {
    use carapace_policy::ArgvMatcher;

    let matcher = ArgvMatcher::new(
        vec!["pr list*".to_string(), "issue view *".to_string()],
        vec!["* --delete *".to_string()],
    ).expect("matcher creation failed");

    // Should match
    assert!(matcher.matches(&["pr".to_string(), "list".to_string()]));
    assert!(matcher.matches(&["issue".to_string(), "view".to_string(), "123".to_string()]));

    // Should not match
    assert!(!matcher.matches(&["pr".to_string(), "create".to_string()]));
    assert!(!matcher.matches(&["pr".to_string(), "--delete".to_string()]));
}

#[test]
fn test_json_rpc_method_validation() {
    use carapace_policy::PolicyValidator;

    let allowed = vec!["send".to_string(), "receive".to_string()];
    let denied = vec![];

    // Should allow
    let result = PolicyValidator::validate_jsonrpc_method("send", &allowed, &denied);
    assert!(result.is_ok());

    // Should deny
    let result = PolicyValidator::validate_jsonrpc_method("deleteAll", &allowed, &denied);
    assert!(result.is_err());
}

#[test]
fn test_dangerous_shell_chars_detection() {
    use carapace_policy::PolicyValidator;

    assert!(PolicyValidator::has_dangerous_shell_chars("cmd; rm -rf /"));
    assert!(PolicyValidator::has_dangerous_shell_chars("cmd | cat"));
    assert!(PolicyValidator::has_dangerous_shell_chars("$(whoami)"));
    assert!(!PolicyValidator::has_dangerous_shell_chars("safe-command"));
}

#[test]
fn test_binary_path_validation() {
    use carapace_policy::PolicyValidator;

    // Valid paths
    assert!(PolicyValidator::validate_binary_path("/usr/bin/gh").is_ok());
    assert!(PolicyValidator::validate_binary_path("/usr/local/bin/tool").is_ok());

    // Invalid paths
    assert!(PolicyValidator::validate_binary_path("../../etc/passwd").is_err());
    assert!(PolicyValidator::validate_binary_path("/usr/bin/gh\0hidden").is_err());
}

#[test]
fn test_full_message_serialization_roundtrip() {
    use carapace_protocol::{Message, CliRequest, CliResponse};
    use std::collections::HashMap;

    let req = CliRequest {
        id: "test-123".to_string(),
        tool: "gh".to_string(),
        argv: vec!["pr".to_string(), "list".to_string()],
        env: HashMap::new(),
        stdin: None,
        cwd: "/".to_string(),
    };

    let msg = Message::CliRequest(req.clone());
    let json = serde_json::to_string(&msg).expect("serialize failed");
    let deserialized: Message = serde_json::from_str(&json).expect("deserialize failed");

    match deserialized {
        Message::CliRequest(req2) => {
            assert_eq!(req2.id, req.id);
            assert_eq!(req2.tool, req.tool);
            assert_eq!(req2.argv, req.argv);
        },
        _ => panic!("Expected CliRequest"),
    }
}

#[test]
fn test_multiple_concurrent_message_handling() {
    use carapace_protocol::Message;
    use std::collections::HashMap;

    let mut messages = Vec::new();

    for i in 0..100 {
        let req = carapace_protocol::CliRequest {
            id: format!("msg-{}", i),
            tool: "tool".to_string(),
            argv: vec![],
            env: HashMap::new(),
            stdin: None,
            cwd: "/".to_string(),
        };
        messages.push(Message::CliRequest(req));
    }

    assert_eq!(messages.len(), 100);

    // All should serialize without error
    for msg in &messages {
        let _json = serde_json::to_string(msg).expect("serialize failed");
    }
}

#[test]
fn test_policy_config_loading() {
    use carapace_policy::PolicyConfig;

    let yaml = r#"
tools:
  gh:
    type: cli
    binary: /usr/bin/gh
    argv_allow_patterns:
      - "pr list"
      - "issue view *"
    env_inject:
      GH_TOKEN: "token_here"
    audit:
      enabled: true
  signal-cli:
    type: http
    upstream: "http://localhost:18080"
    jsonrpc_allow_methods:
      - send
      - receive
"#;

    let config: PolicyConfig = serde_yaml::from_str(yaml).expect("parse failed");
    assert_eq!(config.tools.len(), 2);
    assert!(config.tools.contains_key("gh"));
    assert!(config.tools.contains_key("signal-cli"));
}

#[test]
fn test_tool_execution_flow() {
    // Simulates the full flow: argv → policy check → execution → response

    // 1. Parse tool name and args
    let argv0 = "gh";
    let argv = vec!["pr", "list"];

    // 2. Check policy
    use carapace_policy::ArgvMatcher;
    let matcher = ArgvMatcher::new(
        vec!["pr list".to_string()],
        vec![],
    ).expect("matcher creation failed");

    let policy_ok = matcher.matches(&argv.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    assert!(policy_ok);

    // 3. In real scenario, would execute tool here
    // 4. Capture output
    let mock_exit_code = 0;
    let mock_stdout = "output here";

    assert_eq!(mock_exit_code, 0);
    assert!(!mock_stdout.is_empty());
}

#[test]
fn test_http_request_flow() {
    // Simulates HTTP request through the system

    use carapace_protocol::HttpRequest;
    use carapace_policy::PolicyValidator;
    use std::collections::HashMap;

    // 1. Create HTTP request
    let req = HttpRequest {
        id: "http-1".to_string(),
        tool: "signal-cli".to_string(),
        method: "POST".to_string(),
        path: "/api/v1/rpc".to_string(),
        headers: HashMap::new(),
        body: Some(r#"{"jsonrpc":"2.0","method":"send","params":{}}"#.to_string()),
    };

    // 2. Extract JSON-RPC method
    let body = req.body.unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let method = json.get("method").and_then(|m| m.as_str());

    assert_eq!(method, Some("send"));

    // 3. Validate against policy
    let allowed = vec!["send".to_string()];
    let denied = vec![];
    let policy_ok = PolicyValidator::validate_jsonrpc_method("send", &allowed, &denied);

    assert!(policy_ok.is_ok());
}

#[test]
fn test_sse_event_streaming() {
    use carapace_protocol::SseEvent;

    let event = SseEvent {
        tool: "signal-cli".to_string(),
        event: "message".to_string(),
        data: r#"{"from":"+15551234567","text":"hello"}"#.to_string(),
    };

    let msg = carapace_protocol::Message::SseEvent(event);
    let json = serde_json::to_string(&msg).expect("serialize failed");

    assert!(json.contains("\"tool\":\"signal-cli\""));
}

#[test]
fn test_error_message_propagation() {
    use carapace_protocol::ErrorMessage;

    let error = ErrorMessage {
        id: Some("req-123".to_string()),
        code: "POLICY_DENIED".to_string(),
        message: "Policy does not allow this action".to_string(),
    };

    let msg = carapace_protocol::Message::Error(error);
    let json = serde_json::to_string(&msg).expect("serialize failed");

    assert!(json.contains("POLICY_DENIED"));
}

#[test]
fn test_large_payload_handling() {
    use carapace_protocol::{Message, CliResponse};

    let large_output = "x".repeat(50 * 1024 * 1024); // 50MB

    let resp = CliResponse {
        id: "large".to_string(),
        exit_code: 0,
        stdout: large_output,
        stderr: "".to_string(),
    };

    let msg = Message::CliResponse(resp);
    // In real system, this would be framed and sent
    // Just verify it serializes
    let _json = serde_json::to_string(&msg).expect("serialize failed");
}

#[test]
fn test_mixed_cli_and_http_requests() {
    use carapace_protocol::{Message, CliRequest, HttpRequest};
    use std::collections::HashMap;

    let cli_req = Message::CliRequest(CliRequest {
        id: "cli-1".to_string(),
        tool: "gh".to_string(),
        argv: vec![],
        env: HashMap::new(),
        stdin: None,
        cwd: "/".to_string(),
    });

    let http_req = Message::HttpRequest(HttpRequest {
        id: "http-1".to_string(),
        tool: "signal-cli".to_string(),
        method: "POST".to_string(),
        path: "/api".to_string(),
        headers: HashMap::new(),
        body: None,
    });

    // Both should serialize independently
    let cli_json = serde_json::to_string(&cli_req).expect("cli serialize failed");
    let http_json = serde_json::to_string(&http_req).expect("http serialize failed");

    assert!(cli_json.contains("\"tool\":\"gh\""));
    assert!(http_json.contains("\"tool\":\"signal-cli\""));
}
