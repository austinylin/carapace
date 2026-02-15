use carapace_protocol::Message;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

pub type ResponseWaiter = mpsc::Sender<Message>;

/// Multiplexes concurrent CLI and HTTP requests over a single connection
///
/// Supports both single-response (HTTP) and multi-message (SSE) patterns:
/// - HTTP requests: channel receives 1 HttpResponse message
/// - SSE requests: channel receives N SseEvent messages + completion signal
pub struct Multiplexer {
    // Maps request ID to response channel
    waiters: Arc<Mutex<HashMap<String, ResponseWaiter>>>,
}

impl Multiplexer {
    pub fn new() -> Self {
        Multiplexer {
            waiters: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a waiter for a request ID (supports single or multiple responses)
    ///
    /// Returns an mpsc receiver that can handle:
    /// - Single message for HTTP requests
    /// - Multiple SseEvent messages for SSE streaming
    pub async fn register_waiter(&self, id: String) -> mpsc::Receiver<Message> {
        let (tx, rx) = mpsc::channel(100); // Buffer up to 100 messages per request
        self.waiters.lock().await.insert(id, tx);
        rx
    }

    /// Call when a response arrives to send to the waiter
    ///
    /// For HTTP: sends single HttpResponse (channel then closes on receiver side)
    /// For SSE: sends multiple SseEvent messages followed by completion signal
    pub async fn handle_response(&self, msg: Message) {
        if let Some(id) = msg.id() {
            let waiters = self.waiters.lock().await;
            if let Some(tx) = waiters.get(id) {
                let _ = tx.send(msg).await;
                // Don't remove yet - for SSE, more messages may arrive
                // Only remove on Error or final completion signal
            }
        }
    }

    /// Called when connection is lost (network failure, timeout, etc.)
    /// Sends error to all active waiters so they can detect the disconnect
    pub async fn cleanup_on_disconnect(&self) {
        let mut waiters = self.waiters.lock().await;
        for (id, _) in waiters.drain() {
            // Note: We don't send error message here since the channel
            // sender is dropped when this function exits. Instead,
            // the receiver will get None when trying to recv(), which signals
            // that the channel closed unexpectedly (connection lost).
            tracing::debug!("Cleaned up waiter for disconnected request: {}", id);
        }
    }

    /// Remove a waiter after completion (for HTTP requests, called after single response received)
    pub async fn remove_waiter(&self, id: &str) {
        self.waiters.lock().await.remove(id);
    }

    /// Get number of pending requests
    pub async fn pending_count(&self) -> usize {
        self.waiters.lock().await.len()
    }

    /// Clean up all waiters (on shutdown)
    pub async fn clear(&self) {
        self.waiters.lock().await.clear();
    }
}

impl Default for Multiplexer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_single_http_response() {
        let multiplexer = Multiplexer::new();
        let id = "http-001".to_string();

        // Register waiter
        let mut rx = multiplexer.register_waiter(id.clone()).await;

        // Simulate HTTP response
        let resp = Message::Error(carapace_protocol::ErrorMessage {
            id: Some(id),
            code: "test".to_string(),
            message: "test".to_string(),
        });

        multiplexer.handle_response(resp).await;

        // Waiter should receive the response
        let result = rx.recv().await;
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_multiple_sse_events() {
        let multiplexer = Arc::new(Multiplexer::new());
        let id = "sse-001".to_string();

        // Register waiter for SSE
        let mut rx = multiplexer.register_waiter(id.clone()).await;

        // Clone multiplexer for background task
        let m = multiplexer.clone();
        let id_clone = id.clone();

        // Spawn task to send multiple SSE events
        tokio::spawn(async move {
            for i in 0..3 {
                let event = Message::SseEvent(carapace_protocol::SseEvent {
                    id: id_clone.clone(),
                    tool: "signal-cli".to_string(),
                    event: "message".to_string(),
                    data: format!(r#"{{"num":{}}}"#, i),
                });
                m.handle_response(event).await;
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            }
        });

        // Receive all 3 events
        let mut count = 0;
        while let Some(msg) = rx.recv().await {
            if let Message::SseEvent(evt) = msg {
                assert_eq!(evt.id, id);
                count += 1;
            }
            if count >= 3 {
                break;
            }
        }
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn test_concurrent_requests_isolated() {
        let multiplexer = Arc::new(Multiplexer::new());
        let mut tasks = vec![];

        // Start 10 concurrent requests
        for i in 0..10 {
            let m = multiplexer.clone();
            let task = tokio::spawn(async move {
                let id = format!("req-{}", i);
                let mut rx = m.register_waiter(id.clone()).await;

                // Send a response
                let resp = Message::Error(carapace_protocol::ErrorMessage {
                    id: Some(id),
                    code: "test".to_string(),
                    message: format!("response-{}", i),
                });
                m.handle_response(resp).await;

                // Receive it
                if let Some(Message::Error(err)) = rx.recv().await {
                    assert_eq!(err.message, format!("response-{}", i));
                    return true;
                }
                false
            });
            tasks.push(task);
        }

        // All requests should complete successfully
        for task in tasks {
            let result = task.await.unwrap();
            assert!(result);
        }

        assert_eq!(multiplexer.pending_count().await, 10);
    }

    #[tokio::test]
    async fn test_clear_all_waiters() {
        let multiplexer = Multiplexer::new();

        for i in 0..10 {
            multiplexer.register_waiter(format!("req-{}", i)).await;
        }

        assert_eq!(multiplexer.pending_count().await, 10);

        multiplexer.clear().await;
        assert_eq!(multiplexer.pending_count().await, 0);
    }

    #[tokio::test]
    async fn test_orphaned_response_ignored() {
        let multiplexer = Multiplexer::new();

        // Send response with no waiter
        let resp = Message::Error(carapace_protocol::ErrorMessage {
            id: Some("nonexistent".to_string()),
            code: "test".to_string(),
            message: "test".to_string(),
        });

        // Should not panic
        multiplexer.handle_response(resp).await;
        assert_eq!(multiplexer.pending_count().await, 0);
    }

    #[tokio::test]
    async fn test_cleanup_on_disconnect() {
        let multiplexer = Arc::new(Multiplexer::new());
        let id = "disconnecting-req".to_string();

        // Register waiter
        let mut rx = multiplexer.register_waiter(id.clone()).await;

        assert_eq!(multiplexer.pending_count().await, 1);

        // Simulate connection drop
        multiplexer.cleanup_on_disconnect().await;

        // Waiter should detect the disconnect
        let result = rx.recv().await;
        assert!(
            result.is_none(),
            "Channel should be closed after disconnect"
        );

        assert_eq!(multiplexer.pending_count().await, 0);
    }
}
