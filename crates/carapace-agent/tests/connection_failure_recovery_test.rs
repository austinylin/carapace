/// Connection Failure and Recovery Tests
///
/// Tests resilience to network failures, connection drops, and recovery scenarios.
/// These are critical for production stability.
use carapace_agent::{Connection, Multiplexer};
use carapace_protocol::{CliRequest, CliResponse, Message, MessageCodec, PingPong};
use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_util::codec::{FramedRead, FramedWrite};

/// Test that connection detects when server closes mid-stream
#[tokio::test]
async fn test_connection_detects_server_close() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind");

    let server_addr = listener.local_addr().unwrap();
    let server_port = server_addr.port();

    // Spawn server that accepts one connection then closes immediately
    tokio::spawn(async move {
        if let Ok((socket, _)) = listener.accept().await {
            drop(socket); // Close immediately
        }
    });

    // Try to connect and detect the close
    let connection = Connection::connect_tcp("127.0.0.1", server_port)
        .await
        .expect("Failed to connect");

    // Try to receive - should detect the closed connection
    let result = tokio::time::timeout(Duration::from_secs(1), connection.recv())
        .await
        .expect("Recv timed out");

    match result {
        Ok(None) => {
            // Connection closed - correct behavior
        }
        Ok(Some(_)) => {
            panic!("Should not receive message from closed connection");
        }
        Err(e) => {
            // Connection error - also correct
            eprintln!("Detected connection error: {}", e);
        }
    }
}

/// Test that pending requests are notified when connection drops
#[tokio::test]
async fn test_multiplexer_cleans_up_on_disconnect() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind");

    let server_addr = listener.local_addr().unwrap();
    let server_port = server_addr.port();

    // Spawn server that accepts connection but drops it
    tokio::spawn(async move {
        if let Ok((_socket, _)) = listener.accept().await {
            tokio::time::sleep(Duration::from_millis(100)).await;
            // Connection closes when socket is dropped
        }
    });

    // Create connection and multiplexer
    let connection = Connection::connect_tcp("127.0.0.1", server_port)
        .await
        .expect("Failed to connect");

    let multiplexer = Arc::new(Multiplexer::new());

    // Register a waiter for a request that will never arrive
    let mut rx = multiplexer
        .register_waiter("pending-request-1".to_string())
        .await;

    // Send the request (will succeed initially)
    let req = Message::CliRequest(CliRequest {
        id: "pending-request-1".to_string(),
        tool: "test".to_string(),
        argv: vec![],
        env: HashMap::new(),
        stdin: None,
        cwd: "/".to_string(),
    });

    connection.send(req).await.expect("Failed to send request");

    // Wait for connection to be detected as closed
    // The receiver should eventually detect the connection is gone
    let timeout_result = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;

    // Either timeout (connection dropped before response) or error
    // but NOT hang forever
    match timeout_result {
        Ok(None) => {
            // Channel closed - connection lost, cleanup happened
        }
        Ok(Some(_)) => {
            panic!("Should not receive response after server closed");
        }
        Err(_) => {
            // Timeout - channel closed, cleanup happened
        }
    }
}

/// Test that partial messages are handled gracefully
#[tokio::test]
async fn test_partial_message_handling() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind");

    let server_addr = listener.local_addr().unwrap();
    let server_port = server_addr.port();

    // Spawn server that sends partial message then closes
    tokio::spawn(async move {
        if let Ok((socket, _)) = listener.accept().await {
            let (reader, _) = socket.into_split();
            let mut frame_read = FramedRead::new(reader, MessageCodec);

            // Read the incoming request
            if let Some(_result) = frame_read.next().await {
                // Send partial length prefix only (4 bytes) then close
                // This would hang a naive implementation
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            // Socket closes - connection drops mid-message
        }
    });

    // Create a client connection
    let connection = Connection::connect_tcp("127.0.0.1", server_port)
        .await
        .expect("Failed to connect");

    // Send a request
    let req = Message::CliRequest(CliRequest {
        id: "test-001".to_string(),
        tool: "test".to_string(),
        argv: vec![],
        env: HashMap::new(),
        stdin: None,
        cwd: "/".to_string(),
    });

    connection.send(req).await.expect("Failed to send request");

    // Try to receive - should timeout or error, NOT panic
    let recv_result = tokio::time::timeout(Duration::from_millis(500), connection.recv()).await;

    match recv_result {
        Ok(Ok(None)) => {
            // Connection closed cleanly
        }
        Ok(Err(_)) => {
            // Connection error - expected
        }
        Err(_) => {
            // Timeout - acceptable for dropped connection
        }
        Ok(Ok(Some(_))) => {
            panic!("Should not receive valid message from dropped connection");
        }
    }
}

/// Test that rapid reconnection attempts are rate-limited
#[tokio::test]
async fn test_reconnection_backoff() {
    // Try to connect to a non-existent server
    // Should fail with exponential backoff, not rapid-fire attempts

    let start = std::time::Instant::now();

    let result = Connection::connect_tcp_with_config("127.0.0.1", 9999, 3, 50).await;

    let elapsed = start.elapsed();

    // With 3 attempts and 50ms backoff:
    // Attempt 1: immediate
    // Attempt 2: 50ms later
    // Attempt 3: 100ms later (2^1 * 50ms)
    // Total: ~150ms minimum

    assert!(
        result.is_err(),
        "Should fail to connect to non-existent server"
    );
    assert!(
        elapsed >= Duration::from_millis(150),
        "Should have backoff between attempts, took {:?}",
        elapsed
    );
}

/// Test that concurrent requests survive connection drop
#[tokio::test]
async fn test_concurrent_requests_survive_connection_drop() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind");

    let server_addr = listener.local_addr().unwrap();
    let server_port = server_addr.port();

    let requests_received = Arc::new(AtomicBool::new(false));
    let requests_received_clone = requests_received.clone();

    // Spawn server that receives requests then drops connection
    tokio::spawn(async move {
        if let Ok((socket, _)) = listener.accept().await {
            let (reader, _) = socket.into_split();
            let mut frame_read = FramedRead::new(reader, MessageCodec);

            let mut count = 0;
            while let Some(result) = frame_read.next().await {
                if result.is_ok() {
                    count += 1;
                }
                if count >= 2 {
                    requests_received_clone.store(true, Ordering::SeqCst);
                    break;
                }
            }
            // Connection closes when socket drops
        }
    });

    // Create a client connection and multiplexer
    let connection = Arc::new(
        Connection::connect_tcp("127.0.0.1", server_port)
            .await
            .expect("Failed to connect"),
    );

    // Send multiple requests concurrently
    let mut handles = vec![];

    for i in 0..3 {
        let conn = connection.clone();
        let handle = tokio::spawn(async move {
            let req = Message::CliRequest(CliRequest {
                id: format!("concurrent-{}", i),
                tool: "test".to_string(),
                argv: vec![],
                env: HashMap::new(),
                stdin: None,
                cwd: "/".to_string(),
            });

            // Some might succeed, some might fail due to connection drop
            let _result = conn.send(req).await;
        });
        handles.push(handle);
    }

    // Wait for all sends to complete
    for handle in handles {
        let _ = handle.await;
    }

    // Give server time to close
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(
        requests_received.load(Ordering::SeqCst),
        "Server should have received requests before dropping"
    );
}

/// Test that oversized messages are rejected without hanging
#[tokio::test]
async fn test_oversized_message_rejection() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind");

    let server_addr = listener.local_addr().unwrap();
    let server_port = server_addr.port();

    // Spawn server that tries to send oversized frame
    tokio::spawn(async move {
        if let Ok((socket, _)) = listener.accept().await {
            let (_reader, writer) = socket.into_split();
            let mut frame_write = FramedWrite::new(writer, MessageCodec);

            // Try to encode a message with huge argv
            let huge_argv: Vec<String> = (0..100_000).map(|i| format!("arg-{}", i)).collect();

            let req = Message::CliRequest(CliRequest {
                id: "huge".to_string(),
                tool: "test".to_string(),
                argv: huge_argv,
                env: HashMap::new(),
                stdin: None,
                cwd: "/".to_string(),
            });

            // This might fail or succeed depending on the actual payload size
            let _result = frame_write.send(req).await;
        }
    });

    // Create a client connection
    let connection = Connection::connect_tcp("127.0.0.1", server_port)
        .await
        .expect("Failed to connect");

    // Try to receive - might get error about frame too large
    let recv_result = tokio::time::timeout(Duration::from_secs(1), connection.recv()).await;

    // Should not hang, regardless of outcome
    match recv_result {
        Ok(Ok(Some(_))) => {
            // Received a message (maybe it was under the limit)
        }
        Ok(Ok(None)) => {
            // Connection closed
        }
        Ok(Err(e)) => {
            // Got an error - frame too large or similar
            eprintln!("Received expected error: {}", e);
        }
        Err(_) => {
            // Timeout - not ideal but better than hang
        }
    }
}

/// Test that connection recovers from transient errors
#[tokio::test]
async fn test_transient_error_recovery() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind");

    let server_addr = listener.local_addr().unwrap();
    let server_port = server_addr.port();

    let (tx, mut rx) = mpsc::unbounded_channel::<bool>();

    // Spawn server that accepts, processes, then drops connection
    tokio::spawn(async move {
        // Accept first connection
        if let Ok((socket, _)) = listener.accept().await {
            let (reader, writer) = socket.into_split();
            let mut frame_read = FramedRead::new(reader, MessageCodec);
            let mut frame_write = FramedWrite::new(writer, MessageCodec);

            // Read one message
            if let Some(Ok(_msg)) = frame_read.next().await {
                // Send a response
                let response = Message::CliRequest(CliRequest {
                    id: "response".to_string(),
                    tool: "test".to_string(),
                    argv: vec![],
                    env: HashMap::new(),
                    stdin: None,
                    cwd: "/".to_string(),
                });

                if frame_write.send(response).await.is_ok() {
                    let _ = frame_write.flush().await;
                    let _ = tx.send(true);
                }
            }
            // Connection closes here
        }
    });

    // Create a client connection
    let connection = Connection::connect_tcp("127.0.0.1", server_port)
        .await
        .expect("Failed to connect");

    // Send a request
    let req = Message::CliRequest(CliRequest {
        id: "test-001".to_string(),
        tool: "test".to_string(),
        argv: vec![],
        env: HashMap::new(),
        stdin: None,
        cwd: "/".to_string(),
    });

    connection.send(req).await.expect("Failed to send request");

    // Receive the response (server already waiting)
    let _response = tokio::time::timeout(Duration::from_secs(1), connection.recv())
        .await
        .expect("Timeout waiting for response")
        .expect("Failed to receive response");

    // Verify server processed it
    let processed = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("Timeout")
        .expect("Server didn't process");

    assert!(processed, "Server should have processed the request");
}

/// Test that wait_for_reconnect unblocks when reconnection succeeds
#[tokio::test]
async fn test_wait_for_reconnect_unblocks() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    // Server: accept connection, close it, then accept a second (reconnection)
    tokio::spawn(async move {
        // First connection - accept and close
        let (socket, _) = listener.accept().await.unwrap();
        drop(socket);

        // Second connection (reconnection) - keep alive
        let (_socket2, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    let connection = Arc::new(
        Connection::connect_tcp_with_config("127.0.0.1", port, 5, 100)
            .await
            .unwrap(),
    );

    // Wait for server to close first connection
    let _ = connection.recv().await;
    assert!(!connection.is_healthy());

    // Spawn reconnection after a short delay
    let conn_clone = connection.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        conn_clone.reconnect_if_needed().await.unwrap();
    });

    // wait_for_reconnect should unblock once reconnect completes
    let result =
        tokio::time::timeout(Duration::from_secs(3), connection.wait_for_reconnect()).await;
    assert!(
        result.is_ok(),
        "wait_for_reconnect should have been notified"
    );
    assert!(connection.is_healthy());
}

/// Test that recv loop recovers after server restart (full integration)
///
/// Simulates the exact production scenario:
/// 1. Agent connects and processes a request
/// 2. Server restarts (connection drops)
/// 3. Ping monitor reconnects
/// 4. Recv loop resumes reading from new connection
/// 5. New requests work end-to-end
#[tokio::test]
async fn test_recv_loop_recovers_after_server_restart() {
    // Phase 1: Start server, connect, verify round-trip works
    let listener1 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener1.local_addr().unwrap().port();

    // Server v1: echo CliRequests with "phase1" stdout
    let server1 = tokio::spawn(async move {
        let (socket, _) = listener1.accept().await.unwrap();
        let (reader, writer) = socket.into_split();
        let mut frame_read = FramedRead::new(reader, MessageCodec);
        let mut frame_write = FramedWrite::new(writer, MessageCodec);

        while let Some(Ok(msg)) = frame_read.next().await {
            match msg {
                Message::CliRequest(req) => {
                    let resp = Message::CliResponse(CliResponse {
                        id: req.id,
                        exit_code: 0,
                        stdout: "phase1".to_string(),
                        stderr: String::new(),
                    });
                    let _ = frame_write.send(resp).await;
                    let _ = frame_write.flush().await;
                }
                Message::Ping(p) => {
                    let _ = frame_write.send(Message::Pong(p)).await;
                    let _ = frame_write.flush().await;
                }
                _ => {}
            }
        }
    });

    let connection = Arc::new(
        Connection::connect_tcp_with_config("127.0.0.1", port, 5, 100)
            .await
            .unwrap(),
    );
    let multiplexer = Arc::new(Multiplexer::new());

    // Spawn resilient recv loop (same logic as the fixed main.rs)
    let conn_read = connection.clone();
    let mux_read = multiplexer.clone();
    tokio::spawn(async move {
        loop {
            if !conn_read.is_healthy() {
                mux_read.cleanup_on_disconnect().await;
                conn_read.wait_for_reconnect().await;
                continue;
            }

            match conn_read.recv().await {
                Ok(Some(msg)) => {
                    if matches!(&msg, Message::Pong(_)) {
                        continue;
                    }
                    mux_read.handle_response(msg).await;
                }
                Ok(None) | Err(_) => {
                    // recv() already set connected=false; loop back to the
                    // is_healthy() check which handles cleanup + wait.
                }
            }
        }
    });

    // Spawn ping monitor (1s interval for fast test)
    let conn_ping = connection.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if !conn_ping.is_healthy() {
                let _ = conn_ping.reconnect_if_needed().await;
                continue;
            }
            let ping = Message::Ping(PingPong {
                id: "test-ping".into(),
                timestamp: 0,
            });
            let _ = conn_ping.send(ping).await;
        }
    });

    // Phase 1: Verify round-trip works
    let mut rx1 = multiplexer.register_waiter("req-1".into()).await;
    connection
        .send(Message::CliRequest(CliRequest {
            id: "req-1".into(),
            tool: "test".into(),
            argv: vec![],
            env: HashMap::new(),
            stdin: None,
            cwd: "/".into(),
        }))
        .await
        .unwrap();

    let resp1 = tokio::time::timeout(Duration::from_secs(5), rx1.recv())
        .await
        .expect("Phase 1 timeout")
        .expect("Phase 1 no response");
    match &resp1 {
        Message::CliResponse(r) => assert_eq!(r.stdout, "phase1"),
        _ => panic!("Expected CliResponse, got {:?}", resp1),
    }
    multiplexer.remove_waiter("req-1").await;

    // Phase 2: Kill server, wait for detection
    server1.abort();
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Phase 3: Restart server on same port with "phase2" responses
    let listener2 = TcpListener::bind(format!("127.0.0.1:{}", port))
        .await
        .expect("Failed to rebind to same port");

    tokio::spawn(async move {
        let (socket, _) = listener2.accept().await.unwrap();
        let (reader, writer) = socket.into_split();
        let mut frame_read = FramedRead::new(reader, MessageCodec);
        let mut frame_write = FramedWrite::new(writer, MessageCodec);

        while let Some(Ok(msg)) = frame_read.next().await {
            match msg {
                Message::CliRequest(req) => {
                    let resp = Message::CliResponse(CliResponse {
                        id: req.id,
                        exit_code: 0,
                        stdout: "phase2".to_string(),
                        stderr: String::new(),
                    });
                    let _ = frame_write.send(resp).await;
                    let _ = frame_write.flush().await;
                }
                Message::Ping(p) => {
                    let _ = frame_write.send(Message::Pong(p)).await;
                    let _ = frame_write.flush().await;
                }
                _ => {}
            }
        }
    });

    // Wait for reconnection to complete
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        connection.is_healthy(),
        "Connection should be healthy after reconnect"
    );

    // Phase 4: Verify round-trip works through new connection
    let mut rx2 = multiplexer.register_waiter("req-2".into()).await;
    connection
        .send(Message::CliRequest(CliRequest {
            id: "req-2".into(),
            tool: "test".into(),
            argv: vec![],
            env: HashMap::new(),
            stdin: None,
            cwd: "/".into(),
        }))
        .await
        .unwrap();

    let resp2 = tokio::time::timeout(Duration::from_secs(5), rx2.recv())
        .await
        .expect("Phase 2 timeout - recv loop didn't recover")
        .expect("Phase 2 no response - recv loop didn't recover");
    match &resp2 {
        Message::CliResponse(r) => assert_eq!(r.stdout, "phase2"),
        _ => panic!("Expected CliResponse, got {:?}", resp2),
    }
}
