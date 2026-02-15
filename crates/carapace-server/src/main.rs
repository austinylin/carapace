use carapace_policy::PolicyConfig;
use carapace_server::{CliDispatcher, HttpDispatcher, Listener, Result};
use clap::Parser;
use std::sync::Arc;
use tokio::net::TcpListener;

#[derive(Parser, Debug)]
#[command(name = "carapace-server")]
struct Args {
    /// Listen on TCP socket (e.g., 127.0.0.1:8765) instead of stdin/stdout
    #[arg(long)]
    listen: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    tracing::info!("carapace-server starting");

    // Load policy from YAML file
    let policy_file = std::env::var("CARAPACE_POLICY_FILE")
        .unwrap_or_else(|_| "/etc/carapace/policy.yaml".to_string());

    tracing::info!("Loading policy from: {}", policy_file);
    let policy = PolicyConfig::from_file(&policy_file)
        .map_err(|e| carapace_server::error::ServerError::ConfigError(e.to_string()))?;
    tracing::info!(
        "Policy loaded successfully: {} tools configured",
        policy.tools.len()
    );

    // Create CLI dispatcher with policy
    let cli_dispatcher = Arc::new(CliDispatcher::with_policy(policy.clone()));

    // Create HTTP dispatcher with policy
    let http_dispatcher = Arc::new(HttpDispatcher::with_policy(policy));

    if let Some(listen_addr) = args.listen {
        // TCP mode: listen on socket and accept multiple connections
        tracing::info!("Starting TCP server on {}", listen_addr);
        let listener = TcpListener::bind(&listen_addr).await?;
        tracing::info!("Server listening on {}", listen_addr);

        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    tracing::info!("New connection from {}", addr);
                    let cli_dispatcher = cli_dispatcher.clone();
                    let http_dispatcher = http_dispatcher.clone();

                    tokio::spawn(async move {
                        let (read, write) = stream.into_split();
                        let listener = Listener::new(cli_dispatcher, http_dispatcher);

                        if let Err(e) = listener.listen(read, write).await {
                            tracing::error!("Connection error: {}", e);
                        }
                        tracing::info!("Connection from {} closed", addr);
                    });
                }
                Err(e) => {
                    tracing::error!("Failed to accept connection: {}", e);
                }
            }
        }
    } else {
        // SSH mode: single connection on stdin/stdout
        tracing::info!("Server ready, listening on stdin/stdout");
        let listener = Listener::new(cli_dispatcher, http_dispatcher);
        listener
            .listen(tokio::io::stdin(), tokio::io::stdout())
            .await?;
    }

    Ok(())
}
