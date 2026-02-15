use carapace_protocol::{Message, MessageCodec};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex;
use tokio_util::codec::{FramedRead, FramedWrite};

use crate::cli_dispatch::CliDispatcher;
use crate::http_dispatch::HttpDispatcher;
use crate::Result;

/// Listens for incoming messages on SSH tunnel and dispatches them
pub struct Listener {
    cli_dispatcher: Arc<CliDispatcher>,
    http_dispatcher: Arc<HttpDispatcher>,
}

impl Listener {
    pub fn new(cli_dispatcher: Arc<CliDispatcher>, http_dispatcher: Arc<HttpDispatcher>) -> Self {
        Listener {
            cli_dispatcher,
            http_dispatcher,
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
                match fw_clone.lock().await.send(event).await {
                    Ok(_) => {}
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
                    // Log what type of message we received
                    match &msg {
                        Message::CliRequest(_) => tracing::debug!("Received CliRequest message"),
                        Message::HttpRequest(_) => tracing::debug!("Received HttpRequest message"),
                        Message::CliResponse(_) => tracing::debug!("Received CliResponse message"),
                        Message::HttpResponse(_) => {
                            tracing::debug!("Received HttpResponse message")
                        }
                        Message::Error(_) => tracing::debug!("Received Error message"),
                        Message::SseEvent { .. } => tracing::debug!("Received SseEvent message"),
                    }

                    // Dispatch message, passing SSE channel
                    if let Some(response) =
                        self.dispatch_message(msg, Some(sse_event_tx.clone())).await
                    {
                        if let Err(e) = frame_write.lock().await.send(response).await {
                            tracing::error!("Failed to send response: {}", e);
                            // Continue processing messages instead of closing connection
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
            Message::CliRequest(req) => match self.cli_dispatcher.dispatch_cli(req.clone()).await {
                Ok(resp) => Some(Message::CliResponse(resp)),
                Err(e) => {
                    tracing::error!("CLI dispatch error: {}", e);
                    Some(Message::Error(carapace_protocol::ErrorMessage {
                        id: Some(req.id),
                        code: "cli_error".to_string(),
                        message: e.to_string(),
                    }))
                }
            },
            Message::HttpRequest(req) => {
                tracing::info!(
                    "HTTP request received: {} {} for tool '{}'",
                    req.method,
                    req.path,
                    req.tool
                );

                match self
                    .http_dispatcher
                    .dispatch_http(req.clone(), sse_event_tx)
                    .await
                {
                    Ok(Some(response)) => {
                        // Non-SSE response: return normally
                        tracing::info!(
                            "HTTP request {} succeeded with status {}",
                            response.id,
                            response.status
                        );
                        Some(Message::HttpResponse(response))
                    }
                    Ok(None) => {
                        // SSE response: events were already sent through sse_event_tx
                        tracing::info!("SSE streaming completed for request {}", req.id);
                        None
                    }
                    Err(e) => {
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
            | Message::SseEvent { .. } => {
                // Server should not receive these from client
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
