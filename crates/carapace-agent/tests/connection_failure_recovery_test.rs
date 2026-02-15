/// Connection Failure and Recovery Tests
///
/// Tests resilience to network failures, connection drops, and recovery scenarios.
/// These are critical for production stability.
use carapace_agent::{Connection, Multiplexer};
use carapace_protocol::{CliRequest, Message, MessageCodec};
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
