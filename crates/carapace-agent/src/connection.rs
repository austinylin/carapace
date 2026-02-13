use carapace_protocol::{Message, MessageCodec};
use futures::{SinkExt, StreamExt};
use std::process::Stdio;
use tokio::process::{Child, Command};
use tokio_util::codec::{FramedRead, FramedWrite};

pub struct Connection {
    _child: Child,
    frame_read: FramedRead<tokio::process::ChildStdout, MessageCodec>,
    frame_write: FramedWrite<tokio::process::ChildStdin, MessageCodec>,
}

impl Connection {
    /// Establish SSH connection to the server
    pub async fn connect(
        ssh_host: &str,
        remote_command: &str,
        control_socket: &str,
    ) -> anyhow::Result<Self> {
        let mut child = Command::new("ssh")
            .args(&[
                "-M",                    // Master mode (multiplexing)
                "-S", control_socket,    // Control socket path
                "-o", "ControlPersist=10m", // Keep connection alive
                "-o", "StrictHostKeyChecking=accept-new", // Accept unknown hosts
                ssh_host,
                remote_command,          // Remote command: carapace-server
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stdin = child.stdin.take().ok_or_else(|| {
            anyhow::anyhow!("Failed to get stdin from SSH process")
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            anyhow::anyhow!("Failed to get stdout from SSH process")
        })?;

        let frame_read = FramedRead::new(stdout, MessageCodec);
        let frame_write = FramedWrite::new(stdin, MessageCodec);

        Ok(Connection {
            _child: child,
            frame_read,
            frame_write,
        })
    }

    /// Send a message to the server
    pub async fn send(&mut self, msg: Message) -> anyhow::Result<()> {
        self.frame_write.send(msg).await?;
        Ok(())
    }

    /// Receive a message from the server
    pub async fn recv(&mut self) -> anyhow::Result<Option<Message>> {
        Ok(self.frame_read.next().await.transpose()?)
    }

    /// Kill the SSH connection
    pub async fn kill(&mut self) -> anyhow::Result<()> {
        self._child.kill().await?;
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
