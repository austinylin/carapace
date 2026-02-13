use carapace_protocol::{Message, MessageCodec};
use futures::{StreamExt, SinkExt};
use std::sync::Arc;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::cli_dispatch::CliDispatcher;

/// Listens for incoming messages on SSH tunnel and dispatches them
pub struct Listener {
    dispatcher: Arc<CliDispatcher>,
}

impl Listener {
    pub fn new(dispatcher: Arc<CliDispatcher>) -> Self {
        Listener { dispatcher }
    }

    /// Start listening for messages (typically on stdin/stdout)
    pub async fn listen<R, W>(
        &self,
        stdin: R,
        stdout: W,
    ) -> anyhow::Result<()>
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
            Message::CliRequest(req) => {
                match self.dispatcher.dispatch_cli(req).await {
                    Ok(resp) => Some(Message::CliResponse(resp)),
                    Err(e) => {
                        tracing::error!("CLI dispatch error: {}", e);
                        Some(Message::Error(carapace_protocol::ErrorMessage {
                            id: None,
                            code: "cli_error".to_string(),
                            message: e.to_string(),
                        }))
                    }
                }
            }
            Message::HttpRequest(_req) => {
                // HTTP dispatch not yet implemented
                Some(Message::Error(carapace_protocol::ErrorMessage {
                    id: None,
                    code: "not_implemented".to_string(),
                    message: "HTTP dispatch not yet implemented".to_string(),
                }))
            }
            Message::CliResponse(_) | Message::Error(_) | Message::HttpResponse(_) | Message::SseEvent { .. } => {
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
        let dispatcher = Arc::new(CliDispatcher::new());
        let _listener = Listener::new(dispatcher);
    }
}
