use carapace_agent::{CliHandler, Connection, HttpProxy, Multiplexer, Result as AgentResult};
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

    // Spawn background task to read messages from TCP connection and feed into multiplexer
    let connection_read = connection.clone();
    let multiplexer_response = multiplexer.clone();
    tokio::spawn(async move {
        loop {
            match connection_read.recv().await {
                Ok(Some(msg)) => {
                    tracing::debug!("Received message from server");
                    multiplexer_response.handle_response(msg).await;
                }
                Ok(None) => {
                    tracing::warn!("TCP connection closed by server");
                    multiplexer_response.cleanup_on_disconnect().await;
                    break;
                }
                Err(e) => {
                    tracing::error!("Error reading from TCP connection: {}", e);
                    multiplexer_response.cleanup_on_disconnect().await;
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                }
            }
        }
    });

    // Spawn connection health monitor for automatic reconnection
    let connection_monitor = connection.clone();
    let server_host = config.server.host.clone();
    let server_port = config.server.port;
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

            if !connection_monitor.is_healthy().await {
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
