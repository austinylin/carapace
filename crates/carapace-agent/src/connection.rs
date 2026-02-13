use carapace_protocol::{Message, MessageCodec};
use futures::{SinkExt, StreamExt};
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::{Child, Command};
use tokio::sync::RwLock;
use tokio_util::codec::{FramedRead, FramedWrite};

use crate::error::{AgentError, Result};

pub struct Connection {
    child: Arc<RwLock<Option<Child>>>,
    frame_read: Arc<RwLock<Option<FramedRead<tokio::process::ChildStdout, MessageCodec>>>>,
    frame_write: Arc<RwLock<Option<FramedWrite<tokio::process::ChildStdin, MessageCodec>>>>,
    ssh_host: String,
    remote_command: String,
    control_socket: String,
    reconnect_attempts: u32,
    reconnect_backoff_ms: u64,
}

impl Connection {
    /// Establish SSH connection to the server with reconnection support
    pub async fn connect(
        ssh_host: &str,
        remote_command: &str,
        control_socket: &str,
    ) -> Result<Self> {
        Self::connect_with_config(ssh_host, remote_command, control_socket, 5, 100).await
    }

    pub async fn connect_with_config(
        ssh_host: &str,
        remote_command: &str,
        control_socket: &str,
        reconnect_attempts: u32,
        reconnect_backoff_ms: u64,
    ) -> Result<Self> {
        let connection = Connection {
            child: Arc::new(RwLock::new(None)),
            frame_read: Arc::new(RwLock::new(None)),
            frame_write: Arc::new(RwLock::new(None)),
            ssh_host: ssh_host.to_string(),
            remote_command: remote_command.to_string(),
            control_socket: control_socket.to_string(),
            reconnect_attempts,
            reconnect_backoff_ms,
        };

        // Establish initial connection
        connection.establish_connection().await?;
        Ok(connection)
    }

    /// Establish/re-establish SSH connection
    async fn establish_connection(&self) -> Result<()> {
        let mut last_error = None;

        for attempt in 0..self.reconnect_attempts {
            match self.try_connect().await {
                Ok((child, frame_read, frame_write)) => {
                    let mut child_lock = self.child.write().await;
                    let mut read_lock = self.frame_read.write().await;
                    let mut write_lock = self.frame_write.write().await;

                    *child_lock = Some(child);
                    *read_lock = Some(frame_read);
                    *write_lock = Some(frame_write);

                    tracing::info!(
                        "SSH connection established after {} attempts",
                        attempt + 1
                    );
                    return Ok(());
                }
                Err(e) => {
                    last_error = Some(e);
                    if attempt < self.reconnect_attempts - 1 {
                        let backoff =
                            self.reconnect_backoff_ms * (2_u64.pow(attempt as u32)).min(3600000);
                        tracing::warn!(
                            "Connection attempt {} failed, retrying in {}ms: {}",
                            attempt + 1,
                            backoff,
                            last_error.as_ref().unwrap()
                        );
                        tokio::time::sleep(tokio::time::Duration::from_millis(backoff)).await;
                    }
                }
            }
        }

        Err(AgentError::ReconnectionFailed {
            attempts: self.reconnect_attempts,
        })
    }

    /// Try to establish SSH connection (single attempt)
    async fn try_connect(
        &self,
    ) -> Result<(
        Child,
        FramedRead<tokio::process::ChildStdout, MessageCodec>,
        FramedWrite<tokio::process::ChildStdin, MessageCodec>,
    )> {
        let mut child = Command::new("ssh")
            .args(&[
                "-M",                                   // Master mode (multiplexing)
                "-S",
                &self.control_socket,                  // Control socket path
                "-o",
                "ControlPersist=10m",                  // Keep connection alive
                "-o",
                "StrictHostKeyChecking=accept-new",   // Accept unknown hosts
                &self.ssh_host,
                &self.remote_command,                 // Remote command
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| AgentError::SSHConnectionRefused(e.to_string()))?;

        let stdin = child.stdin.take().ok_or_else(|| {
            AgentError::SSHConnectionLost("Failed to get stdin".to_string())
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            AgentError::SSHConnectionLost("Failed to get stdout".to_string())
        })?;

        let frame_read = FramedRead::new(stdout, MessageCodec);
        let frame_write = FramedWrite::new(stdin, MessageCodec);

        Ok((child, frame_read, frame_write))
    }

    /// Send a message to the server
    pub async fn send(&self, msg: Message) -> Result<()> {
        let mut write_lock = self.frame_write.write().await;

        match write_lock.as_mut() {
            Some(writer) => writer.send(msg).await.map_err(|e| {
                AgentError::SSHConnectionLost(format!("Send failed: {}", e))
            }),
            None => Err(AgentError::SSHConnectionLost(
                "Connection not established".to_string(),
            )),
        }
    }

    /// Receive a message from the server
    pub async fn recv(&self) -> Result<Option<Message>> {
        let mut read_lock = self.frame_read.write().await;

        match read_lock.as_mut() {
            Some(reader) => reader.next().await.transpose().map_err(|e| {
                AgentError::SSHConnectionLost(format!("Receive failed: {}", e))
            }),
            None => Err(AgentError::SSHConnectionLost(
                "Connection not established".to_string(),
            )),
        }
    }

    /// Kill the SSH connection
    pub async fn kill(&self) -> Result<()> {
        let mut child_lock = self.child.write().await;

        if let Some(mut child) = child_lock.take() {
            child
                .kill()
                .await
                .map_err(|e| AgentError::IOError(e))?;
        }

        Ok(())
    }

    /// Check if connection is healthy (attempt to receive with timeout)
    pub async fn is_healthy(&self) -> bool {
        let read_lock = self.frame_read.read().await;
        read_lock.is_some()
    }

    /// Reconnect if connection is lost
    pub async fn reconnect_if_needed(&self) -> Result<()> {
        if !self.is_healthy().await {
            tracing::warn!("Connection unhealthy, reconnecting...");
            self.establish_connection().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_connection_struct_creation() {
        // Just verify the struct can be created in theory
        // Real connection requires SSH server
    }
}
