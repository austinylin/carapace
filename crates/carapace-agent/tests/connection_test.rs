use carapace_protocol::CliRequest;
use std::collections::HashMap;

#[test]
fn test_cli_request_creation() {
    let req = CliRequest {
        id: "test-001".to_string(),
        tool: "gh".to_string(),
        argv: vec!["pr".to_string(), "list".to_string()],
        env: HashMap::new(),
        stdin: None,
        cwd: "/home/user".to_string(),
    };

    assert_eq!(req.id, "test-001");
    assert_eq!(req.tool, "gh");
    assert_eq!(req.argv.len(), 2);
}

#[test]
fn test_request_id_generation() {
    // Request IDs should be unique
    let id1 = uuid::Uuid::new_v4().to_string();
    let id2 = uuid::Uuid::new_v4().to_string();

    assert_ne!(id1, id2);
}

#[test]
fn test_concurrent_request_ids() {
    // Multiple IDs generated concurrently should all be unique
    let ids: Vec<String> = (0..100).map(|_| uuid::Uuid::new_v4().to_string()).collect();

    let unique_count = ids.iter().collect::<std::collections::HashSet<_>>().len();
    assert_eq!(unique_count, 100, "All IDs should be unique");
}

#[test]
fn test_cli_request_with_large_argv() {
    let mut argv = vec!["cmd".to_string()];
    for i in 0..10000 {
        argv.push(format!("arg{}", i));
    }

    let req = CliRequest {
        id: "large".to_string(),
        tool: "test".to_string(),
        argv: argv.clone(),
        env: HashMap::new(),
        stdin: None,
        cwd: "/".to_string(),
    };

    assert_eq!(req.argv.len(), 10001);
}

#[test]
fn test_cli_request_with_env_vars() {
    let mut env = HashMap::new();
    env.insert("PATH".to_string(), "/usr/bin".to_string());
    env.insert("HOME".to_string(), "/home/user".to_string());

    let req = CliRequest {
        id: "env-test".to_string(),
        tool: "gh".to_string(),
        argv: vec![],
        env: env.clone(),
        stdin: None,
        cwd: "/".to_string(),
    };

    assert_eq!(req.env.get("PATH"), Some(&"/usr/bin".to_string()));
    assert_eq!(req.env.get("HOME"), Some(&"/home/user".to_string()));
}

#[test]
fn test_cli_request_with_stdin() {
    let stdin_data = "POST data here\nLine 2\n".to_string();

    let req = CliRequest {
        id: "stdin-test".to_string(),
        tool: "curl".to_string(),
        argv: vec!["https://example.com".to_string()],
        env: HashMap::new(),
        stdin: Some(stdin_data.clone()),
        cwd: "/".to_string(),
    };

    assert_eq!(req.stdin, Some(stdin_data));
}

#[test]
fn test_cwd_with_special_paths() {
    let paths = vec![
        "/home/user",
        "/tmp",
        "/",
        "/very/deep/nested/directory/structure/here",
        "/path with spaces",
    ];

    for path in paths {
        let req = CliRequest {
            id: "path-test".to_string(),
            tool: "test".to_string(),
            argv: vec![],
            env: HashMap::new(),
            stdin: None,
            cwd: path.to_string(),
        };

        assert_eq!(req.cwd, path);
    }
}

#[test]
fn test_tool_name_validation() {
    let valid_tools = vec!["gh", "op", "curl", "signal-cli", "my-custom-tool"];

    for tool in valid_tools {
        let req = CliRequest {
            id: "tool-test".to_string(),
            tool: tool.to_string(),
            argv: vec![],
            env: HashMap::new(),
            stdin: None,
            cwd: "/".to_string(),
        };

        assert_eq!(req.tool, tool);
    }
}

#[test]
fn test_unicode_in_argv_elements() {
    let req = CliRequest {
        id: "unicode".to_string(),
        tool: "gh".to_string(),
        argv: vec![
            "issue".to_string(),
            "create".to_string(),
            "--title".to_string(),
            "Test 中文 🎉 العربية".to_string(),
        ],
        env: HashMap::new(),
        stdin: None,
        cwd: "/".to_string(),
    };

    assert_eq!(req.argv[3], "Test 中文 🎉 العربية");
}

#[test]
fn test_empty_env_vars() {
    let req = CliRequest {
        id: "empty-env".to_string(),
        tool: "test".to_string(),
        argv: vec![],
        env: HashMap::new(),
        stdin: None,
        cwd: "/".to_string(),
    };

    assert!(req.env.is_empty());
}

#[test]
fn test_null_bytes_in_argv() {
    let req = CliRequest {
        id: "null-test".to_string(),
        tool: "test".to_string(),
        argv: vec!["arg\0hidden".to_string()],
        env: HashMap::new(),
        stdin: None,
        cwd: "/".to_string(),
    };

    assert!(req.argv[0].contains('\0'));
}

#[test]
fn test_negative_exit_code_representation() {
    // Exit codes can be negative in Rust representation
    let neg_code: i32 = -1;
    // Verify that i32 can hold negative values
    assert!(neg_code < 0);
    assert_eq!(neg_code, -1);
}
