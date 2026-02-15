/// Test multiplexer response routing
///
/// The multiplexer is responsible for:
/// 1. Registering waiters for outgoing requests
/// 2. Receiving responses from server
/// 3. Routing responses back to the waiting handler
///
/// If this doesn't work, HTTP handlers will timeout waiting for responses.
use carapace_agent::Multiplexer;
use carapace_protocol::{HttpResponse, Message};
use std::collections::HashMap;

#[tokio::test]
async fn test_multiplexer_registers_waiter() {
    let multiplexer = Multiplexer::new();

    // Register a waiter for request ID "test-1"
    let mut rx = multiplexer.register_waiter("test-1".to_string()).await;

    // Verify the waiter is registered (we should be able to send a response)
    let response = HttpResponse {
        id: "test-1".to_string(),
        status: 200,
        headers: HashMap::new(),
        body: Some(r#"{"result":"ok"}"#.to_string()),
    };

    // Handle response should route it to the waiter
    multiplexer
        .handle_response(Message::HttpResponse(response))
        .await;

    // The waiter should receive the response
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv()).await;

    assert!(
        result.is_ok(),
        "Waiter should receive response within 1 second"
    );

    let msg = result.unwrap();
    match msg {
        Some(Message::HttpResponse(resp)) => {
            assert_eq!(resp.id, "test-1");
            assert_eq!(resp.status, 200);
            assert!(resp.body.unwrap().contains("ok"));
        }
        _ => panic!("Expected HttpResponse"),
    }
}

#[tokio::test]
async fn test_multiplexer_routes_correct_response() {
    let multiplexer = Multiplexer::new();

    // Register two waiters
    let mut rx1 = multiplexer.register_waiter("request-1".to_string()).await;
    let mut rx2 = multiplexer.register_waiter("request-2".to_string()).await;

    // Send response for request-2
    let response_2 = HttpResponse {
        id: "request-2".to_string(),
        status: 200,
        headers: HashMap::new(),
        body: Some("response-2".to_string()),
    };
    multiplexer
        .handle_response(Message::HttpResponse(response_2))
        .await;

    // rx2 should get it
    let result2 = tokio::time::timeout(std::time::Duration::from_millis(100), rx2.recv()).await;
    assert!(result2.is_ok(), "rx2 should receive response-2");

    // rx1 should NOT get it (should timeout)
    let result1 = tokio::time::timeout(std::time::Duration::from_millis(100), rx1.recv()).await;
    assert!(result1.is_err(), "rx1 should not receive response-2");
}

#[tokio::test]
async fn test_multiplexer_concurrent_requests() {
    let multiplexer = std::sync::Arc::new(Multiplexer::new());

    // Spawn 10 concurrent requests
    let mut handles = vec![];

    for i in 0..10 {
        let mux = multiplexer.clone();
        let handle = tokio::spawn(async move {
            let request_id = format!("request-{}", i);
            let mut rx = mux.register_waiter(request_id.clone()).await;

            // Simulate server sending response back
            let response = HttpResponse {
                id: request_id.clone(),
                status: 200,
                headers: HashMap::new(),
                body: Some(format!("response-{}", i)),
            };

            mux.handle_response(Message::HttpResponse(response)).await;

            // Wait for response
            tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv()).await
        });

        handles.push(handle);
    }

    // All should complete successfully
    for handle in handles {
        let result = handle.await;
        assert!(result.is_ok(), "Task should complete without panicking");
        assert!(
            result.unwrap().is_ok(),
            "Should receive response within timeout"
        );
    }
}
