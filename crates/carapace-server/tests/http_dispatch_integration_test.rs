/// End-to-end integration test for HTTP dispatch through Carapace
///
/// This test verifies the complete flow:
/// Client → Agent → Server (policy enforcement) → Mock Upstream → Response back
///
/// We use a mock HTTP server to simulate signal-cli or other HTTP upstreams.
use carapace_policy::{HttpPolicy, PolicyConfig, ToolPolicy};
use carapace_protocol::HttpRequest;
use carapace_server::http_dispatch::HttpDispatcher;
use std::collections::HashMap;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Mock HTTP server that responds to all requests
async fn start_mock_http_server(addr: &str) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind mock server");

    let local_addr = listener.local_addr().expect("Failed to get local addr");

    // Spawn background task to handle connections
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0; 4096];
                if let Ok(n) = socket.read(&mut buf).await {
                    let request_str = String::from_utf8_lossy(&buf[..n]);

                    // Parse HTTP request to extract method and path
                    let method = if request_str.contains("POST") {
                        "POST"
                    } else if request_str.contains("GET") {
                        "GET"
                    } else {
                        "UNKNOWN"
                    };

                    let path = request_str
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/");

                    // Send mock response
                    let response_body = format!(
                        r#"{{"jsonrpc":"2.0","result":{{"method":"{}","path":"{}","status":"ok"}},"id":"1"}}"#,
                        method, path
                    );

                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        response_body.len(),
                        response_body
                    );

                    let _ = socket.write_all(response.as_bytes()).await;
                }
            });
        }
    });

    local_addr
}

#[tokio::test]
async fn test_http_dispatch_allowed_request() {
    // Start mock HTTP server
    let mock_addr = start_mock_http_server("127.0.0.1:0").await;

    // Create policy
    let http_policy = HttpPolicy {
        upstream: format!("http://{}", mock_addr),
        jsonrpc_allow_methods: vec!["version".to_string(), "send".to_string()],
        jsonrpc_deny_methods: vec![],
        jsonrpc_param_filters: HashMap::new(),
        rate_limit: None,
        timeout_secs: None,
        audit: Default::default(),
    };

    let mut tools = HashMap::new();
    tools.insert("signal-cli".to_string(), ToolPolicy::Http(http_policy));

    let policy = PolicyConfig { tools };

    // Create dispatcher with policy
    let dispatcher = HttpDispatcher::with_policy(policy);

    // Create HTTP request for 'version' method (allowed)
    let http_req = HttpRequest {
        id: "test-1".to_string(),
        tool: "signal-cli".to_string(),
        method: "POST".to_string(),
        path: "/api/v1/rpc".to_string(),
        headers: {
            let mut h = HashMap::new();
            h.insert("Content-Type".to_string(), "application/json".to_string());
            h
        },
        body: Some(r#"{"jsonrpc":"2.0","id":"1","method":"version","params":{}}"#.to_string()),
    };

    // Dispatch request
    let response = dispatcher.dispatch_http(http_req, None).await;

    // Verify response is successful
    assert!(response.is_ok());
    let http_resp = response.unwrap().unwrap();
    assert_eq!(http_resp.status, 200);
    assert!(http_resp.body.is_some());
    assert!(http_resp
        .body
        .unwrap()
        .contains("\"result\":{\"method\":\"POST\""));
}

#[tokio::test]
async fn test_http_dispatch_denied_method() {
    // Start mock HTTP server
    let mock_addr = start_mock_http_server("127.0.0.1:0").await;

    // Create policy that denies 'deleteEverything'
    let http_policy = HttpPolicy {
        upstream: format!("http://{}", mock_addr),
        jsonrpc_allow_methods: vec!["version".to_string(), "send".to_string()],
        jsonrpc_deny_methods: vec!["deleteEverything".to_string()],
        jsonrpc_param_filters: HashMap::new(),
        rate_limit: None,
        timeout_secs: None,
        audit: Default::default(),
    };

    let mut tools = HashMap::new();
    tools.insert("signal-cli".to_string(), ToolPolicy::Http(http_policy));

    let policy = PolicyConfig { tools };
    let dispatcher = HttpDispatcher::with_policy(policy);

    // Create HTTP request for denied method
    let http_req = HttpRequest {
        id: "test-2".to_string(),
        tool: "signal-cli".to_string(),
        method: "POST".to_string(),
        path: "/api/v1/rpc".to_string(),
        headers: HashMap::new(),
        body: Some(
            r#"{"jsonrpc":"2.0","id":"1","method":"deleteEverything","params":{}}"#.to_string(),
        ),
    };

    // Dispatch should fail policy validation
    let response = dispatcher.dispatch_http(http_req, None).await;

    assert!(response.is_err());
    let err_msg = response.unwrap_err().to_string();
    assert!(
        err_msg.contains("denied") || err_msg.contains("Method not allowed"),
        "Error message should mention denial or method: {}",
        err_msg
    );
}

#[tokio::test]
async fn test_http_dispatch_tool_not_in_policy() {
    // Start mock HTTP server
    let mock_addr = start_mock_http_server("127.0.0.1:0").await;

    // Create policy with only 'signal-cli' tool
    let http_policy = HttpPolicy {
        upstream: format!("http://{}", mock_addr),
        jsonrpc_allow_methods: vec!["version".to_string()],
        jsonrpc_deny_methods: vec![],
        jsonrpc_param_filters: HashMap::new(),
        rate_limit: None,
        timeout_secs: None,
        audit: Default::default(),
    };

    let mut tools = HashMap::new();
    tools.insert("signal-cli".to_string(), ToolPolicy::Http(http_policy));

    let policy = PolicyConfig { tools };
    let dispatcher = HttpDispatcher::with_policy(policy);

    // Try to use 'unknown-tool' which is not in policy
    let http_req = HttpRequest {
        id: "test-3".to_string(),
        tool: "unknown-tool".to_string(),
        method: "POST".to_string(),
        path: "/api/v1/rpc".to_string(),
        headers: HashMap::new(),
        body: Some(r#"{"jsonrpc":"2.0","id":"1","method":"version","params":{}}"#.to_string()),
    };

    // Dispatch should fail because tool is not in policy
    let response = dispatcher.dispatch_http(http_req, None).await;

    assert!(response.is_err());
    assert!(response
        .unwrap_err()
        .to_string()
        .to_lowercase()
        .contains("not in policy"));
}

#[tokio::test]
async fn test_http_dispatch_preserves_request_path() {
    // Start mock HTTP server
    let mock_addr = start_mock_http_server("127.0.0.1:0").await;

    // Create policy
    let http_policy = HttpPolicy {
        upstream: format!("http://{}", mock_addr),
        jsonrpc_allow_methods: vec!["version".to_string()],
        jsonrpc_deny_methods: vec![],
        jsonrpc_param_filters: HashMap::new(),
        rate_limit: None,
        timeout_secs: None,
        audit: Default::default(),
    };

    let mut tools = HashMap::new();
    tools.insert("signal-cli".to_string(), ToolPolicy::Http(http_policy));

    let policy = PolicyConfig { tools };
    let dispatcher = HttpDispatcher::with_policy(policy);

    // Create request with specific path
    let http_req = HttpRequest {
        id: "test-4".to_string(),
        tool: "signal-cli".to_string(),
        method: "POST".to_string(),
        path: "/api/v1/rpc".to_string(), // Specific path that should be preserved
        headers: HashMap::new(),
        body: Some(r#"{"jsonrpc":"2.0","id":"1","method":"version","params":{}}"#.to_string()),
    };

    // Dispatch request
    let response = dispatcher.dispatch_http(http_req, None).await;

    // Verify response contains the path we requested
    assert!(response.is_ok());
    let http_resp = response.unwrap().unwrap();
    let body = http_resp.body.unwrap();

    // The mock server echoes back the path it received
    assert!(body.contains("\"path\":\"/api/v1/rpc\""));
}

#[tokio::test]
async fn test_http_dispatch_with_json_rpc_param_filtering() {
    // Start mock HTTP server
    let mock_addr = start_mock_http_server("127.0.0.1:0").await;

    // Create policy with param filtering for 'send' method
    let mut param_filters = HashMap::new();
    param_filters.insert(
        "send".to_string(),
        carapace_policy::ParamFilter {
            field: "recipientNumber".to_string(),
            allow_patterns: vec!["+1555*".to_string()],
            deny_patterns: vec![],
        },
    );

    let http_policy = HttpPolicy {
        upstream: format!("http://{}", mock_addr),
        jsonrpc_allow_methods: vec!["send".to_string()],
        jsonrpc_deny_methods: vec![],
        jsonrpc_param_filters: param_filters,
        rate_limit: None,
        timeout_secs: None,
        audit: Default::default(),
    };

    let mut tools = HashMap::new();
    tools.insert("signal-cli".to_string(), ToolPolicy::Http(http_policy));

    let policy = PolicyConfig { tools };
    let dispatcher = HttpDispatcher::with_policy(policy);

    // Request with allowed phone number
    let allowed_req = HttpRequest {
        id: "test-5a".to_string(),
        tool: "signal-cli".to_string(),
        method: "POST".to_string(),
        path: "/api/v1/rpc".to_string(),
        headers: HashMap::new(),
        body: Some(
            r#"{"jsonrpc":"2.0","id":"1","method":"send","params":{"recipientNumber":"+15551234567","message":"test"}}"#.to_string(),
        ),
    };

    // Should succeed
    assert!(dispatcher.dispatch_http(allowed_req, None).await.is_ok());

    // Request with blocked phone number
    let blocked_req = HttpRequest {
        id: "test-5b".to_string(),
        tool: "signal-cli".to_string(),
        method: "POST".to_string(),
        path: "/api/v1/rpc".to_string(),
        headers: HashMap::new(),
        body: Some(
            r#"{"jsonrpc":"2.0","id":"1","method":"send","params":{"recipientNumber":"+18009999999","message":"test"}}"#.to_string(),
        ),
    };

    // Should fail policy validation (number not in allow list)
    let response = dispatcher.dispatch_http(blocked_req, None).await;
    assert!(response.is_err());
}
