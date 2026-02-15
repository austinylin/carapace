/// TCP Message Framing Test
///
/// This test ensures that messages sent via Connection::send() are properly
/// framed with a 4-byte length prefix and flushed to the socket.
///
/// Bug that this test catches:
/// - Missing flush() after send() causes buffered data to stay in the kernel buffer
/// - Server reads garbage/partial data because the frame is never flushed
/// - Symptoms: "Frame too large: 1195725856 bytes" (0x47534F4E = "GSON")
use carapace_agent::Connection;
use carapace_protocol::{CliRequest, HttpRequest, Message, MessageCodec};
use futures::StreamExt;
use std::collections::HashMap;
use tokio::net::TcpListener;
use tokio_util::codec::FramedRead;

/// Test that Connection::send() properly frames messages
#[tokio::test]
async fn test_connection_send_flushes_to_socket() {
    // Start a mock server that receives and validates framed messages
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind mock server");

    let server_addr = listener.local_addr().unwrap();
    let server_port = server_addr.port();

    // Spawn server task to receive and validate the message
    let server_handle = tokio::spawn(async move {
        let (socket, _) = listener
            .accept()
            .await
            .expect("Failed to accept connection");
        let (reader, _) = socket.into_split();
        let mut frame_read = FramedRead::new(reader, MessageCodec);

        // Read the first message
        let msg = frame_read
            .next()
            .await
            .expect("No message received")
            .expect("Failed to deserialize message");

        // Verify we got the right message
        match msg {
            Message::CliRequest(req) => {
                assert_eq!(req.tool, "test-tool");
                assert_eq!(req.argv[0], "test-arg");
            }
            _ => panic!("Expected CliRequest, got {:?}", msg),
        }
    });

    // Create a client connection
    let connection = Connection::connect_tcp("127.0.0.1", server_port)
        .await
        .expect("Failed to connect");

    // Send a message (this is where the bug would manifest)
    let test_request = Message::CliRequest(CliRequest {
        id: "test-001".to_string(),
        tool: "test-tool".to_string(),
        argv: vec!["test-arg".to_string()],
        env: HashMap::new(),
        stdin: None,
        cwd: "/tmp".to_string(),
    });

    // This should NOT hang or panic
    connection
        .send(test_request)
        .await
        .expect("Failed to send message - this indicates the flush() is missing!");

    // Wait for server to receive and validate the message
    server_handle.await.expect("Server task failed");
}

/// Test that multiple messages are properly framed
#[tokio::test]
async fn test_connection_send_multiple_messages() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind mock server");

    let server_addr = listener.local_addr().unwrap();
    let server_port = server_addr.port();

    // Spawn server task to receive multiple messages
    let server_handle = tokio::spawn(async move {
        let (socket, _) = listener
            .accept()
            .await
            .expect("Failed to accept connection");
        let (reader, _) = socket.into_split();
        let mut frame_read = FramedRead::new(reader, MessageCodec);

        // Read three messages
        for i in 0..3 {
            let msg = frame_read
                .next()
                .await
                .expect(&format!("Message {} not received", i))
                .expect(&format!("Failed to deserialize message {}", i));

            match msg {
                Message::CliRequest(req) => {
                    assert_eq!(
                        req.id,
                        format!("test-{:03}", i),
                        "Message {} has wrong ID",
                        i
                    );
                }
                _ => panic!("Expected CliRequest for message {}", i),
            }
        }
    });

    // Create a client connection
    let connection = Connection::connect_tcp("127.0.0.1", server_port)
        .await
        .expect("Failed to connect");

    // Send multiple messages in rapid succession
    // If flush() is missing, only the last message might be received
    for i in 0..3 {
        let req = Message::CliRequest(CliRequest {
            id: format!("test-{:03}", i),
            tool: format!("tool-{}", i),
            argv: vec![],
            env: HashMap::new(),
            stdin: None,
            cwd: "/".to_string(),
        });

        connection.send(req).await.expect(&format!(
            "Failed to send message {} - missing flush() would cause this",
            i
        ));
    }

    // Wait for server to receive all messages
    server_handle.await.expect("Server task failed");
}

/// Test that HttpRequest messages are properly framed
///
/// This is the specific message type that was failing in production
#[tokio::test]
async fn test_connection_send_http_request() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind mock server");

    let server_addr = listener.local_addr().unwrap();
    let server_port = server_addr.port();

    // Spawn server task
    let server_handle = tokio::spawn(async move {
        let (socket, _) = listener
            .accept()
            .await
            .expect("Failed to accept connection");
        let (reader, _) = socket.into_split();
        let mut frame_read = FramedRead::new(reader, MessageCodec);

        let msg = frame_read
            .next()
            .await
            .expect("No message received")
            .expect("Failed to deserialize message");

        match msg {
            Message::HttpRequest(http_req) => {
                assert_eq!(http_req.tool, "signal-cli");
                assert_eq!(http_req.method, "GET");
                assert_eq!(http_req.path, "/api/v1/events");
            }
            _ => {
                panic!(
                    "Expected HttpRequest, got: {:?}. \
                    If you see 'Frame too large: 1195725856 bytes', \
                    the flush() is missing from Connection::send()",
                    msg
                );
            }
        }
    });

    // Create a client connection
    let connection = Connection::connect_tcp("127.0.0.1", server_port)
        .await
        .expect("Failed to connect");

    // Send the HttpRequest that was failing in production
    let http_req = Message::HttpRequest(HttpRequest {
        id: "sse-test".to_string(),
        tool: "signal-cli".to_string(),
        method: "GET".to_string(),
        path: "/api/v1/events".to_string(),
        headers: HashMap::new(),
        body: None,
    });

    connection
        .send(http_req)
        .await
        .expect("Failed to send HttpRequest - if flush() is missing, this will fail!");

    server_handle.await.expect("Server task failed");
}

/// Test that rapid fire messages don't overwhelm the buffer
///
/// Without proper flushing, rapid messages could get stuck in the kernel buffer
#[tokio::test]
async fn test_connection_send_rapid_messages() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind mock server");

    let server_addr = listener.local_addr().unwrap();
    let server_port = server_addr.port();

    let message_count = 10;

    // Spawn server task
    let server_handle = tokio::spawn(async move {
        let (socket, _) = listener
            .accept()
            .await
            .expect("Failed to accept connection");
        let (reader, _) = socket.into_split();
        let mut frame_read = FramedRead::new(reader, MessageCodec);

        let mut received_count = 0;
        while let Some(result) = frame_read.next().await {
            match result {
                Ok(_msg) => {
                    received_count += 1;
                }
                Err(e) => {
                    panic!(
                        "Error reading message {}: {}. \
                        This likely means a message wasn't properly flushed",
                        received_count, e
                    );
                }
            }

            if received_count >= message_count {
                break;
            }
        }

        assert_eq!(
            received_count, message_count,
            "Expected {} messages, got {}. Missing flush() would cause this!",
            message_count, received_count
        );
    });

    // Create a client connection
    let connection = Connection::connect_tcp("127.0.0.1", server_port)
        .await
        .expect("Failed to connect");

    // Send many messages rapidly
    for i in 0..message_count {
        let req = Message::CliRequest(CliRequest {
            id: format!("rapid-{}", i),
            tool: "test".to_string(),
            argv: vec![],
            env: HashMap::new(),
            stdin: None,
            cwd: "/".to_string(),
        });

        connection
            .send(req)
            .await
            .expect(&format!("Failed to send rapid message {}", i));
    }

    // Wait for server to verify it received all messages
    server_handle.await.expect("Server task failed");
}
