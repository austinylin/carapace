use carapace_agent::{CliHandler, Connection, HttpProxy, Multiplexer, Result as AgentResult};
use carapace_protocol::{Message, PingPong};
use std::sync::Arc;

#[tokio::main]
async fn main() -> AgentResult<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    tracing::info!("carapace-agent starting");

    // Load config from environment
    let config = carapace_agent::config::AgentConfig::from_env();

    // Establish TCP connection to server
    let connection =
        Arc::new(Connection::connect_tcp(&config.server.host, config.server.port).await?);

    tracing::info!(
        "TCP connection established to {}:{}",
        config.server.host,
        config.server.port
    );

    // Create multiplexer for request/response matching
    let multiplexer = Arc::new(Multiplexer::new());

    // Spawn background task to read messages from TCP connection and feed into multiplexer.
    // This loop never exits — on disconnect it waits for the ping monitor to reconnect,
    // then resumes reading from the new connection (recv() picks up the new FramedRead
    // via Arc<Mutex> on each call).
    let connection_read = connection.clone();
    let multiplexer_response = multiplexer.clone();
    tokio::spawn(async move {
        loop {
            // If connection is down, clean up pending requests and wait for
            // the ping monitor to re-establish the connection.
            if !connection_read.is_healthy() {
                tracing::warn!("Recv loop: connection lost, waiting for reconnection");
                multiplexer_response.cleanup_on_disconnect().await;
                connection_read.wait_for_reconnect().await;
                tracing::info!("Recv loop: reconnection detected, resuming reads");
                continue;
            }

            match connection_read.recv().await {
                Ok(Some(msg)) => {
                    // Handle Pong silently (keepalive response, not a real message)
                    if matches!(&msg, Message::Pong(_)) {
                        tracing::trace!("Received Pong from server");
                        continue;
                    }
                    tracing::debug!("Received message from server");
                    multiplexer_response.handle_response(msg).await;
                }
                Ok(None) => {
                    tracing::warn!("TCP connection closed by server");
                    // recv() already set connected=false; loop back to the
                    // is_healthy() check which handles cleanup + wait.
                }
                Err(e) => {
                    tracing::error!("Error reading from TCP connection: {}", e);
                    // recv() already set connected=false; loop back to the
                    // is_healthy() check which handles cleanup + wait.
                }
            }
        }
    });

    // Spawn ping-based keepalive monitor (replaces simple is_healthy check)
    // Sends a Ping message every 30 seconds; if send fails, connection is dead
    let connection_monitor = connection.clone();
    let server_host = config.server.host.clone();
    let server_port = config.server.port;
    let ping_interval = std::env::var("CARAPACE_PING_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5u64);
    tokio::spawn(async move {
        let mut ping_counter: u64 = 0;
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(ping_interval)).await;

            if !connection_monitor.is_healthy() {
                tracing::warn!(
                    "Connection unhealthy, attempting automatic reconnection to {}:{}",
                    server_host,
                    server_port
                );
                if let Err(e) = connection_monitor.reconnect_if_needed().await {
                    tracing::error!("Auto-reconnection failed: {}", e);
                } else {
                    tracing::info!("Auto-reconnection successful");
                }
                continue;
            }

            // Send Ping to verify connection is actually alive (not just locally marked healthy)
            ping_counter += 1;
            let ping = Message::Ping(PingPong {
                id: format!("ping-{}", ping_counter),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            });
            if let Err(e) = connection_monitor.send(ping).await {
                tracing::warn!("Ping failed (connection likely dead): {}", e);
                // send() already marks connection unhealthy, reconnect will happen next iteration
            } else {
                tracing::trace!("Ping {} sent", ping_counter);
            }
        }
    });

    // Spawn CLI handler (Unix socket)
    let cli_handler = CliHandler::new(
        config.cli_socket.clone(),
        multiplexer.clone(),
        connection.clone(),
    );
    let cli_socket = config.cli_socket.clone();
    tokio::spawn(async move {
        if let Err(e) = cli_handler.listen().await {
            tracing::error!("CLI handler error: {}", e);
        }
    });

    // Spawn HTTP proxy server
    let http_proxy = HttpProxy::new(multiplexer.clone(), connection.clone(), config.http.port);
    tokio::spawn(async move {
        if let Err(e) = http_proxy.listen().await {
            tracing::error!("HTTP proxy error: {}", e);
        }
    });

    tracing::info!(
        "Agent ready: CLI socket at {}, HTTP proxy on port {}",
        cli_socket,
        config.http.port
    );

    // Set up signal handlers for graceful shutdown
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(carapace_agent::error::AgentError::IOError)?;

    let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
        .map_err(carapace_agent::error::AgentError::IOError)?;

    // Wait for shutdown signal
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received SIGINT, shutting down");
        }
        _ = sigterm.recv() => {
            tracing::info!("Received SIGTERM, shutting down");
        }
        _ = sighup.recv() => {
            tracing::info!("Received SIGHUP (reload signal) - not implemented yet, continue running");
            // In future, could reload configuration here
            // For now, just log and continue
        }
    }

    tracing::info!("Agent shutting down gracefully");

    // Clean up resources
    multiplexer.clear().await;
    connection.kill().await.ok(); // Ignore errors during shutdown

    tracing::info!("Agent shutdown complete");
    Ok(())
}
