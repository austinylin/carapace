/// OpenClaw Integration Test - Real Signal-CLI API Scenarios
///
/// This test suite simulates real OpenClaw → Carapace → signal-cli flows:
/// 1. Tests actual signal-cli HTTP API response formats
/// 2. Tests actual OpenClaw request patterns
/// 3. Tests sending messages with recipient arrays
/// 4. Tests receiving messages via SSE streaming
/// 5. Tests error cases and policy enforcement
/// 6. Tests concurrent requests
///
/// Based on:
/// - signal-cli HTTP API documentation
/// - OpenClaw TypeScript integration code
/// - Real production request/response patterns

use carapace_agent::{Connection, Multiplexer};
use carapace_policy::{HttpPolicy, PolicyConfig, ParamFilter, ToolPolicy};
use carapace_protocol::{Message, MessageCodec};
use carapace_server::{
    cli_dispatch::CliDispatcher,
    http_dispatch::HttpDispatcher,
};
use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::codec::{FramedRead, FramedWrite};

/// Extract request ID from JSON-RPC body for echoing in response
fn extract_request_id(request: &str) -> Option<String> {
    if let Some(start) = request.find("\"id\":\"") {
        let after_prefix = &request[start + 6..];
        if let Some(end) = after_prefix.find('"') {
            return Some(after_prefix[..end].to_string());
        }
    }
    None
}

/// Mock signal-cli server with real API responses
struct MockSignalCli {
    addr: String,
}

impl MockSignalCli {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("Failed to bind mock signal-cli");

        let addr = listener.local_addr().unwrap();
        let server_url = format!("http://{}", addr);

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((mut socket, _)) => {
                        tokio::spawn(async move {
                            // Handle multiple requests on this connection
                            loop {
                                let mut buf = vec![0; 8192];
                                match socket.read(&mut buf).await {
                                    Ok(0) => break, // Connection closed
                                    Ok(n) => {
                                        let request = String::from_utf8_lossy(&buf[..n]);
                                        eprintln!("Mock signal-cli received:\n{}", request);

                                        // Route to appropriate handler
                                        if request.contains("GET /api/v1/events") {
                                            // SSE is special - write events and close connection
                                            if socket.write_all(MockSignalCli::handle_events_request(&request).as_bytes()).await.is_err() {
                                                break;
                                            }
                                            // For SSE, close connection after sending events (simulates stream ending)
                                            drop(socket);
                                            break;
                                        } else {
                                            let response = if request.contains("POST /api/v1/rpc") {
                                                MockSignalCli::handle_rpc_request(&request)
                                            } else {
                                                MockSignalCli::handle_default_request()
                                            };

                                            if socket.write_all(response.as_bytes()).await.is_err() {
                                                break;
                                            }
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                        });
                    }
                    Err(_) => break,
                }
            }
        });

        MockSignalCli {
            addr: server_url,
        }
    }

    fn handle_rpc_request(request: &str) -> String {
        // Extract the request ID from the JSON body to echo back in response
        let id = extract_request_id(request).unwrap_or_else(|| "1".to_string());

        // Parse to detect method from actual OpenClaw request format
        let response_body = if request.contains("\"method\":\"send\"") {
            // send response: returns timestamp (milliseconds since epoch)
            // This matches signal-cli HTTP API response
            format!(r#"{{"jsonrpc":"2.0","result":{{"timestamp":1707920000000}},"id":"{id}"}}"#)
        } else if request.contains("\"method\":\"version\"") {
            // version response: returns version string
            format!(r#"{{"jsonrpc":"2.0","result":{{"version":"0.13.24"}},"id":"{id}"}}"#)
        } else if request.contains("\"method\":\"sendTyping\"") {
            // sendTyping response: null result
            format!(r#"{{"jsonrpc":"2.0","result":null,"id":"{id}"}}"#)
        } else if request.contains("\"method\":\"sendReceipt\"") {
            // sendReceipt response: null result
            format!(r#"{{"jsonrpc":"2.0","result":null,"id":"{id}"}}"#)
        } else if request.contains("\"method\":\"sendReaction\"") {
            // sendReaction response: returns timestamp
            format!(r#"{{"jsonrpc":"2.0","result":{{"timestamp":1707920000000}},"id":"{id}"}}"#)
        } else {
            // Unknown method - return JSON-RPC error per spec
            format!(r#"{{"jsonrpc":"2.0","error":{{"code":-32601,"message":"Method not found"}},"id":"{id}"}}"#)
        };

        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            response_body.len(),
            response_body
        )
    }

    fn handle_events_request(_request: &str) -> String {
        // SSE response - stream of events
        // Based on real signal-cli ServerSentEventSender format from:
        // org/asamk/signal/http/ServerSentEventSender.java
        //
        // Format per HTML spec: https://html.spec.whatwg.org/multipage/server-sent-events.html
        // Each event is:
        //   id:event-id
        //   event:event-type
        //   data:json-data
        //   <blank line>
        //
        // signal-cli JsonReceiveMessageHandler sends:
        // {
        //   "account": "+1234567890",
        //   "envelope": { message details }
        // }

        let events = vec![
            // Event 1: Incoming message
            "id:1\nevent:message\ndata:{\"account\":\"+12242120288\",\"envelope\":{\"timestamp\":1707920000000,\"source\":\"+12025551234\",\"sourceDevice\":1,\"message\":{\"body\":\"Hello from Signal\"},\"expirationTime\":0}}\n\n",
            // Event 2: Keep-alive (colon at start means comment, receiver ignores it)
            ":\n\n",
            // Event 3: Another incoming message
            "id:2\nevent:message\ndata:{\"account\":\"+12242120288\",\"envelope\":{\"timestamp\":1707920001000,\"source\":\"+12025559999\",\"sourceDevice\":1,\"message\":{\"body\":\"Another message\"},\"expirationTime\":0}}\n\n",
        ];

        let body = events.join("");
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn handle_default_request() -> String {
        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\n\r\n{\"error\":\"Not found\"}".to_string()
    }
}

/// Start carapace server with signal-cli policy
async fn start_carapace_server(upstream: &str) -> (String, Arc<HttpDispatcher>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind carapace server");

    let server_addr = listener.local_addr().unwrap();
    let server_url = format!("{}", server_addr);

    // Create signal-cli policy matching production config
    let mut param_filters = HashMap::new();

    // send method - recipient field is array
    param_filters.insert(
        "send".to_string(),
        ParamFilter {
            field: "recipient".to_string(),
            allow_patterns: vec!["+1*".to_string()],
            deny_patterns: vec![],
        },
    );

    // sendTyping
    param_filters.insert(
        "sendTyping".to_string(),
        ParamFilter {
            field: "recipient".to_string(),
            allow_patterns: vec!["+1*".to_string()],
            deny_patterns: vec![],
        },
    );

    // sendReceipt
    param_filters.insert(
        "sendReceipt".to_string(),
        ParamFilter {
            field: "recipient".to_string(),
            allow_patterns: vec!["+1*".to_string()],
            deny_patterns: vec![],
        },
    );

    // sendReaction - uses plural field
    param_filters.insert(
        "sendReaction".to_string(),
        ParamFilter {
            field: "recipients".to_string(),
            allow_patterns: vec!["+1*".to_string()],
            deny_patterns: vec![],
        },
    );

    let http_policy = HttpPolicy {
        upstream: upstream.to_string(),
        jsonrpc_allow_methods: vec![
            "send".to_string(),
            "sendTyping".to_string(),
            "sendReceipt".to_string(),
            "sendReaction".to_string(),
            "version".to_string(),
        ],
        jsonrpc_deny_methods: vec!["deleteEverything".to_string()],
        jsonrpc_param_filters: param_filters,
        rate_limit: None,
        timeout_secs: Some(30),
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
        while let Ok((socket, _peer_addr)) = listener.accept().await {
            let (reader, writer) = socket.into_split();
            let http_dispatcher = http_dispatcher_clone.clone();
            let cli_dispatcher = cli_dispatcher_clone.clone();

            tokio::spawn(async move {
                let mut frame_read = FramedRead::new(reader, MessageCodec);
                let mut frame_write = FramedWrite::new(writer, MessageCodec);

                while let Some(result) = frame_read.next().await {
                    match result {
                        Ok(msg) => {
                            let response: Option<Message> = match msg {
                                Message::HttpRequest(req) => {
                                    match http_dispatcher.dispatch_http(req.clone()).await {
                                        Ok(resp) => Some(Message::HttpResponse(resp)),
                                        Err(e) => Some(Message::Error(
                                            carapace_protocol::ErrorMessage {
                                                id: Some(req.id),
                                                code: "error".to_string(),
                                                message: format!("{}", e),
                                            },
                                        )),
                                    }
                                }
                                Message::CliRequest(req) => {
                                    match cli_dispatcher.dispatch_cli(req.clone()).await {
                                        Ok(resp) => Some(Message::CliResponse(resp)),
                                        Err(e) => Some(Message::Error(
                                            carapace_protocol::ErrorMessage {
                                                id: Some(req.id),
                                                code: "error".to_string(),
                                                message: format!("{}", e),
                                            },
                                        )),
                                    }
                                }
                                _ => None,
                            };

                            if let Some(response_msg) = response {
                                let _ = frame_write.send(response_msg).await;
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
        }
    });

    (server_url, http_dispatcher)
}

#[tokio::test]
#[ignore] // cargo test --test openclaw_integration_test -- --ignored --nocapture
async fn test_openclaw_send_message() {
    eprintln!("\n=== Test: Send Message (OpenClaw → signal-cli) ===\n");

    let mock = MockSignalCli::start().await;
    let (server_addr, _) = start_carapace_server(&mock.addr).await;

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let multiplexer = Arc::new(Multiplexer::new());

    let (host, port) = {
        let parts: Vec<&str> = server_addr.split(':').collect();
        (parts[0].to_string(), parts[1].parse::<u16>().unwrap())
    };

    let connection = Arc::new(
        Connection::connect_tcp_with_config(&host, port, 3, 100)
            .await
            .expect("Failed to connect"),
    );

    let connection_read = connection.clone();
    let multiplexer_response = multiplexer.clone();
    tokio::spawn(async move {
        loop {
            match connection_read.recv().await {
                Ok(Some(msg)) => {
                    multiplexer_response.handle_response(msg).await;
                }
                _ => break,
            }
        }
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Simulate OpenClaw sending a message
    // Format from actual OpenClaw src/signal/send.ts buildTargetParams():
    // - recipient is an ARRAY: ["+12025551234"]
    // - message is the text
    // - OpenClaw uses UUID string for request ID
    let request_id = "550e8400-e29b-41d4-a716-446655440000".to_string();
    let rx = multiplexer.register_waiter(request_id.clone()).await;

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
        // Real OpenClaw request format from src/signal/send.ts
        body: Some(
            r#"{"jsonrpc":"2.0","id":"550e8400-e29b-41d4-a716-446655440000","method":"send","params":{"message":"Hello from OpenClaw","recipient":["+12025551234"]}}"#
                .to_string(),
        ),
    };

    eprintln!("Sending message request (UUID ID)...");
    connection
        .send(Message::HttpRequest(http_req))
        .await
        .expect("Failed to send");

    let response_result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        rx,
    )
    .await;

    match response_result {
        Ok(Ok(Message::HttpResponse(resp))) => {
            eprintln!("✓ Response received: status={}", resp.status);
            assert_eq!(resp.status, 200);
            assert!(resp.body.is_some());
            let body = resp.body.unwrap();
            assert!(body.contains("timestamp"), "Expected timestamp in response: {}", body);
            assert!(body.contains("550e8400-e29b-41d4-a716-446655440000"), "Expected UUID ID echoed back");
        }
        _ => panic!("Did not receive expected response"),
    }
}

#[tokio::test]
#[ignore] // cargo test --test openclaw_integration_test -- --ignored --nocapture
async fn test_openclaw_send_typing_indicator() {
    eprintln!("\n=== Test: Send Typing Indicator ===\n");

    let mock = MockSignalCli::start().await;
    let (server_addr, _) = start_carapace_server(&mock.addr).await;

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let multiplexer = Arc::new(Multiplexer::new());
    let (host, port) = {
        let parts: Vec<&str> = server_addr.split(':').collect();
        (parts[0].to_string(), parts[1].parse::<u16>().unwrap())
    };

    let connection = Arc::new(
        Connection::connect_tcp_with_config(&host, port, 3, 100)
            .await
            .expect("Failed to connect"),
    );

    let connection_read = connection.clone();
    let multiplexer_response = multiplexer.clone();
    tokio::spawn(async move {
        loop {
            match connection_read.recv().await {
                Ok(Some(msg)) => {
                    multiplexer_response.handle_response(msg).await;
                }
                _ => break,
            }
        }
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let request_id = "550e8400-e29b-41d4-a716-446655440001".to_string();
    let rx = multiplexer.register_waiter(request_id.clone()).await;

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
        // Real OpenClaw format from src/signal/send.ts sendTypingSignal()
        // Uses recipient array format
        body: Some(
            r#"{"jsonrpc":"2.0","id":"550e8400-e29b-41d4-a716-446655440001","method":"sendTyping","params":{"recipient":["+12025551234"]}}"#
                .to_string(),
        ),
    };

    eprintln!("Sending typing indicator...");
    connection
        .send(Message::HttpRequest(http_req))
        .await
        .expect("Failed to send");

    let response_result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        rx,
    )
    .await;

    match response_result {
        Ok(Ok(Message::HttpResponse(resp))) => {
            eprintln!("✓ Typing indicator sent: status={}", resp.status);
            assert_eq!(resp.status, 200);
            // sendTyping returns null result
            assert!(resp.body.is_some());
        }
        _ => panic!("Did not receive expected response"),
    }
}

#[tokio::test]
#[ignore] // cargo test --test openclaw_integration_test -- --ignored --nocapture
async fn test_openclaw_receive_events_sse() {
    eprintln!("\n=== Test: Receive Events (SSE Streaming) ===\n");

    let mock = MockSignalCli::start().await;
    let (server_addr, _) = start_carapace_server(&mock.addr).await;

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let multiplexer = Arc::new(Multiplexer::new());
    let (host, port) = {
        let parts: Vec<&str> = server_addr.split(':').collect();
        (parts[0].to_string(), parts[1].parse::<u16>().unwrap())
    };

    let connection = Arc::new(
        Connection::connect_tcp_with_config(&host, port, 3, 100)
            .await
            .expect("Failed to connect"),
    );

    let connection_read = connection.clone();
    let multiplexer_response = multiplexer.clone();
    tokio::spawn(async move {
        loop {
            match connection_read.recv().await {
                Ok(Some(msg)) => {
                    multiplexer_response.handle_response(msg).await;
                }
                _ => break,
            }
        }
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let request_id = "550e8400-e29b-41d4-a716-446655440003".to_string();
    let rx = multiplexer.register_waiter(request_id.clone()).await;

    // Request SSE stream for events (from OpenClaw src/signal/client.ts streamSignalEvents)
    let http_req = carapace_protocol::HttpRequest {
        id: request_id.clone(),
        tool: "signal-cli".to_string(),
        method: "GET".to_string(),
        path: "/api/v1/events?account=%2B12242120288".to_string(),
        headers: HashMap::new(),
        body: None,
    };

    eprintln!("Requesting event stream...");
    connection
        .send(Message::HttpRequest(http_req))
        .await
        .expect("Failed to send");

    let response_result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        rx,
    )
    .await;

    match response_result {
        Ok(Ok(Message::HttpResponse(resp))) => {
            eprintln!("✓ Event stream received: status={}", resp.status);
            assert_eq!(resp.status, 200);

            // Verify SSE content
            let body = resp.body.expect("No body in response");
            assert!(body.contains("data:"));
            assert!(body.contains("message"));
            eprintln!("Events:\n{}", body);
        }
        _ => panic!("Did not receive expected response"),
    }
}

#[tokio::test]
#[ignore] // cargo test --test openclaw_integration_test -- --ignored --nocapture
async fn test_openclaw_blocked_number() {
    eprintln!("\n=== Test: Policy - Block Number ===\n");

    let mock = MockSignalCli::start().await;
    let (server_addr, _) = start_carapace_server(&mock.addr).await;

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let multiplexer = Arc::new(Multiplexer::new());
    let (host, port) = {
        let parts: Vec<&str> = server_addr.split(':').collect();
        (parts[0].to_string(), parts[1].parse::<u16>().unwrap())
    };

    let connection = Arc::new(
        Connection::connect_tcp_with_config(&host, port, 3, 100)
            .await
            .expect("Failed to connect"),
    );

    let connection_read = connection.clone();
    let multiplexer_response = multiplexer.clone();
    tokio::spawn(async move {
        loop {
            match connection_read.recv().await {
                Ok(Some(msg)) => {
                    multiplexer_response.handle_response(msg).await;
                }
                _ => break,
            }
        }
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let request_id = "550e8400-e29b-41d4-a716-446655440002".to_string();
    let rx = multiplexer.register_waiter(request_id.clone()).await;

    // Try to send to non-US number (policy allows +1* only)
    // Real OpenClaw format
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
        body: Some(
            r#"{"jsonrpc":"2.0","id":"550e8400-e29b-41d4-a716-446655440002","method":"send","params":{"message":"International number","recipient":["+442071234567"]}}"#
                .to_string(),
        ),
    };

    eprintln!("Sending to blocked number (+44 UK)...");
    connection
        .send(Message::HttpRequest(http_req))
        .await
        .expect("Failed to send");

    let response_result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        rx,
    )
    .await;

    match response_result {
        Ok(Ok(Message::Error(err))) => {
            eprintln!("✓ Correctly blocked: {}", err.message);
            assert!(err.message.contains("Denied") || err.message.contains("not in allow list"));
        }
        Ok(Ok(msg)) => {
            eprintln!("Unexpected message type: {:?}", msg);
            panic!("Expected error for blocked number");
        }
        _ => panic!("Did not receive expected response"),
    }
}

#[tokio::test]
#[ignore] // cargo test --test openclaw_integration_test -- --ignored --nocapture
async fn test_openclaw_concurrent_requests() {
    eprintln!("\n=== Test: Concurrent Requests ===\n");

    let mock = MockSignalCli::start().await;
    let (server_addr, _) = start_carapace_server(&mock.addr).await;

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let multiplexer = Arc::new(Multiplexer::new());
    let (host, port) = {
        let parts: Vec<&str> = server_addr.split(':').collect();
        (parts[0].to_string(), parts[1].parse::<u16>().unwrap())
    };

    let connection = Arc::new(
        Connection::connect_tcp_with_config(&host, port, 3, 100)
            .await
            .expect("Failed to connect"),
    );

    let connection_read = connection.clone();
    let multiplexer_response = multiplexer.clone();
    tokio::spawn(async move {
        loop {
            match connection_read.recv().await {
                Ok(Some(msg)) => {
                    multiplexer_response.handle_response(msg).await;
                }
                _ => break,
            }
        }
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Send 5 concurrent requests - each with different UUID ID
    let mut handles = vec![];

    for i in 0..5 {
        let conn = connection.clone();
        let mux = multiplexer.clone();

        let handle = tokio::spawn(async move {
            // Generate unique UUID-like ID for each request (matching OpenClaw format)
            let request_id = format!("550e8400-e29b-41d4-a716-44665544000{}", i);
            let rx = mux.register_waiter(request_id.clone()).await;

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
                // Real OpenClaw version request format
                body: Some(
                    format!(r#"{{"jsonrpc":"2.0","id":"{}","method":"version","params":{{}}}}"#, request_id),
                ),
            };

            conn.send(Message::HttpRequest(http_req))
                .await
                .expect("Failed to send");

            tokio::time::timeout(std::time::Duration::from_secs(5), rx)
                .await
                .expect("Timeout")
                .expect("Channel error")
        });

        handles.push(handle);
    }

    eprintln!("Waiting for 5 concurrent responses...");
    for handle in handles {
        match handle.await {
            Ok(Message::HttpResponse(resp)) => {
                eprintln!("✓ Got response: status={}", resp.status);
                assert_eq!(resp.status, 200);
            }
            Ok(msg) => {
                eprintln!("Got message type: {:?}", msg);
                panic!("Expected HttpResponse, got different message type");
            }
            Err(e) => {
                eprintln!("Handle join error: {}", e);
                panic!("Task error");
            }
        }
    }

    eprintln!("✓ All concurrent requests completed successfully");
}
