/// SSE Streaming Latency Tests
///
/// Verifies that SSE events are streamed to the HTTP client in real-time,
/// not buffered until the stream ends.
use carapace_agent::{Connection, HttpProxy, Multiplexer};
use carapace_protocol::{Message, MessageCodec, SseEvent};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio_util::codec::{FramedRead, FramedWrite};

/// Test that SSE events arrive at the HTTP client incrementally (not buffered).
///
/// The mock server sends 3 SseEvent messages with 200ms gaps.
/// If streaming works: first event arrives within ~200ms.
/// If buffered: all events arrive together after the stream ends (300s timeout).
#[tokio::test]
async fn test_sse_events_are_streamed_not_buffered() {
    // Start mock carapace-server
    let server_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_port = server_listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        let (socket, _) = server_listener.accept().await.unwrap();
        let (reader, writer) = socket.into_split();
        let mut frame_read = FramedRead::new(reader, MessageCodec);
        let mut frame_write = FramedWrite::new(writer, MessageCodec);

        while let Some(Ok(msg)) = frame_read.next().await {
            match msg {
                Message::HttpRequest(req) if req.path.contains("/events") => {
                    // Send 3 SSE events with 200ms gaps
                    for i in 0..3 {
                        let evt = Message::SseEvent(SseEvent {
                            id: req.id.clone(),
                            tool: "signal-cli".to_string(),
                            event: "message".to_string(),
                            data: format!(r#"{{"seq":{}}}"#, i),
                        });
                        frame_write.send(evt).await.unwrap();
                        frame_write.flush().await.unwrap();
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                    // Close connection after sending events (ends the stream)
                    break;
                }
                Message::Ping(p) => {
                    let _ = frame_write.send(Message::Pong(p)).await;
                    let _ = frame_write.flush().await;
                }
                _ => {}
            }
        }
    });

    // Connect agent to mock server
    let connection = Arc::new(
        Connection::connect_tcp_with_config("127.0.0.1", server_port, 3, 100)
            .await
            .unwrap(),
    );
    let multiplexer = Arc::new(Multiplexer::new());

    // Spawn recv loop
    let conn_read = connection.clone();
    let mux_read = multiplexer.clone();
    tokio::spawn(async move {
        while let Ok(Some(msg)) = conn_read.recv().await {
            if !matches!(&msg, Message::Pong(_)) {
                mux_read.handle_response(msg).await;
            }
        }
    });

    // Start HTTP proxy on a random port
    let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http_port = proxy_listener.local_addr().unwrap().port();
    drop(proxy_listener);

    let proxy = HttpProxy::new(multiplexer.clone(), connection.clone(), http_port);
    tokio::spawn(async move {
        proxy.listen().await.unwrap();
    });
    // Give axum time to start listening
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Make HTTP GET to /api/v1/events and measure when events arrive
    let client = reqwest::Client::new();
    let start = Instant::now();

    let resp = client
        .get(format!("http://127.0.0.1:{}/api/v1/events", http_port))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "text/event-stream"
    );

    // Read the response as a byte stream to detect individual chunks
    let mut event_times = vec![];
    let mut stream = resp.bytes_stream();
    let mut accumulated = String::new();

    while let Ok(Some(chunk)) = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .map(|opt| opt.map(|r| r.unwrap()))
    {
        let text = String::from_utf8_lossy(&chunk);
        accumulated.push_str(&text);

        // Count how many complete events we've received so far
        let event_count = accumulated.matches("\"seq\"").count();
        while event_times.len() < event_count {
            event_times.push(start.elapsed());
        }

        if event_times.len() >= 3 {
            break;
        }
    }

    assert!(
        event_times.len() >= 3,
        "Should receive 3 events, got {}",
        event_times.len()
    );

    // Key assertion: first event should arrive quickly (within 1s),
    // NOT after the full 300s SSE timeout.
    // With streaming: first event arrives in ~200ms (after server sends it)
    // With buffering: first event wouldn't arrive until 300s timeout or stream close
    let first_event_time = event_times[0];
    assert!(
        first_event_time < Duration::from_secs(2),
        "First SSE event should arrive within 2s (streaming), took {:?} (likely buffered)",
        first_event_time
    );

    // Events should arrive incrementally, not all at once
    if event_times.len() >= 2 {
        let gap = event_times[1] - event_times[0];
        // Gap should be ~200ms (the server delay). If all arrive at once, gap is ~0.
        assert!(
            gap > Duration::from_millis(50),
            "Events should arrive incrementally (gap={:?}), not all at once",
            gap
        );
    }
}
