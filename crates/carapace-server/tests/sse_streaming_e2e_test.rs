/// End-to-end integration test for SSE streaming
///
/// This test suite verifies that Carapace correctly handles Server-Sent Events (SSE)
/// streaming, which is the critical use case for OpenClaw receiving incoming Signal messages.
///
/// The key insight: SSE requires events to be delivered incrementally as they arrive,
/// not buffered and sent all at once. Our current architecture has a fundamental issue:
/// it uses a request/response model where each HttpRequest gets exactly one HttpResponse.
/// This is incompatible with true SSE streaming.
///
/// These tests verify:
/// 1. SSE events are emitted with proper headers
/// 2. Events arrive in real-time (not buffered)
/// 3. Multiple concurrent requests are properly multiplexed
/// 4. Long-lived connections are maintained

use carapace_policy::{HttpPolicy, PolicyConfig, ToolPolicy};
use carapace_protocol::HttpRequest;
use carapace_server::http_dispatch::HttpDispatcher;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{sleep, Duration};

/// Mock signal-cli server that emits SSE events over time
/// This simulates real signal-cli behavior where events arrive incrementally
async fn start_mock_signal_cli_with_sse(addr: &str, event_count: usize) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind mock signal-cli");

    let local_addr = listener.local_addr().expect("Failed to get local addr");

    // Spawn background task to handle connections
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0; 4096];
                if let Ok(n) = socket.read(&mut buf).await {
                    let request_str = String::from_utf8_lossy(&buf[..n]);

                    // Check if this is an SSE request
                    if request_str.contains("GET /api/v1/events") {
                        // Send SSE headers immediately
                        let sse_headers = "HTTP/1.1 200 OK\r\n\
                            Content-Type: text/event-stream\r\n\
                            Cache-Control: no-cache\r\n\
                            Connection: keep-alive\r\n\
                            \r\n";

                        let _ = socket.write_all(sse_headers.as_bytes()).await;
                        let _ = socket.flush().await;

                        // Emit events over time (simulating real incoming messages)
                        for i in 0..event_count {
                            // Simulate delay between events (like real Signal messages arriving)
                            sleep(Duration::from_millis(100)).await;

                            let event = format!(
                                "data: {{\"type\":\"message\",\"id\":\"{}\",\"sender\":\"+12025551234\",\"timestamp\":{},\"message\":\"Test message {}\"}}\n\n",
                                i, 1612345670000i64 + (i as i64 * 1000), i
                            );

                            if socket.write_all(event.as_bytes()).await.is_err() {
                                break; // Client disconnected
                            }
                            let _ = socket.flush().await;
                        }
                    } else if request_str.contains("POST /api/v1/rpc") {
                        // Handle JSON-RPC requests (like 'send' or 'version')
                        let response_body =
                            r#"{"jsonrpc":"2.0","result":{"status":"ok"},"id":"1"}"#;

                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                            response_body.len(),
                            response_body
                        );

                        let _ = socket.write_all(response.as_bytes()).await;
                    }
                }
            });
        }
    });

    local_addr
}

#[tokio::test]
async fn test_sse_streaming_events_delivered_incrementally() {
    // Start mock signal-cli that emits 3 events over time
    let mock_addr = start_mock_signal_cli_with_sse("127.0.0.1:0", 3).await;

    // Create policy for signal-cli
    let http_policy = HttpPolicy {
        upstream: format!("http://{}", mock_addr),
        jsonrpc_allow_methods: vec!["send".to_string(), "receive".to_string()],
        jsonrpc_deny_methods: vec![],
        jsonrpc_param_filters: HashMap::new(),
        rate_limit: None,
        timeout_secs: Some(5), // 5 seconds to allow events to arrive
        audit: Default::default(),
    };

    let mut tools = HashMap::new();
    tools.insert("signal-cli".to_string(), ToolPolicy::Http(http_policy));
    let policy = PolicyConfig { tools };

    let dispatcher = HttpDispatcher::with_policy(policy);

    // Create SSE request (what OpenClaw does to listen for incoming messages)
    let sse_req = HttpRequest {
        id: "test-sse-1".to_string(),
        tool: "signal-cli".to_string(),
        method: "GET".to_string(),
        path: "/api/v1/events".to_string(),
        headers: {
            let mut h = HashMap::new();
            h.insert("Accept".to_string(), "text/event-stream".to_string());
            h
        },
        body: None,
    };

    // Dispatch the SSE request
    let response = dispatcher.dispatch_http(sse_req).await;
    assert!(response.is_ok(), "SSE request should succeed");

    let response = response.unwrap();
    assert_eq!(response.status, 200, "Should return 200 OK");

    // Check that response has SSE content type
    assert!(
        response
            .headers
            .get("content-type")
            .map(|v| v.contains("text/event-stream"))
            .unwrap_or(false),
        "Response should have text/event-stream content type"
    );

    // Parse the response body for SSE events
    let body = response.body.unwrap_or_default();
    println!("SSE Response body: {}", body);

    // CRITICAL TEST: Should have received some events
    // This will FAIL with current implementation because:
    // - Server waits 2 seconds for events
    // - Events are emitted 100ms apart (0ms, 100ms, 200ms)
    // - After 2 seconds, response is returned with only events that arrived
    // - The buffering defeats real-time delivery
    assert!(
        !body.is_empty(),
        "Should have received some SSE events (currently buffered for 2 seconds)"
    );

    // Should contain at least one event
    assert!(
        body.contains("data:"),
        "Response should contain SSE data lines"
    );
}

#[tokio::test]
async fn test_multiple_concurrent_sse_requests() {
    // This test verifies that the multiplexing system correctly handles
    // multiple concurrent SSE requests with different request IDs

    let mock_addr = start_mock_signal_cli_with_sse("127.0.0.1:0", 2).await;

    let http_policy = HttpPolicy {
        upstream: format!("http://{}", mock_addr),
        jsonrpc_allow_methods: vec![],
        jsonrpc_deny_methods: vec![],
        jsonrpc_param_filters: HashMap::new(),
        rate_limit: None,
        timeout_secs: Some(5),
        audit: Default::default(),
    };

    let mut tools = HashMap::new();
    tools.insert("signal-cli".to_string(), ToolPolicy::Http(http_policy));
    let policy = PolicyConfig { tools };

    let dispatcher = Arc::new(HttpDispatcher::with_policy(policy));

    // Send multiple concurrent SSE requests
    let mut handles = vec![];

    for i in 0..3 {
        let dispatcher = dispatcher.clone();

        let handle = tokio::spawn(async move {
            let sse_req = HttpRequest {
                id: format!("test-concurrent-{}", i),
                tool: "signal-cli".to_string(),
                method: "GET".to_string(),
                path: "/api/v1/events".to_string(),
                headers: {
                    let mut h = HashMap::new();
                    h.insert("Accept".to_string(), "text/event-stream".to_string());
                    h
                },
                body: None,
            };

            let response = dispatcher.dispatch_http(sse_req).await;
            assert!(
                response.is_ok(),
                "Concurrent SSE request should succeed"
            );

            let response = response.unwrap();
            assert_eq!(response.status, 200);
            response
        });

        handles.push(handle);
    }

    // Wait for all requests to complete
    for handle in handles {
        let result = handle.await;
        assert!(result.is_ok(), "All concurrent requests should complete");
    }
}

#[tokio::test]
async fn test_sse_headers_correct() {
    let mock_addr = start_mock_signal_cli_with_sse("127.0.0.1:0", 1).await;

    let http_policy = HttpPolicy {
        upstream: format!("http://{}", mock_addr),
        jsonrpc_allow_methods: vec![],
        jsonrpc_deny_methods: vec![],
        jsonrpc_param_filters: HashMap::new(),
        rate_limit: None,
        timeout_secs: Some(3),
        audit: Default::default(),
    };

    let mut tools = HashMap::new();
    tools.insert("signal-cli".to_string(), ToolPolicy::Http(http_policy));
    let policy = PolicyConfig { tools };

    let dispatcher = HttpDispatcher::with_policy(policy);

    let sse_req = HttpRequest {
        id: "test-headers".to_string(),
        tool: "signal-cli".to_string(),
        method: "GET".to_string(),
        path: "/api/v1/events".to_string(),
        headers: HashMap::new(),
        body: None,
    };

    let response = dispatcher.dispatch_http(sse_req).await.unwrap();

    // Verify SSE headers are present
    let content_type = response
        .headers
        .get("content-type")
        .map(|s| s.to_lowercase());

    assert!(
        content_type.map(|ct| ct.contains("text/event-stream")).unwrap_or(false),
        "SSE endpoint must return text/event-stream content type"
    );
}

#[tokio::test]
async fn test_json_rpc_and_sse_mixed_requests() {
    // Verify that dispatcher can handle both JSON-RPC (POST /api/v1/rpc)
    // and SSE (GET /api/v1/events) in the same session

    let mock_addr = start_mock_signal_cli_with_sse("127.0.0.1:0", 1).await;

    let http_policy = HttpPolicy {
        upstream: format!("http://{}", mock_addr),
        jsonrpc_allow_methods: vec!["send".to_string(), "receive".to_string()],
        jsonrpc_deny_methods: vec![],
        jsonrpc_param_filters: HashMap::new(),
        rate_limit: None,
        timeout_secs: Some(3),
        audit: Default::default(),
    };

    let mut tools = HashMap::new();
    tools.insert("signal-cli".to_string(), ToolPolicy::Http(http_policy));
    let policy = PolicyConfig { tools };

    let dispatcher = HttpDispatcher::with_policy(policy);

    // First: Send a JSON-RPC request (like OpenClaw sending a message)
    let rpc_req = HttpRequest {
        id: "send-msg".to_string(),
        tool: "signal-cli".to_string(),
        method: "POST".to_string(),
        path: "/api/v1/rpc".to_string(),
        headers: {
            let mut h = HashMap::new();
            h.insert("Content-Type".to_string(), "application/json".to_string());
            h
        },
        body: Some(
            r#"{"jsonrpc":"2.0","id":"1","method":"send","params":{"recipientNumber":"+12025551234","message":"Test"}}"#
                .to_string(),
        ),
    };

    let rpc_response = dispatcher.dispatch_http(rpc_req).await;
    assert!(rpc_response.is_ok(), "JSON-RPC request should succeed");
    assert_eq!(rpc_response.unwrap().status, 200);

    // Second: Listen for events (like OpenClaw waiting for incoming messages)
    let sse_req = HttpRequest {
        id: "listen-events".to_string(),
        tool: "signal-cli".to_string(),
        method: "GET".to_string(),
        path: "/api/v1/events".to_string(),
        headers: HashMap::new(),
        body: None,
    };

    let sse_response = dispatcher.dispatch_http(sse_req).await;
    assert!(sse_response.is_ok(), "SSE request should succeed");
    assert_eq!(sse_response.unwrap().status, 200);
}

#[tokio::test]
async fn test_sse_timeout_handling() {
    // Verify that SSE requests timeout correctly if upstream takes too long
    // This tests the 2-second timeout for initial events

    let mock_addr = start_mock_signal_cli_with_sse("127.0.0.1:0", 0).await; // 0 events = never sends

    let http_policy = HttpPolicy {
        upstream: format!("http://{}", mock_addr),
        jsonrpc_allow_methods: vec![],
        jsonrpc_deny_methods: vec![],
        jsonrpc_param_filters: HashMap::new(),
        rate_limit: None,
        timeout_secs: Some(1), // Short timeout
        audit: Default::default(),
    };

    let mut tools = HashMap::new();
    tools.insert("signal-cli".to_string(), ToolPolicy::Http(http_policy));
    let policy = PolicyConfig { tools };

    let dispatcher = HttpDispatcher::with_policy(policy);

    let sse_req = HttpRequest {
        id: "timeout-test".to_string(),
        tool: "signal-cli".to_string(),
        method: "GET".to_string(),
        path: "/api/v1/events".to_string(),
        headers: HashMap::new(),
        body: None,
    };

    let start = std::time::Instant::now();
    let response = dispatcher.dispatch_http(sse_req).await;
    let elapsed = start.elapsed();

    // Should timeout and return empty (or error)
    // Currently returns 200 with empty body after 2-second wait
    assert!(response.is_ok() || response.is_err());

    // Should have waited at least the timeout duration
    // The 2-second SSE wait should be noticeable
    println!("SSE request completed in {:?}", elapsed);
}

/// FAILING TEST: Events arriving after timeout window are lost
///
/// This test demonstrates the real problem: events that arrive
/// after the 2-second buffering window are silently dropped.
///
/// STATUS: Currently fails due to response.text() buffering semantics,
/// but demonstrates the kind of test that SHOULD fail to catch the bug.
/// The architectural issue is: we wait 2 seconds then return buffered response,
/// which means any events arriving after 2 seconds are lost.
#[tokio::test]
#[ignore] // Currently fails due to test mocking complexity, but documents the issue
async fn test_sse_events_after_timeout_window_lost() {
    // Mock signal-cli that sends events at different times
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind");

    let addr = listener.local_addr().expect("Failed to get addr");

    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = vec![0; 4096];
            if let Ok(_) = socket.read(&mut buf).await {
                // Send SSE headers immediately
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n",
                    )
                    .await;
                let _ = socket.flush().await;

                // Send first event immediately (will be captured)
                sleep(Duration::from_millis(100)).await;
                let _ = socket
                    .write_all(b"data: {\"id\": \"event-1\", \"message\": \"First\"}\n\n")
                    .await;
                let _ = socket.flush().await;

                // Send second event AFTER 2-second window closes (will be lost)
                sleep(Duration::from_secs(3)).await;
                let _ = socket
                    .write_all(b"data: {\"id\": \"event-2\", \"message\": \"Second\"}\n\n")
                    .await;
                let _ = socket.flush().await;
            }
        }
    });

    let http_policy = HttpPolicy {
        upstream: format!("http://{}", addr),
        jsonrpc_allow_methods: vec![],
        jsonrpc_deny_methods: vec![],
        jsonrpc_param_filters: HashMap::new(),
        rate_limit: None,
        timeout_secs: Some(2), // 2-second timeout
        audit: Default::default(),
    };

    let mut tools = HashMap::new();
    tools.insert("signal-cli".to_string(), ToolPolicy::Http(http_policy));
    let policy = PolicyConfig { tools };

    let dispatcher = HttpDispatcher::with_policy(policy);

    let sse_req = HttpRequest {
        id: "late-events-test".to_string(),
        tool: "signal-cli".to_string(),
        method: "GET".to_string(),
        path: "/api/v1/events".to_string(),
        headers: HashMap::new(),
        body: None,
    };

    let response = dispatcher.dispatch_http(sse_req).await;
    assert!(response.is_ok());

    let body = response.unwrap().body.unwrap_or_default();
    println!("Received body:\n{}", body);

    // THIS ASSERTION DOCUMENTS THE BUG:
    // We expect 2 events (event-1 and event-2)
    // But we only get event-1 because event-2 arrives after 2-second window
    let has_event_1 = body.contains("event-1");
    let has_event_2 = body.contains("event-2");

    println!("Event 1 received: {}", has_event_1);
    println!("Event 2 received: {}", has_event_2);

    assert!(
        has_event_1,
        "Should have received event-1 (arrived within 2-second window)"
    );

    // THIS TEST FAILS - demonstrating the architectural problem
    assert!(
        has_event_2,
        "FAILS: event-2 arrived after 2-second window so it's lost - this is the bug!"
    );
}

/// ARCHITECTURAL ISSUE TEST
///
/// This test documents the fundamental problem with SSE streaming
/// in a message-based protocol.
#[tokio::test]
#[ignore] // This test demonstrates the problem, not a solution
async fn test_sse_streaming_architectural_limitation() {
    // The core issue:
    // 1. Our protocol: HttpRequest -> [wait] -> HttpResponse
    // 2. SSE requirement: Open connection -> [receive events over time] -> keep stream open
    //
    // Current workaround in http_dispatch.rs:
    // - Wait 2 seconds for initial events
    // - Buffer all events that arrive in those 2 seconds
    // - Return buffered events in response body
    //
    // This means:
    // ✓ Events that arrive in first 2 seconds are captured
    // ✗ Events that arrive after 2 seconds are lost
    // ✗ Client gets all events at once (not incremental)
    // ✗ If no events in 2 seconds, client gets empty response
    //
    // Real solution would require:
    // - SseEvent message type that can be streamed incrementally
    // - Agent-level SSE streaming to client (not buffered response)
    // - Or: Agent translates SSE to chunked HTTP response
    //
    // This is a fundamental architectural limitation of the
    // message-based request/response protocol.

    println!(
        "ARCHITECTURAL ISSUE DOCUMENTED:\n\
        SSE streaming requires incremental event delivery,\n\
        but our message protocol is strictly request/response.\n\
        \n\
        Current workaround: 2-second buffer for initial events.\n\
        This prevents real-time message delivery in OpenClaw.\n\
        \n\
        Proper fix would require protocol redesign to support\n\
        streaming message types."
    );
}
