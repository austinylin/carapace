use carapace_protocol::{Message, MessageCodec};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex;
use tokio_util::codec::{FramedRead, FramedWrite};

use crate::audit::AuditLogger;
use crate::cli_dispatch::CliDispatcher;
use crate::http_dispatch::HttpDispatcher;
use crate::rate_limiter::RateLimiter;
use crate::Result;

/// Listens for incoming messages on SSH tunnel and dispatches them
pub struct Listener {
    cli_dispatcher: Arc<CliDispatcher>,
    http_dispatcher: Arc<HttpDispatcher>,
    audit_logger: Arc<AuditLogger>,
    rate_limiter: Arc<RateLimiter>,
}

impl Listener {
    pub fn new(cli_dispatcher: Arc<CliDispatcher>, http_dispatcher: Arc<HttpDispatcher>) -> Self {
        Listener {
            cli_dispatcher,
            http_dispatcher,
            audit_logger: Arc::new(AuditLogger::new()),
            rate_limiter: Arc::new(RateLimiter::new(1000, 60)),
        }
    }

    pub fn with_audit_and_rate_limit(
        cli_dispatcher: Arc<CliDispatcher>,
        http_dispatcher: Arc<HttpDispatcher>,
        audit_logger: Arc<AuditLogger>,
        rate_limiter: Arc<RateLimiter>,
    ) -> Self {
        Listener {
            cli_dispatcher,
            http_dispatcher,
            audit_logger,
            rate_limiter,
        }
    }

    /// Start listening for messages (typically on stdin/stdout)
    pub async fn listen<R, W>(&self, stdin: R, stdout: W) -> Result<()>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let mut frame_read = FramedRead::new(stdin, MessageCodec);
        let frame_write = Arc::new(Mutex::new(FramedWrite::new(stdout, MessageCodec)));

        // Create unbounded channel for SSE events
        // Events sent through this channel are forwarded to client by background task
        let (sse_event_tx, mut sse_event_rx) = tokio::sync::mpsc::unbounded_channel::<Message>();

        // Spawn background task to forward SSE events as they arrive
        // This allows real-time delivery without blocking the main request loop
        let fw_clone = frame_write.clone();
        tokio::spawn(async move {
            while let Some(event) = sse_event_rx.recv().await {
                let mut writer = fw_clone.lock().await;
                match writer.send(event).await {
                    Ok(_) => {
                        if let Err(e) = writer.flush().await {
                            tracing::error!("Failed to flush SSE event: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to send SSE event: {}", e);
                        break;
                    }
                }
            }
        });

        // Main loop: read messages and dispatch them
        while let Some(result) = frame_read.next().await {
            match result {
                Ok(msg) => {
                    // Handle Ping immediately (don't dispatch)
                    if let Message::Ping(ping) = &msg {
                        let pong = Message::Pong(carapace_protocol::PingPong {
                            id: ping.id.clone(),
                            timestamp: ping.timestamp,
                        });
                        let mut writer = frame_write.lock().await;
                        if let Err(e) = writer.send(pong).await {
                            tracing::error!("Failed to send Pong: {}", e);
                        } else if let Err(e) = writer.flush().await {
                            tracing::error!("Failed to flush Pong: {}", e);
                        }
                        continue;
                    }

                    // Log what type of message we received
                    match &msg {
                        Message::CliRequest(_) => tracing::debug!("Received CliRequest message"),
                        Message::HttpRequest(_) => tracing::debug!("Received HttpRequest message"),
                        Message::CliResponse(_) => tracing::debug!("Received CliResponse message"),
                        Message::HttpResponse(_) => {
                            tracing::debug!("Received HttpResponse message")
                        }
                        Message::Error(_) => tracing::debug!("Received Error message"),
                        Message::SseEvent(_) => tracing::debug!("Received SseEvent message"),
                        Message::Ping(_) | Message::Pong(_) => {
                            tracing::debug!("Received Ping/Pong message")
                        }
                    }

                    // Dispatch message, passing SSE channel
                    if let Some(response) =
                        self.dispatch_message(msg, Some(sse_event_tx.clone())).await
                    {
                        let mut writer = frame_write.lock().await;
                        if let Err(e) = writer.send(response).await {
                            tracing::error!("Failed to send response: {}", e);
                            // Continue processing messages instead of closing connection
                        } else if let Err(e) = writer.flush().await {
                            tracing::error!("Failed to flush response: {}", e);
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Error reading message: {}", e);
                    // Continue reading next message
                }
            }
        }

        Ok(())
    }

    /// Dispatch incoming message to appropriate handler
    async fn dispatch_message(
        &self,
        msg: Message,
        sse_event_tx: Option<tokio::sync::mpsc::UnboundedSender<Message>>,
    ) -> Option<Message> {
        match msg {
            Message::CliRequest(req) => {
                // Rate limit check
                if let Err(e) = self.rate_limiter.check_request(&req.tool).await {
                    tracing::warn!("Rate limit exceeded for CLI tool '{}': {}", req.tool, e);
                    self.audit_logger.log_cli_request(
                        &req.id,
                        &req.tool,
                        &req.argv,
                        false,
                        Some("rate_limit_exceeded"),
                    );
                    return Some(Message::Error(carapace_protocol::ErrorMessage {
                        id: Some(req.id),
                        code: "rate_limited".to_string(),
                        message: e.to_string(),
                    }));
                }

                // Audit log the request
                self.audit_logger
                    .log_cli_request(&req.id, &req.tool, &req.argv, true, None);

                let start = std::time::Instant::now();

                match self.cli_dispatcher.dispatch_cli(req.clone()).await {
                    Ok(resp) => {
                        let latency_ms = start.elapsed().as_millis() as u64;
                        self.audit_logger.log_cli_response(
                            &req.id,
                            resp.exit_code,
                            resp.stdout.len(),
                            resp.stderr.len(),
                            latency_ms,
                        );
                        Some(Message::CliResponse(resp))
                    }
                    Err(e) => {
                        let latency_ms = start.elapsed().as_millis() as u64;
                        self.audit_logger
                            .log_cli_response(&req.id, -1, 0, 0, latency_ms);
                        tracing::error!("CLI dispatch error: {}", e);
                        Some(Message::Error(carapace_protocol::ErrorMessage {
                            id: Some(req.id),
                            code: "cli_error".to_string(),
                            message: e.to_string(),
                        }))
                    }
                }
            }
            Message::HttpRequest(req) => {
                tracing::info!(
                    "HTTP request received: {} {} for tool '{}'",
                    req.method,
                    req.path,
                    req.tool
                );

                // Rate limit check
                if let Err(e) = self.rate_limiter.check_request(&req.tool).await {
                    tracing::warn!("Rate limit exceeded for HTTP tool '{}': {}", req.tool, e);
                    self.audit_logger.log_http_request(
                        &req.id,
                        &req.tool,
                        &req.method,
                        &req.path,
                        false,
                        Some("rate_limit_exceeded"),
                    );
                    return Some(Message::Error(carapace_protocol::ErrorMessage {
                        id: Some(req.id),
                        code: "rate_limited".to_string(),
                        message: e.to_string(),
                    }));
                }

                // Audit log the request
                self.audit_logger.log_http_request(
                    &req.id,
                    &req.tool,
                    &req.method,
                    &req.path,
                    true,
                    None,
                );

                let start = std::time::Instant::now();

                match self
                    .http_dispatcher
                    .dispatch_http(req.clone(), sse_event_tx)
                    .await
                {
                    Ok(Some(response)) => {
                        let latency_ms = start.elapsed().as_millis() as u64;
                        self.audit_logger
                            .log_http_response(&req.id, response.status, latency_ms);
                        tracing::info!(
                            "HTTP request {} succeeded with status {}",
                            response.id,
                            response.status
                        );
                        Some(Message::HttpResponse(response))
                    }
                    Ok(None) => {
                        let latency_ms = start.elapsed().as_millis() as u64;
                        self.audit_logger
                            .log_http_response(&req.id, 200, latency_ms);
                        tracing::info!("SSE streaming completed for request {}", req.id);
                        None
                    }
                    Err(e) => {
                        let latency_ms = start.elapsed().as_millis() as u64;
                        self.audit_logger
                            .log_http_response(&req.id, 500, latency_ms);
                        tracing::error!("HTTP dispatch failed for {}: {}", req.id, e);
                        Some(Message::Error(carapace_protocol::ErrorMessage {
                            id: Some(req.id),
                            code: "http_error".to_string(),
                            message: format!("HTTP dispatch error: {}", e),
                        }))
                    }
                }
            }
            Message::CliResponse(_)
            | Message::Error(_)
            | Message::HttpResponse(_)
            | Message::SseEvent(_)
            | Message::Ping(_)
            | Message::Pong(_) => {
                // Server should not receive these from client (Ping handled in listen loop)
                tracing::warn!("Unexpected message type from client");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_listener_creation() {
        let cli_dispatcher = Arc::new(CliDispatcher::new());
        let http_dispatcher = Arc::new(HttpDispatcher::new());
        let _listener = Listener::new(cli_dispatcher, http_dispatcher);
    }
}
