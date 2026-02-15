/// Test suite that mimics OpenClaw's actual SSE streaming behavior
///
/// This tests the complete flow:
/// 1. OpenClaw connects to agent's HTTP proxy at localhost:8080
/// 2. Agent forwards SSE request to server
/// 3. Server proxies to signal-cli's /api/v1/events endpoint
/// 4. Events arrive asynchronously from signal-cli
/// 5. Agent must stream events back to OpenClaw as they arrive
///
/// Current problem: Events are buffered for 2 seconds then returned,
/// not streamed incrementally. This breaks real-time message delivery.

use carapace_policy::{HttpPolicy, PolicyConfig, ToolPolicy};
use carapace_protocol::HttpRequest;
use carapace_server::http_dispatch::HttpDispatcher;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration, Instant};

/// Mock signal-cli that emits events one at a time with delays
/// This simulates real Signal messages arriving over time
async fn start_realistic_signal_cli(addr: &str) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind mock signal-cli");

    let local_addr = listener.local_addr().expect("Failed to get local addr");

    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0; 4096];
                if let Ok(n) = socket.read(&mut buf).await {
                    let request_str = String::from_utf8_lossy(&buf[..n]);

                    if request_str.contains("GET /api/v1/events") {
                        // Send SSE headers immediately
                        let sse_headers = "HTTP/1.1 200 OK\r\n\
                            Content-Type: text/event-stream\r\n\
                            Cache-Control: no-cache\r\n\
                            Connection: keep-alive\r\n\
                            \r\n";

                        let _ = socket.write_all(sse_headers.as_bytes()).await;
                        let _ = socket.flush().await;

                        // Emit events one at a time, as they would arrive in real Signal
                        // This simulates: message arrives -> server parses -> event emitted
                        let events = vec![
                            (100, "Message from Alice"),
                            (500, "Message from Bob"),
                            (1000, "Message from Charlie"),
                        ];

                        for (delay_ms, sender) in events {
                            sleep(Duration::from_millis(delay_ms)).await;

                            let timestamp = (1000 + delay_ms) as i64 * 1000;
                            let event = format!(
                                "data: {{\"type\":\"message\",\"sender\":\"{}\",\"timestamp\":{},\"message\":\"Test from {}\"}}\n\n",
                                sender.split_whitespace().next().unwrap_or("Unknown"),
                                timestamp,
                                sender
                            );

                            if socket.write_all(event.as_bytes()).await.is_err() {
                                break;
                            }
                            let _ = socket.flush().await;
                        }
                    } else if request_str.contains("POST /api/v1/rpc") {
                        let response_body =
                            r#"{"jsonrpc":"2.0","result":{"status":"sent"},"id":"1"}"#;
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
async fn test_openclaw_incoming_message_pattern() {
    // This test reproduces the actual OpenClaw scenario:
    // 1. User wants to receive Signal messages
    // 2. OpenClaw connects to /api/v1/events for SSE stream
    // 3. Three Signal messages arrive at different times (100ms, 500ms, 1000ms apart)
    // 4. OpenClaw should receive each message as it arrives

    let signal_cli_addr = start_realistic_signal_cli("127.0.0.1:0").await;

    let http_policy = HttpPolicy {
        upstream: format!("http://{}", signal_cli_addr),
        jsonrpc_allow_methods: vec!["send".to_string(), "receive".to_string()],
        jsonrpc_deny_methods: vec![],
        jsonrpc_param_filters: HashMap::new(),
        rate_limit: None,
        timeout_secs: Some(5),
        audit: Default::default(),
    };

    let mut tools = HashMap::new();
    tools.insert("signal-cli".to_string(), ToolPolicy::Http(http_policy));
    let policy = PolicyConfig { tools };

    let dispatcher = HttpDispatcher::with_policy(policy);

    // This is what OpenClaw does to listen for incoming messages
    let start = Instant::now();
    let sse_req = HttpRequest {
        id: "openclaw-listen".to_string(),
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
    let elapsed = start.elapsed();

    println!("SSE response completed in {:?}", elapsed);

    assert!(response.is_ok(), "SSE request should succeed");
    let response = response.unwrap();

    let body = response.body.unwrap_or_default();
    println!("Received {} bytes from SSE endpoint", body.len());
    println!("Response body:\n{}", body);

    // PROBLEM ANALYSIS:
    // Current implementation waits 2 seconds for events.
    // Events arrive at: 100ms, 500ms, 1000ms
    // So all 3 events should be buffered in those 2 seconds.
    //
    // But here's the real issue: In production, signal-cli's SSE endpoint
    // doesn't emit events on a timer - it emits them when Signal messages arrive.
    // If we wait 2 seconds and no messages arrive, we get empty response.
    // OpenClaw polls again -> gets empty response -> keeps polling.
    //
    // The latency and batching make the experience feel broken:
    // - Message arrives at signal-cli: 0ms
    // - Takes up to 2 seconds to return to OpenClaw
    // - User sees message appear with 2-second delay (not real-time)

    assert!(
        body.contains("data:"),
        "Should have received events (testing if events are batched)"
    );

    // Count events in response
    let event_count = body.lines().filter(|line| line.contains("\"type\":\"message\"")).count();
    println!("Number of events in response: {}", event_count);

    // CRITICAL: Should have 3 events buffered in 2-second window
    assert_eq!(
        event_count, 3,
        "Should have buffered all 3 events that arrived within 2-second timeout"
    );
}

#[tokio::test]
async fn test_openclaw_send_then_listen_pattern() {
    // This test simulates OpenClaw's complete workflow:
    // 1. User sends a Signal message (POST to /api/v1/rpc)
    // 2. OpenClaw listens for incoming messages (GET /api/v1/events)
    // 3. Expects to see the sent message echoed back in events

    let signal_cli_addr = start_realistic_signal_cli("127.0.0.1:0").await;

    let http_policy = HttpPolicy {
        upstream: format!("http://{}", signal_cli_addr),
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

    let dispatcher = Arc::new(HttpDispatcher::with_policy(policy));

    // Step 1: Send a Signal message
    let send_req = HttpRequest {
        id: "send-msg-1".to_string(),
        tool: "signal-cli".to_string(),
        method: "POST".to_string(),
        path: "/api/v1/rpc".to_string(),
        headers: {
            let mut h = HashMap::new();
            h.insert("Content-Type".to_string(), "application/json".to_string());
            h
        },
        body: Some(
            r#"{"jsonrpc":"2.0","id":"1","method":"send","params":{"recipientNumber":"+12025551234","message":"Hello from OpenClaw"}}"#
                .to_string(),
        ),
    };

    let send_response = dispatcher.dispatch_http(send_req).await;
    assert!(send_response.is_ok(), "Send request should succeed");
    println!("Message sent successfully");

    // Small delay to simulate user opening the events stream after sending
    sleep(Duration::from_millis(50)).await;

    // Step 2: Listen for incoming events
    let listen_req = HttpRequest {
        id: "listen-events".to_string(),
        tool: "signal-cli".to_string(),
        method: "GET".to_string(),
        path: "/api/v1/events".to_string(),
        headers: HashMap::new(),
        body: None,
    };

    let start = Instant::now();
    let listen_response = dispatcher.dispatch_http(listen_req).await;
    let elapsed = start.elapsed();

    assert!(listen_response.is_ok(), "Listen request should succeed");

    let response = listen_response.unwrap();
    let body = response.body.unwrap_or_default();

    println!(
        "Received events after {:?}:\n{}",
        elapsed, body
    );

    assert!(
        !body.is_empty(),
        "Should have received some events (may be empty due to 2-second buffer)"
    );
}

/// Documents the fundamental problem with OpenClaw + Carapace + signal-cli
#[tokio::test]
#[ignore]
async fn test_openclaw_message_latency_issue_documented() {
    // THE REAL PROBLEM:
    //
    // When a Signal message arrives at signal-cli, here's what happens:
    //
    // signal-cli receives message from Signal network (0ms)
    //   ↓
    // signal-cli emits SSE event on /api/v1/events stream (1ms)
    //   ↓
    // Carapace server's http_dispatch waits for event
    //   - Currently waits MAX 2 seconds for initial event
    //   - Buffers all events that arrive in that window
    //   ↓
    // Server returns HttpResponse with buffered events (after 2s or when timeout expires)
    //   ↓
    // Agent receives response and forwards to OpenClaw (network latency ~50-100ms)
    //   ↓
    // OpenClaw receives message (now 2+ seconds after arrival)
    //   ↓
    // User sees the message with 2-second delay
    //
    // THE ARCHITECTURAL ISSUE:
    // - Our message protocol: REQUEST → [wait] → RESPONSE
    // - This is incompatible with true SSE streaming
    // - Real SSE: connection stays open, events stream as they arrive
    //
    // CURRENT WORKAROUND:
    // - Wait up to 2 seconds for events to batch
    // - Return all batched events at once
    // - This reduces latency but breaks real-time delivery
    //
    // PROPER SOLUTION WOULD REQUIRE:
    // 1. Protocol-level streaming support (SseEvent message type)
    // 2. Agent translating SSE to chunked HTTP response to client
    // 3. Or: Keep server connection open indefinitely (new protocol)
    //
    // OPENCLAW EXPERIENCE:
    // ✓ Messages do eventually arrive (mostly)
    // ✗ With 2+ second delay
    // ✗ Batched in groups
    // ✗ No indication to user of delivery delay
    //
    // The user experience is: "Messages are being batched and delayed"

    println!(
        "OPENCLAW LATENCY ISSUE DOCUMENTED:\n\
        \n\
        Messages are delayed because:\n\
        1. SSE requires streaming, we buffer for 2 seconds\n\
        2. Message arrives → server waits up to 2s → returns buffered batch\n\
        3. Total latency: 2s + network + processing\n\
        \n\
        User reports: 'Messages I'm sending... just don't show up' or\n\
        'incoming messages aren't working' or\n\
        'messages are being batched and delayed'\n\
        \n\
        This is the architectural limitation of message-based protocol\n\
        trying to support streaming endpoints."
    );
}
