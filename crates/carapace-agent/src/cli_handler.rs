use carapace_protocol::{Message, CliRequest, CliResponse};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use uuid::Uuid;

use crate::multiplexer::Multiplexer;
use crate::error::Result;

pub struct CliHandler {
    socket_path: String,
    multiplexer: Arc<Multiplexer>,
}

impl CliHandler {
    pub fn new(socket_path: String, multiplexer: Arc<Multiplexer>) -> Self {
        CliHandler {
            socket_path,
            multiplexer,
        }
    }

    /// Start listening for CLI requests on Unix socket
    pub async fn listen(&self) -> Result<()> {
        // Remove existing socket if present
        let _ = std::fs::remove_file(&self.socket_path);

        let listener = UnixListener::bind(&self.socket_path)?;
        tracing::info!("CLI handler listening on {}", self.socket_path);

        loop {
            let (socket, _) = listener.accept().await?;
            let multiplexer = self.multiplexer.clone();

            tokio::spawn(async move {
                if let Err(e) = Self::handle_client(socket, multiplexer).await {
                    tracing::error!("Error handling CLI client: {}", e);
                }
            });
        }
    }

    async fn handle_client(
        mut socket: UnixStream,
        multiplexer: Arc<Multiplexer>,
    ) -> Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // Read request from socket
        let mut buf = [0u8; 4096];
        let n = socket.read(&mut buf).await?;

        if n == 0 {
            return Ok(());
        }

        // Parse JSON request
        let req_json: serde_json::Value = serde_json::from_slice(&buf[..n])?;

        // Extract fields
        let tool = req_json["tool"].as_str().unwrap_or("unknown").to_string();
        let argv: Vec<String> = req_json["argv"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.to_string())
            .collect();

        // Create CLI request
        let id = Uuid::new_v4().to_string();
        let cli_req = CliRequest {
            id: id.clone(),
            tool,
            argv,
            env: HashMap::new(),
            stdin: None,
            cwd: std::env::current_dir()?
                .to_str()
                .unwrap_or("/")
                .to_string(),
        };

        let _msg = Message::CliRequest(cli_req);

        // Register waiter for response
        let rx = multiplexer.register_waiter(id.clone()).await;

        // In real implementation, would send to server here
        // For now, just simulate
        tokio::spawn({
            let multiplexer = multiplexer.clone();
            async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

                let resp = Message::CliResponse(CliResponse {
                    id,
                    exit_code: 0,
                    stdout: "test output".to_string(),
                    stderr: "".to_string(),
                });

                multiplexer.handle_response(resp).await;
            }
        });

        // Wait for response
        if let Ok(response) = rx.await {
            let json = serde_json::to_vec(&response)?;
            socket.write_all(&json).await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_handler_creation() {
        let multiplexer = Arc::new(Multiplexer::new());
        let handler = CliHandler::new("/tmp/test.sock".to_string(), multiplexer);

        assert_eq!(handler.socket_path, "/tmp/test.sock");
    }
}
