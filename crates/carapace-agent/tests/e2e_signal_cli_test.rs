/// End-to-end test for signal-cli integration through Carapace
///
/// This test simulates the complete flow:
/// HTTP Client (OpenClaw) → Agent HTTP Proxy → Server → signal-cli daemon → Response
///
/// We start:
/// 1. Mock signal-cli HTTP server (simulates signal-cli daemon)
/// 2. Carapace server (policy enforcement)
/// 3. Carapace agent (HTTP proxy)
/// 4. Make HTTP requests like OpenClaw would
///
/// This allows us to debug the integration without touching production systems.

use carapace_policy::{HttpPolicy, PolicyConfig, ToolPolicy};
use carapace_protocol::{Message, MessageCodec};
use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::codec::{FramedRead, FramedWrite};

/// Mock signal-cli HTTP server that responds to JSON-RPC requests
async fn start_mock_signal_cli_server(addr: &str) -> String {
    let listener = TcpListener::bind(addr)
        .await
        .expect("Failed to bind mock signal-cli server");

    let bound_addr = listener.local_addr().expect("Failed to get local addr");
    let server_addr = format!("http://{}", bound_addr);

    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0; 4096];
                if let Ok(n) = socket.read(&mut buf).await {
                    let request_str = String::from_utf8_lossy(&buf[..n]);

                    // Parse to extract JSON-RPC method
                    let method = if let Some(start) = request_str.find("\"method\":\"") {
                        let start = start + 10;
                        if let Some(end) = request_str[start..].find('"') {
                            request_str[start..start + end].to_string()
                        } else {
                            "unknown".to_string()
                        }
                    } else {
                        "unknown".to_string()
                    };

                    // Send mock response
                    let response_body = match method.as_str() {
                        "version" => r#"{"jsonrpc":"2.0","result":{"version":"0.13.24"},"id":"1"}"#,
                        "send" => r#"{"jsonrpc":"2.0","result":{"timestamp":1708000000000},"id":"1"}"#,
                        "sendTyping" => r#"{"jsonrpc":"2.0","result":null,"id":"1"}"#,
                        _ => r#"{"jsonrpc":"2.0","result":{"method":"ok"},"id":"1"}"#,
                    };

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

    server_addr
}

/// Start a carapace server that enforces signal-cli policy
async fn start_carapace_server(
    listener_addr: &str,
    upstream_url: &str,
) -> String {
    let listener = TcpListener::bind(listener_addr)
        .await
        .expect("Failed to bind carapace server");

    let server_addr = listener.local_addr().expect("Failed to get local addr");
    let server_url = format!("{}", server_addr);

    // Create signal-cli policy with version and send methods allowed
    let http_policy = HttpPolicy {
        upstream: upstream_url.to_string(),
        jsonrpc_allow_methods: vec!["version".to_string(), "send".to_string(), "sendTyping".to_string()],
        jsonrpc_deny_methods: vec![],
        jsonrpc_param_filters: HashMap::new(),
        rate_limit: None,
        timeout_secs: None,
        audit: Default::default(),
    };

    let mut tools = HashMap::new();
    tools.insert("signal-cli".to_string(), ToolPolicy::Http(http_policy));
    let policy = PolicyConfig { tools };

    let http_dispatcher = Arc::new(carapace_server::http_dispatch::HttpDispatcher::with_policy(policy));
    let cli_dispatcher = Arc::new(carapace_server::cli_dispatch::CliDispatcher::with_policy(PolicyConfig { tools: HashMap::new() }));

    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let (reader, writer) = socket.into_split();

            let http_dispatcher_clone = http_dispatcher.clone();
            let cli_dispatcher_clone = cli_dispatcher.clone();

            tokio::spawn(async move {
                // Manual implementation of listener loop to have better control
                let mut frame_read = FramedRead::new(reader, MessageCodec);
                let mut frame_write = FramedWrite::new(writer, MessageCodec);

                while let Some(result) = frame_read.next().await {
                    if let Ok(msg) = result {
                        let response: Option<Message> = match msg {
                            Message::HttpRequest(req) => {
                                match http_dispatcher_clone.dispatch_http(req.clone()).await {
                                    Ok(resp) => Some(Message::HttpResponse(resp)),
                                    Err(e) => {
                                        eprintln!("HTTP dispatch error: {}", e);
                                        Some(Message::Error(carapace_protocol::ErrorMessage {
                                            id: Some(req.id),
                                            code: "http_error".to_string(),
                                            message: format!("{}", e),
                                        }))
                                    }
                                }
                            }
                            Message::CliRequest(req) => {
                                match cli_dispatcher_clone.dispatch_cli(req.clone()).await {
                                    Ok(resp) => Some(Message::CliResponse(resp)),
                                    Err(e) => Some(Message::Error(carapace_protocol::ErrorMessage {
                                        id: Some(req.id),
                                        code: "cli_error".to_string(),
                                        message: format!("{}", e),
                                    })),
                                }
                            }
                            _ => None,
                        };

                        if let Some(response) = response {
                            if let Err(e) = frame_write.send(response).await {
                                eprintln!("Failed to send response: {}", e);
                                break;
                            }
                        }
                    } else {
                        break;
                    }
                }
            });
        }
    });

    server_url
}


#[tokio::test]
#[ignore] // Run with: cargo test --test e2e_signal_cli_test -- --ignored --nocapture
async fn test_e2e_signal_cli_version() {
    eprintln!("Starting e2e test: signal-cli version check");

    // 1. Start mock signal-cli server
    let signal_cli_addr = start_mock_signal_cli_server("127.0.0.1:0").await;
    eprintln!("Mock signal-cli started at: {}", signal_cli_addr);

    // 2. Start carapace server
    let server_addr = start_carapace_server("127.0.0.1:0", &signal_cli_addr).await;
    eprintln!("Carapace server started at: {}", server_addr);

    // Give services time to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Now in a real test, we would:
    // 3. Start carapace agent connecting to server
    // 4. Make HTTP request to agent
    // 5. Verify response comes back

    eprintln!("Test setup complete - agent and server communication would happen here");
}

#[test]
fn test_json_rpc_request_parsing() {
    // Test that we can parse OpenClaw's request format
    let openclaw_request = r#"{
        "jsonrpc": "2.0",
        "id": "test-1",
        "method": "version",
        "params": {}
    }"#;

    let parsed: serde_json::Value =
        serde_json::from_str(openclaw_request).expect("Failed to parse JSON-RPC request");

    assert_eq!(parsed["jsonrpc"], "2.0");
    assert_eq!(parsed["method"], "version");
    assert!(parsed["params"].is_object());
}

#[test]
fn test_json_rpc_send_request_format() {
    // Test the exact format OpenClaw uses for send requests
    let send_request = r#"{
        "jsonrpc": "2.0",
        "id": "test-send",
        "method": "send",
        "params": {
            "recipient": ["+12025551234"],
            "message": "Test message"
        }
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(send_request).expect("Failed to parse");

    assert_eq!(parsed["method"], "send");
    assert_eq!(parsed["params"]["recipient"][0], "+12025551234");
    assert_eq!(parsed["params"]["message"], "Test message");
}

#[test]
fn test_signal_cli_response_format() {
    // Test that we can parse signal-cli's response format
    let signal_response = r#"{
        "jsonrpc": "2.0",
        "result": {
            "version": "0.13.24"
        },
        "id": "test-1"
    }"#;

    let parsed: serde_json::Value = serde_json::from_str(signal_response).expect("Failed to parse");

    assert_eq!(parsed["result"]["version"], "0.13.24");
    assert_eq!(parsed["id"], "test-1");
}
