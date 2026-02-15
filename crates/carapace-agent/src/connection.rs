use carapace_protocol::{Message, MessageCodec};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio_util::codec::{FramedRead, FramedWrite};

use crate::error::{AgentError, Result};

pub struct Connection {
    frame_read: Arc<RwLock<Option<FramedRead<tokio::net::tcp::OwnedReadHalf, MessageCodec>>>>,
    frame_write: Arc<RwLock<Option<FramedWrite<tokio::net::tcp::OwnedWriteHalf, MessageCodec>>>>,
    server_host: String,
    server_port: u16,
    reconnect_attempts: u32,
    reconnect_backoff_ms: u64,
}

impl Connection {
    /// Connect to server via TCP
    pub async fn connect_tcp(server_host: &str, server_port: u16) -> Result<Self> {
        Self::connect_tcp_with_config(server_host, server_port, 5, 100).await
    }

    pub async fn connect_tcp_with_config(
        server_host: &str,
        server_port: u16,
        reconnect_attempts: u32,
        reconnect_backoff_ms: u64,
    ) -> Result<Self> {
        let connection = Connection {
            frame_read: Arc::new(RwLock::new(None)),
            frame_write: Arc::new(RwLock::new(None)),
            server_host: server_host.to_string(),
            server_port,
            reconnect_attempts,
            reconnect_backoff_ms,
        };

        // Establish initial connection
        connection.establish_connection().await?;
        Ok(connection)
    }

    /// Establish/re-establish TCP connection
    async fn establish_connection(&self) -> Result<()> {
        for attempt in 0..self.reconnect_attempts {
            match self.try_connect().await {
                Ok((frame_read, frame_write)) => {
                    let mut read_lock = self.frame_read.write().await;
                    let mut write_lock = self.frame_write.write().await;

                    *read_lock = Some(frame_read);
                    *write_lock = Some(frame_write);

                    tracing::info!(
                        "TCP connection established to {}:{} after {} attempts",
                        self.server_host,
                        self.server_port,
                        attempt + 1
                    );
                    return Ok(());
                }
                Err(e) => {
                    if attempt < self.reconnect_attempts - 1 {
                        let backoff =
                            self.reconnect_backoff_ms * (2_u64.pow(attempt as u32)).min(3600000);
                        tracing::warn!(
                            "Connection attempt {} failed, retrying in {}ms: {}",
                            attempt + 1,
                            backoff,
                            e
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

    /// Try to establish TCP connection (single attempt)
    async fn try_connect(
        &self,
    ) -> Result<(
        FramedRead<tokio::net::tcp::OwnedReadHalf, MessageCodec>,
        FramedWrite<tokio::net::tcp::OwnedWriteHalf, MessageCodec>,
    )> {
        let addr = format!("{}:{}", self.server_host, self.server_port);
        let stream = TcpStream::connect(&addr)
            .await
            .map_err(|e| AgentError::SSHConnectionRefused(format!("TCP connect failed: {}", e)))?;

        let (read, write) = stream.into_split();
        let frame_read = FramedRead::new(read, MessageCodec);
        let frame_write = FramedWrite::new(write, MessageCodec);

        Ok((frame_read, frame_write))
    }

    /// Send a message to the server
    pub async fn send(&self, msg: Message) -> Result<()> {
        let mut write_lock = self.frame_write.write().await;

        if let Some(writer) = write_lock.as_mut() {
            writer.send(msg).await.map_err(|e| {
                tracing::error!("Failed to send message: {}", e);
                AgentError::SSHConnectionRefused(format!("Send failed: {}", e))
            })?;
            Ok(())
        } else {
            Err(AgentError::SSHConnectionRefused(
                "Connection not established".to_string(),
            ))
        }
    }

    /// Receive a message from the server
    pub async fn recv(&self) -> Result<Option<Message>> {
        let mut read_lock = self.frame_read.write().await;

        if let Some(reader) = read_lock.as_mut() {
            match reader.next().await {
                Some(Ok(msg)) => Ok(Some(msg)),
                Some(Err(e)) => {
                    tracing::error!("Failed to receive message: {}", e);
                    Err(AgentError::SSHConnectionRefused(format!(
                        "Recv failed: {}",
                        e
                    )))
                }
                None => Ok(None),
            }
        } else {
            Err(AgentError::SSHConnectionRefused(
                "Connection not established".to_string(),
            ))
        }
    }

    /// Kill the connection
    pub async fn kill(&self) -> Result<()> {
        let mut read_lock = self.frame_read.write().await;
        let mut write_lock = self.frame_write.write().await;

        *read_lock = None;
        *write_lock = None;

        tracing::info!("TCP connection closed");
        Ok(())
    }

    /// Check if connection is healthy
    pub async fn is_healthy(&self) -> bool {
        let read_lock = self.frame_read.read().await;
        let write_lock = self.frame_write.read().await;
        read_lock.is_some() && write_lock.is_some()
    }

    /// Attempt reconnection if needed
    pub async fn reconnect_if_needed(&self) -> Result<()> {
        if !self.is_healthy().await {
            tracing::warn!("Connection unhealthy, attempting reconnect");
            self.establish_connection().await
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_struct() {
        // TCP connection requires actual server to be running
        // This test just verifies the struct compiles
        let _ = std::mem::size_of::<Connection>();
    }
}
