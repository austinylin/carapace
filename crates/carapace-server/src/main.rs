use carapace_policy::PolicyConfig;
use carapace_server::{CliDispatcher, ConnectionTracker, HttpDispatcher, Listener, Result};
use clap::Parser;
use std::sync::Arc;
use tokio::net::TcpListener;

#[derive(Parser, Debug)]
#[command(name = "carapace-server")]
struct Args {
    /// Listen on TCP socket (e.g., 127.0.0.1:8765) instead of stdin/stdout
    #[arg(long)]
    listen: Option<String>,

    /// Listen on HTTP socket for debug endpoints (e.g., 127.0.0.1:8766)
    #[arg(long)]
    debug_listen: Option<String>,
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

    // Create connection tracker
    let connection_tracker = ConnectionTracker::new();

    // Start debug HTTP server if requested
    if let Some(debug_addr) = args.debug_listen {
        let debug_addr_parsed: std::net::SocketAddr = debug_addr.parse().map_err(|e| {
            carapace_server::ServerError::ConfigError(format!("Invalid debug address: {}", e))
        })?;
        let tracker_clone = connection_tracker.clone();
        tokio::spawn(async move {
            if let Err(e) =
                carapace_server::debug_server::start_debug_server(debug_addr_parsed, tracker_clone)
                    .await
            {
                tracing::error!("Debug server error: {}", e);
            }
        });
    }

    if let Some(listen_addr) = args.listen {
        // TCP mode: listen on socket and accept multiple connections
        tracing::info!("Starting TCP server on {}", listen_addr);
        let listener = TcpListener::bind(&listen_addr).await?;
        tracing::info!("Server listening on {}", listen_addr);

        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    eprintln!("DEBUG: TCP connection accepted from {}", addr);
                    tracing::info!("New connection from {}", addr);
                    let cli_dispatcher = cli_dispatcher.clone();
                    let http_dispatcher = http_dispatcher.clone();
                    let tracker_clone = connection_tracker.clone();

                    eprintln!("DEBUG: About to spawn listener task");
                    tokio::spawn(async move {
                        eprintln!("DEBUG: In spawned task for {}", addr);
                        tracing::info!("Spawned listener task for connection from {}", addr);

                        // Register the connection
                        tracker_clone.register(addr).await;

                        let (read, write) = stream.into_split();
                        eprintln!("DEBUG: Stream split");
                        let listener = Listener::new(cli_dispatcher, http_dispatcher);
                        eprintln!("DEBUG: Listener created");

                        tracing::info!("About to call listener.listen() for {}", addr);
                        eprintln!("DEBUG: About to call listener.listen()");
                        if let Err(e) = listener.listen(read, write).await {
                            eprintln!("DEBUG: Connection error: {}", e);
                            tracing::error!("Connection error for {}: {}", addr, e);
                        }

                        // Unregister the connection
                        tracker_clone.unregister(addr).await;

                        eprintln!("DEBUG: Connection from {} closed", addr);
                        tracing::info!("Connection from {} closed", addr);
                    });
                    eprintln!("DEBUG: Task spawned");
                }
                Err(e) => {
                    tracing::error!("Failed to accept connection: {}", e);
                }
            }
        }
    } else {
        // SSH mode: single connection on stdin/stdout
        tracing::info!("Server ready, listening on stdin/stdout");

        // Register stdin/stdout as a connection for SSH mode
        let stdin_addr: std::net::SocketAddr = "127.0.0.1:0"
            .parse()
            .expect("Failed to parse dummy address");
        connection_tracker.register(stdin_addr).await;

        let listener = Listener::new(cli_dispatcher, http_dispatcher);
        let result = listener
            .listen(tokio::io::stdin(), tokio::io::stdout())
            .await;

        connection_tracker.unregister(stdin_addr).await;
        result?;
    }

    Ok(())
}
