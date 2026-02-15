use carapace_policy::PolicyConfig;
use carapace_server::{
    AuditLogger, CliDispatcher, ConnectionTracker, HttpDispatcher, Listener, RateLimiter, Result,
};
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

/// Parse a u32 from an env var with a default
fn env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Parse a u64 from an env var with a default
fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
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

    // Create audit logger (configurable via env)
    let audit_log_file = std::env::var("CARAPACE_AUDIT_LOG").unwrap_or_else(|_| String::new());
    let audit_logger = Arc::new(if audit_log_file.is_empty() {
        AuditLogger::new()
    } else {
        tracing::info!("Audit logging to file: {}", audit_log_file);
        AuditLogger::with_config(
            true,
            true,
            false,
            Some(audit_log_file),
            100 * 1024 * 1024,
            10,
        )
    });

    // Create rate limiter (configurable via env)
    let rate_max = env_u32("CARAPACE_RATE_LIMIT_MAX", 1000);
    let rate_window = env_u64("CARAPACE_RATE_LIMIT_WINDOW_SECS", 60);
    let rate_limiter = Arc::new(RateLimiter::new(rate_max, rate_window));
    tracing::info!(
        "Rate limiter: {} requests per {} seconds per tool",
        rate_max,
        rate_window
    );

    // Connection limit (configurable via env)
    let max_connections = env_u32("CARAPACE_MAX_CONNECTIONS", 100) as usize;
    tracing::info!("Max concurrent connections: {}", max_connections);

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

        // Shutdown signal: broadcast channel notifies all connection tasks
        let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

        // Set up signal handlers
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .map_err(carapace_server::ServerError::IOError)?;

        loop {
            tokio::select! {
                // Accept new connections
                result = listener.accept() => {
                    match result {
                        Ok((stream, addr)) => {
                            // Enforce connection limit
                            let current = connection_tracker.count().await;
                            if current >= max_connections {
                                tracing::warn!(
                                    "Connection limit reached ({}/{}), rejecting {}",
                                    current, max_connections, addr
                                );
                                drop(stream);
                                continue;
                            }

                            tracing::info!("New connection from {} ({}/{})", addr, current + 1, max_connections);
                            let cli_dispatcher = cli_dispatcher.clone();
                            let http_dispatcher = http_dispatcher.clone();
                            let tracker_clone = connection_tracker.clone();
                            let audit_logger = audit_logger.clone();
                            let rate_limiter = rate_limiter.clone();
                            let mut shutdown_rx = shutdown_tx.subscribe();

                            tokio::spawn(async move {
                                tracker_clone.register(addr).await;

                                let (read, write) = stream.into_split();
                                let conn_listener = Listener::with_audit_and_rate_limit(
                                    cli_dispatcher,
                                    http_dispatcher,
                                    audit_logger,
                                    rate_limiter,
                                );

                                // Run connection until it closes or shutdown signal received
                                tokio::select! {
                                    result = conn_listener.listen(read, write) => {
                                        if let Err(e) = result {
                                            tracing::error!("Connection error for {}: {}", addr, e);
                                        }
                                    }
                                    _ = shutdown_rx.recv() => {
                                        tracing::info!("Shutting down connection for {}", addr);
                                    }
                                }

                                tracker_clone.unregister(addr).await;
                                tracing::info!("Connection from {} closed", addr);
                            });
                        }
                        Err(e) => {
                            tracing::error!("Failed to accept connection: {}", e);
                        }
                    }
                }

                // Graceful shutdown on SIGINT
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("Received SIGINT, initiating graceful shutdown");
                    break;
                }

                // Graceful shutdown on SIGTERM
                _ = sigterm.recv() => {
                    tracing::info!("Received SIGTERM, initiating graceful shutdown");
                    break;
                }
            }
        }

        // Signal all connections to shut down
        tracing::info!("Broadcasting shutdown to all connections");
        let _ = shutdown_tx.send(());

        // Wait for connections to drain (with timeout)
        let drain_timeout = env_u64("CARAPACE_SHUTDOWN_TIMEOUT_SECS", 30);
        tracing::info!(
            "Waiting up to {} seconds for {} active connections to drain",
            drain_timeout,
            connection_tracker.count().await
        );
        let deadline = tokio::time::sleep(tokio::time::Duration::from_secs(drain_timeout));
        tokio::pin!(deadline);

        loop {
            let remaining = connection_tracker.count().await;
            if remaining == 0 {
                tracing::info!("All connections drained");
                break;
            }

            tokio::select! {
                _ = &mut deadline => {
                    tracing::warn!(
                        "Shutdown timeout reached, {} connections still active",
                        remaining
                    );
                    break;
                }
                _ = tokio::time::sleep(tokio::time::Duration::from_millis(250)) => {
                    // Check again
                }
            }
        }

        tracing::info!("Server shutdown complete");
    } else {
        // SSH mode: single connection on stdin/stdout
        tracing::info!("Server ready, listening on stdin/stdout");

        // Register stdin/stdout as a connection for SSH mode
        let stdin_addr: std::net::SocketAddr = "127.0.0.1:0"
            .parse()
            .expect("Failed to parse dummy address");
        connection_tracker.register(stdin_addr).await;

        let conn_listener = Listener::with_audit_and_rate_limit(
            cli_dispatcher,
            http_dispatcher,
            audit_logger,
            rate_limiter,
        );
        let result = conn_listener
            .listen(tokio::io::stdin(), tokio::io::stdout())
            .await;

        connection_tracker.unregister(stdin_addr).await;
        result?;
    }

    Ok(())
}
