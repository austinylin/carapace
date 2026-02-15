use carapace_protocol::{Message, MessageCodec};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
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
    pub fn new(
        cli_dispatcher: Arc<CliDispatcher>,
        http_dispatcher: Arc<HttpDispatcher>,
    ) -> Self {
        Listener {
            cli_dispatcher,
            http_dispatcher,
        }
    }

    /// Start listening for messages (typically on stdin/stdout)
    pub async fn listen<R, W>(&self, stdin: R, stdout: W) -> Result<()>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut frame_read = FramedRead::new(stdin, MessageCodec);
        let mut frame_write = FramedWrite::new(stdout, MessageCodec);

        // Main loop: read messages and dispatch them
        while let Some(result) = frame_read.next().await {
            match result {
                Ok(msg) => {
                    // Dispatch message
                    if let Some(response) = self.dispatch_message(msg).await {
                        frame_write.send(response).await?;
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
    async fn dispatch_message(&self, msg: Message) -> Option<Message> {
        match msg {
            Message::CliRequest(req) => match self.cli_dispatcher.dispatch_cli(req).await {
                Ok(resp) => Some(Message::CliResponse(resp)),
                Err(e) => {
                    tracing::error!("CLI dispatch error: {}", e);
                    Some(Message::Error(carapace_protocol::ErrorMessage {
                        id: None,
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

                match self.http_dispatcher.dispatch_http(req.clone()).await {
                    Ok(response) => {
                        tracing::info!("HTTP request {} succeeded with status {}", response.id, response.status);
                        Some(Message::HttpResponse(response))
                    }
                    Err(e) => {
                        tracing::error!("HTTP dispatch failed: {}", e);
                        Some(Message::Error(carapace_protocol::ErrorMessage {
                            id: None,
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
