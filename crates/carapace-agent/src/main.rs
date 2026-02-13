use std::sync::Arc;
use carapace_agent::{Connection, Multiplexer, CliHandler, HttpProxy, Result as AgentResult};

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

    // Load config (for now, use defaults or env vars)
    let config = carapace_agent::config::AgentConfig::from_env();

    // Establish SSH connection to server
    let connection = Arc::new(
        Connection::connect(
            &config.ssh.host,
            &config.ssh.remote_command,
            &config.ssh.control_socket,
        )
        .await?,
    );

    tracing::info!("SSH connection established to {}", config.ssh.host);

    // Create multiplexer for request/response matching
    let multiplexer = Arc::new(Multiplexer::new());

    // Spawn background task to read messages from SSH connection and feed into multiplexer
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
                    tracing::warn!("SSH connection closed by server");
                    break;
                }
                Err(e) => {
                    tracing::error!("Error reading from SSH connection: {}", e);
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
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

    // Keep the main task alive
    tokio::signal::ctrl_c().await?;
    tracing::info!("Agent shutting down");

    connection.kill().await?;
    Ok(())
}
