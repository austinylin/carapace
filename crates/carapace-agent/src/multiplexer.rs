use carapace_protocol::Message;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, oneshot};

pub type ResponseWaiter = oneshot::Sender<Message>;

/// Multiplexes concurrent CLI and HTTP requests over a single connection
pub struct Multiplexer {
    // Maps request ID to response waiter
    waiters: Arc<Mutex<HashMap<String, ResponseWaiter>>>,
}

impl Multiplexer {
    pub fn new() -> Self {
        Multiplexer {
            waiters: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a waiter for a request ID
    pub async fn register_waiter(&self, id: String) -> oneshot::Receiver<Message> {
        let (tx, rx) = oneshot::channel();
        self.waiters.lock().await.insert(id, tx);
        rx
    }

    /// Call when a response arrives to wake up the waiter
    pub async fn handle_response(&self, msg: Message) {
        if let Some(id) = msg.id() {
            if let Some(waiter) = self.waiters.lock().await.remove(id) {
                let _ = waiter.send(msg);
            }
        }
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
    async fn test_register_and_handle_response() {
        let multiplexer = Multiplexer::new();
        let id = "req-001".to_string();

        // Register waiter
        let rx = multiplexer.register_waiter(id.clone()).await;

        // Simulate response arrival
        let resp = Message::Error(carapace_protocol::ErrorMessage {
            id: Some(id),
            code: "test".to_string(),
            message: "test".to_string(),
        });

        multiplexer.handle_response(resp).await;

        // Waiter should receive the response
        let result = rx.await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_multiple_concurrent_requests() {
        let multiplexer = Arc::new(Multiplexer::new());
        let mut tasks = vec![];

        // Start 100 concurrent requesters
        for i in 0..100 {
            let m = multiplexer.clone();
            let task = tokio::spawn(async move {
                let id = format!("req-{}", i);
                let _rx = m.register_waiter(id).await;
            });
            tasks.push(task);
        }

        // Wait for all to register
        for task in tasks {
            task.await.unwrap();
        }

        assert_eq!(multiplexer.pending_count().await, 100);
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
}
