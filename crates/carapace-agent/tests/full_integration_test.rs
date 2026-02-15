/// Full integration test: Agent ↔ Server ↔ Mock Signal-CLI
///
/// This test simulates the complete flow with real message serialization:
/// 1. Mock signal-cli HTTP server (on port 0)
/// 2. Real Carapace server (on port 0)
/// 3. Real Carapace agent (on port 0)
/// 4. Make HTTP request through agent
/// 5. Verify response comes back
///
/// If this test passes, the system works end-to-end.
/// If it fails, we've reproduced the production issue locally.
use carapace_agent::{Connection, Multiplexer};
use carapace_policy::{HttpPolicy, PolicyConfig, ToolPolicy};
use carapace_protocol::{Message, MessageCodec};
use carapace_server::{cli_dispatch::CliDispatcher, http_dispatch::HttpDispatcher};
use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::codec::{FramedRead, FramedWrite};

/// Start a mock signal-cli server that responds to JSON-RPC calls
async fn start_mock_signal_cli() -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind mock signal-cli");

    let addr = listener.local_addr().unwrap();
    let server_url = format!("http://{}", addr);

    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0; 4096];
                if let Ok(n) = socket.read(&mut buf).await {
                    let _request = String::from_utf8_lossy(&buf[..n]);

                    let response_body =
                        r#"{"jsonrpc":"2.0","result":{"version":"0.13.24"},"id":"1"}"#;
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

    server_url
}

/// Start the Carapace server with signal-cli policy
async fn start_carapace_server(
    upstream: &str,
) -> (String, Arc<HttpDispatcher>, Arc<CliDispatcher>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind carapace server");

    let server_addr = listener.local_addr().unwrap();
    let server_url = format!("{}", server_addr);

    // Create policy
    let http_policy = HttpPolicy {
        upstream: upstream.to_string(),
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

    let http_dispatcher = Arc::new(HttpDispatcher::with_policy(policy.clone()));
    let cli_dispatcher = Arc::new(CliDispatcher::with_policy(policy));

    let http_dispatcher_clone = http_dispatcher.clone();
    let cli_dispatcher_clone = cli_dispatcher.clone();

    tokio::spawn(async move {
        while let Ok((socket, peer_addr)) = listener.accept().await {
            eprintln!("Server: New connection from {}", peer_addr);

            let (reader, writer) = socket.into_split();
            let http_dispatcher = http_dispatcher_clone.clone();
            let cli_dispatcher = cli_dispatcher_clone.clone();

            tokio::spawn(async move {
                let mut frame_read = FramedRead::new(reader, MessageCodec);
                let mut frame_write = FramedWrite::new(writer, MessageCodec);

                while let Some(result) = frame_read.next().await {
                    match result {
                        Ok(msg) => {
                            eprintln!("Server: Received message: {:?}", msg);

                            let response: Option<Message> = match msg {
                                Message::HttpRequest(req) => {
                                    eprintln!("Server: Processing HttpRequest id={}", req.id);
                                    match http_dispatcher.dispatch_http(req.clone(), None).await {
                                        Ok(Some(resp)) => {
                                            eprintln!(
                                                "Server: HttpRequest succeeded, sending response"
                                            );
                                            Some(Message::HttpResponse(resp))
                                        }
                                        Ok(None) => {
                                            eprintln!("Server: HttpRequest was SSE streaming, no response");
                                            None
                                        }
                                        Err(e) => {
                                            eprintln!("Server: HttpRequest failed: {}", e);
                                            Some(Message::Error(carapace_protocol::ErrorMessage {
                                                id: Some(req.id),
                                                code: "error".to_string(),
                                                message: format!("{}", e),
                                            }))
                                        }
                                    }
                                }
                                Message::CliRequest(req) => {
                                    match cli_dispatcher.dispatch_cli(req.clone()).await {
                                        Ok(resp) => Some(Message::CliResponse(resp)),
                                        Err(e) => {
                                            Some(Message::Error(carapace_protocol::ErrorMessage {
                                                id: Some(req.id),
                                                code: "error".to_string(),
                                                message: format!("{}", e),
                                            }))
                                        }
                                    }
                                }
                                _ => None,
                            };

                            if let Some(response_msg) = response {
                                eprintln!("Server: Sending response");
                                if let Err(e) = frame_write.send(response_msg).await {
                                    eprintln!("Server: Failed to send response: {}", e);
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Server: Error reading message: {}", e);
                            break;
                        }
                    }
                }

                eprintln!("Server: Connection closed");
            });
        }
    });

    (server_url, http_dispatcher, cli_dispatcher)
}

#[tokio::test]
async fn test_full_integration_signal_cli() {
    eprintln!("\n=== Starting Full Integration Test ===\n");

    // 1. Start mock signal-cli
    eprintln!("1. Starting mock signal-cli server...");
    let signal_cli_url = start_mock_signal_cli().await;
    eprintln!("   Mock signal-cli ready at: {}", signal_cli_url);

    // 2. Start carapace server
    eprintln!("\n2. Starting carapace server...");
    let (server_addr, _http_disp, _cli_disp) = start_carapace_server(&signal_cli_url).await;
    eprintln!("   Carapace server ready at: {}", server_addr);

    // Give services time to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // 3. Start carapace agent
    eprintln!("\n3. Starting carapace agent...");
    let multiplexer = Arc::new(Multiplexer::new());

    // Extract host and port from server_addr
    let (host, port) = {
        let parts: Vec<&str> = server_addr.split(':').collect();
        if parts.len() != 2 {
            panic!("Invalid server address: {}", server_addr);
        }
        (
            parts[0].to_string(),
            parts[1].parse::<u16>().expect("Invalid port"),
        )
    };
    eprintln!("   Connecting to server at {}:{}", host, port);

    let connection = Arc::new(
        Connection::connect_tcp_with_config(&host, port, 3, 100)
            .await
            .expect("Failed to connect agent to server"),
    );
    eprintln!("   Carapace agent connected to server");

    // Spawn background task to handle responses from server
    let connection_read = connection.clone();
    let multiplexer_response = multiplexer.clone();
    tokio::spawn(async move {
        loop {
            match connection_read.recv().await {
                Ok(Some(msg)) => {
                    eprintln!("Agent: Received message from server: {:?}", msg);
                    multiplexer_response.handle_response(msg).await;
                }
                Ok(None) => {
                    eprintln!("Agent: Connection closed by server");
                    break;
                }
                Err(e) => {
                    eprintln!("Agent: Error receiving message: {}", e);
                    break;
                }
            }
        }
    });

    // Give background task time to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // 4. Make a request like the HTTP handler would
    eprintln!("\n4. Sending HTTP request through agent...");
    let request_id = "test-integration-1".to_string();
    let mut rx = multiplexer.register_waiter(request_id.clone()).await;
    eprintln!("   Registered waiter for request_id={}", request_id);

    let http_req = carapace_protocol::HttpRequest {
        id: request_id.clone(),
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

    eprintln!("   Sending HTTP request to server...");
    connection
        .send(Message::HttpRequest(http_req))
        .await
        .expect("Failed to send request");

    // 5. Wait for response with timeout
    eprintln!("   Waiting for response...");
    let response_result = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await;

    eprintln!("\n=== Test Results ===");
    match response_result {
        Ok(Some(Message::HttpResponse(resp))) => {
            eprintln!("✓ SUCCESS - Received HttpResponse!");
            eprintln!("  Status: {}", resp.status);
            eprintln!("  Body: {:?}", resp.body);
            assert_eq!(resp.status, 200);
            assert!(resp.body.is_some());
            assert!(resp.body.unwrap().contains("0.13.24"));
        }
        Ok(Some(msg)) => {
            eprintln!("✗ FAIL - Received wrong message type: {:?}", msg);
            panic!("Expected HttpResponse");
        }
        Ok(None) => {
            eprintln!("✗ FAIL - Channel closed");
            panic!("Channel closed unexpectedly");
        }
        Err(_) => {
            eprintln!("✗ FAIL - Timeout waiting for response");
            panic!("Timeout after 5 seconds");
        }
    }

    eprintln!("\n=== Integration Test PASSED ===\n");
}
