use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Information about an active connection
#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    pub remote_addr: SocketAddr,
    pub connected_at: chrono::DateTime<chrono::Utc>,
}

/// Tracks active TCP connections to the server
pub struct ConnectionTracker {
    connections: Mutex<Vec<ConnectionInfo>>,
}

impl ConnectionTracker {
    /// Create a new connection tracker
    pub fn new() -> Arc<Self> {
        Arc::new(ConnectionTracker {
            connections: Mutex::new(Vec::new()),
        })
    }

    /// Register a new connection
    pub async fn register(&self, addr: SocketAddr) {
        let mut connections = self.connections.lock().await;
        connections.push(ConnectionInfo {
            remote_addr: addr,
            connected_at: chrono::Utc::now(),
        });
        tracing::debug!(
            "Connection registered from {}, total: {}",
            addr,
            connections.len()
        );
    }

    /// Unregister a connection
    pub async fn unregister(&self, addr: SocketAddr) {
        let mut connections = self.connections.lock().await;
        connections.retain(|c| c.remote_addr != addr);
        tracing::debug!(
            "Connection unregistered from {}, total: {}",
            addr,
            connections.len()
        );
    }

    /// Get all active connections
    pub async fn get_all(&self) -> Vec<ConnectionInfo> {
        self.connections.lock().await.clone()
    }

    /// Get active connection count
    pub async fn count(&self) -> usize {
        self.connections.lock().await.len()
    }
}

impl Default for ConnectionTracker {
    fn default() -> Self {
        Self {
            connections: Mutex::new(Vec::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[tokio::test]
    async fn test_register_connection() {
        let tracker = ConnectionTracker::new();
        let addr = SocketAddr::from_str("127.0.0.1:1234").unwrap();

        tracker.register(addr).await;
        assert_eq!(tracker.count().await, 1);

        let connections = tracker.get_all().await;
        assert_eq!(connections[0].remote_addr, addr);
    }

    #[tokio::test]
    async fn test_unregister_connection() {
        let tracker = ConnectionTracker::new();
        let addr = SocketAddr::from_str("127.0.0.1:1234").unwrap();

        tracker.register(addr).await;
        assert_eq!(tracker.count().await, 1);

        tracker.unregister(addr).await;
        assert_eq!(tracker.count().await, 0);
    }

    #[tokio::test]
    async fn test_multiple_connections() {
        let tracker = ConnectionTracker::new();
        let addr1 = SocketAddr::from_str("127.0.0.1:1234").unwrap();
        let addr2 = SocketAddr::from_str("127.0.0.1:5678").unwrap();

        tracker.register(addr1).await;
        tracker.register(addr2).await;
        assert_eq!(tracker.count().await, 2);

        tracker.unregister(addr1).await;
        assert_eq!(tracker.count().await, 1);

        let connections = tracker.get_all().await;
        assert_eq!(connections[0].remote_addr, addr2);
    }
}
