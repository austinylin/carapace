use std::sync::Arc;
use carapace_server::{Listener, CliDispatcher};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    tracing::info!("carapace-server starting");

    // Create CLI dispatcher with empty policy for now
    let dispatcher = Arc::new(CliDispatcher::new());

    // Create listener
    let listener = Listener::new(dispatcher);

    // Listen on stdin/stdout
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    listener.listen(stdin, stdout).await?;

    Ok(())
}
