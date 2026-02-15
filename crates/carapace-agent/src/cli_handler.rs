use carapace_protocol::{CliRequest, Message};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use uuid::Uuid;

use crate::connection::Connection;
use crate::error::Result;
use crate::multiplexer::Multiplexer;

pub struct CliHandler {
    socket_path: String,
    multiplexer: Arc<Multiplexer>,
    connection: Arc<Connection>,
}

impl CliHandler {
    pub fn new(
        socket_path: String,
        multiplexer: Arc<Multiplexer>,
        connection: Arc<Connection>,
    ) -> Self {
        CliHandler {
            socket_path,
            multiplexer,
            connection,
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
            let connection = self.connection.clone();

            tokio::spawn(async move {
                if let Err(e) = Self::handle_client(socket, multiplexer, connection).await {
                    tracing::error!("Error handling CLI client: {}", e);
                }
            });
        }
    }

    async fn handle_client(
        mut socket: UnixStream,
        multiplexer: Arc<Multiplexer>,
        connection: Arc<Connection>,
    ) -> Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // Read request from socket using dynamic buffer
        let mut buf = Vec::with_capacity(8192);
        let mut tmp = [0u8; 4096];
        loop {
            let n = socket.read(&mut tmp).await?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
            // If we read less than the buffer, we likely have the full message
            if n < tmp.len() {
                break;
            }
        }

        if buf.is_empty() {
            return Ok(());
        }

        // Parse JSON request
        let req_json: serde_json::Value = serde_json::from_slice(&buf)?;

        // Extract fields
        let tool = req_json["tool"].as_str().unwrap_or("unknown").to_string();
        let argv: Vec<String> = req_json["argv"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.to_string())
            .collect();

        // Extract environment from the request
        let mut env = HashMap::new();
        if let Some(env_obj) = req_json["env"].as_object() {
            for (key, value) in env_obj {
                if let Some(v) = value.as_str() {
                    env.insert(key.clone(), v.to_string());
                }
            }
        }

        // Create CLI request
        let id = Uuid::new_v4().to_string();
        let cli_req = CliRequest {
            id: id.clone(),
            tool,
            argv,
            env,
            stdin: None,
            cwd: std::env::current_dir()?.to_str().unwrap_or("/").to_string(),
        };

        // Register waiter for response
        let mut rx = multiplexer.register_waiter(id.clone()).await;

        // Send request to server via connection
        let msg = Message::CliRequest(cli_req);
        if let Err(e) = connection.send(msg).await {
            multiplexer.remove_waiter(&id).await;
            return Err(e);
        }

        // Wait for response (with timeout)
        let response =
            match tokio::time::timeout(tokio::time::Duration::from_secs(30), rx.recv()).await {
                Ok(Some(msg)) => msg,
                Ok(None) => {
                    multiplexer.remove_waiter(&id).await;
                    return Err(crate::error::AgentError::RequestNotFound(id));
                }
                Err(_) => {
                    multiplexer.remove_waiter(&id).await;
                    return Err(crate::error::AgentError::RequestTimeout(
                        "CLI request timeout".to_string(),
                    ));
                }
            };

        // Clean up waiter after receiving response
        multiplexer.remove_waiter(&id).await;

        // Send response back to client
        let json = serde_json::to_vec(&response)?;
        socket.write_all(&json).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_cli_handler_compilation() {
        // Note: Can't easily test CliHandler without real Connection
        // Real testing is done in integration tests
        // This test just verifies the module compiles
    }
}
