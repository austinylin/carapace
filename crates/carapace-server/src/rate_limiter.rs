use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

use crate::error::{Result, ServerError};

/// Rate limit tracking for a single tool
#[derive(Debug, Clone)]
struct RateLimitWindow {
    /// Requests in current window
    requests: u32,
    /// Window start time (unix timestamp)
    window_start: u64,
    /// Max requests allowed
    max_requests: u32,
    /// Window size in seconds
    window_secs: u64,
}

impl RateLimitWindow {
    fn new(max_requests: u32, window_secs: u64) -> Self {
        RateLimitWindow {
            requests: 0,
            window_start: current_timestamp(),
            max_requests,
            window_secs,
        }
    }

    /// Check if window has expired
    fn is_expired(&self) -> bool {
        current_timestamp() >= self.window_start + self.window_secs
    }

    /// Reset window if expired
    fn reset_if_needed(&mut self) {
        if self.is_expired() {
            self.requests = 0;
            self.window_start = current_timestamp();
        }
    }

    /// Check if request is allowed and increment counter
    fn check_and_increment(&mut self) -> bool {
        self.reset_if_needed();

        if self.requests < self.max_requests {
            self.requests += 1;
            true
        } else {
            false
        }
    }
}

/// Rate limiter for per-tool request limiting
pub struct RateLimiter {
    /// Per-tool rate limit windows
    windows: Arc<RwLock<HashMap<String, RateLimitWindow>>>,
    /// Default limit if no per-tool override
    default_max_requests: u32,
    /// Default window size
    default_window_secs: u64,
}

impl RateLimiter {
    pub fn new(default_max_requests: u32, default_window_secs: u64) -> Self {
        RateLimiter {
            windows: Arc::new(RwLock::new(HashMap::new())),
            default_max_requests,
            default_window_secs,
        }
    }

    /// Set per-tool limit
    pub async fn set_limit(&self, tool: &str, max_requests: u32, window_secs: u64) {
        let mut windows = self.windows.write().await;
        windows.insert(
            tool.to_string(),
            RateLimitWindow::new(max_requests, window_secs),
        );
    }

    /// Check if request is allowed for tool
    pub async fn check_request(&self, tool: &str) -> Result<()> {
        let mut windows = self.windows.write().await;

        let window = windows.entry(tool.to_string()).or_insert_with(|| {
            RateLimitWindow::new(self.default_max_requests, self.default_window_secs)
        });

        if window.check_and_increment() {
            Ok(())
        } else {
            Err(ServerError::RateLimitExceeded {
                tool: tool.to_string(),
            })
        }
    }

    /// Get current window stats for tool
    pub async fn get_stats(&self, tool: &str) -> Option<(u32, u32, u64)> {
        let windows = self.windows.read().await;
        windows
            .get(tool)
            .map(|w| (w.requests, w.max_requests, w.window_secs))
    }

    /// Reset all limits
    pub async fn reset_all(&self) {
        self.windows.write().await.clear();
    }
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rate_limit_allows_requests() {
        let limiter = RateLimiter::new(5, 1);

        for _ in 0..5 {
            assert!(limiter.check_request("test_tool").await.is_ok());
        }
    }

    #[tokio::test]
    async fn test_rate_limit_denies_after_max() {
        let limiter = RateLimiter::new(5, 60);

        for _ in 0..5 {
            let _ = limiter.check_request("test_tool").await;
        }

        // 6th request should fail
        assert!(limiter.check_request("test_tool").await.is_err());
    }

    #[tokio::test]
    async fn test_rate_limit_window_reset() {
        let limiter = RateLimiter::new(2, 1);

        // Fill window
        let _ = limiter.check_request("tool").await;
        let _ = limiter.check_request("tool").await;

        // This should fail
        assert!(limiter.check_request("tool").await.is_err());

        // Wait for window to expire (simulate)
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        // Should allow again
        assert!(limiter.check_request("tool").await.is_ok());
    }

    #[tokio::test]
    async fn test_per_tool_limits() {
        let limiter = RateLimiter::new(10, 60);

        // Set specific limit for "tool1"
        limiter.set_limit("tool1", 2, 60).await;

        // tool1 allows 2
        assert!(limiter.check_request("tool1").await.is_ok());
        assert!(limiter.check_request("tool1").await.is_ok());
        assert!(limiter.check_request("tool1").await.is_err());

        // tool2 uses default of 10
        for _ in 0..10 {
            assert!(limiter.check_request("tool2").await.is_ok());
        }
        assert!(limiter.check_request("tool2").await.is_err());
    }

    #[test]
    fn test_current_timestamp() {
        let ts = current_timestamp();
        assert!(ts > 0);
    }
}
