use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod audit;
mod connections;
mod health;
mod policy;

#[derive(Parser)]
#[command(name = "carapace-debug")]
#[command(about = "Debugging toolkit for Carapace - efficient visibility into system state")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Query server health and metrics
    Health {
        /// Server host (default: localhost)
        #[arg(long, default_value = "localhost")]
        host: String,

        /// Debug server port (default: 8766)
        #[arg(long, default_value = "8766")]
        port: u16,

        /// Output format: json, text (default: text)
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// Show active connections
    Connections {
        /// Server host (default: localhost)
        #[arg(long, default_value = "localhost")]
        host: String,

        /// Debug server port (default: 8766)
        #[arg(long, default_value = "8766")]
        port: u16,

        /// Watch mode: refresh every N seconds
        #[arg(long)]
        watch: Option<u64>,

        /// Output format: json, text (default: text)
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// Query audit logs
    Audit {
        /// Audit log file (default: /var/log/carapace/audit.log)
        #[arg(long, default_value = "/var/log/carapace/audit.log")]
        file: PathBuf,

        /// Filter by tool name
        #[arg(long)]
        tool: Option<String>,

        /// Filter by action type (cli, http)
        #[arg(long)]
        action: Option<String>,

        /// Filter by policy result (allow, deny)
        #[arg(long)]
        result: Option<String>,

        /// Time range: "5m", "1h", "24h" (default: all)
        #[arg(long)]
        since: Option<String>,

        /// Follow mode: tail new entries
        #[arg(long)]
        follow: bool,

        /// Output format: json, text (default: text)
        #[arg(long, default_value = "text")]
        format: String,

        /// Limit number of results
        #[arg(long, default_value = "50")]
        limit: usize,
    },

    /// Test policy decisions
    Policy {
        /// Policy file to test
        policy: PathBuf,

        /// Request JSON file or inline JSON
        request: String,

        /// Output format: json, text (default: text)
        #[arg(long, default_value = "text")]
        format: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Health { host, port, format } => {
            health::health(&host, port, &format).await?;
        }
        Commands::Connections {
            host,
            port,
            watch,
            format,
        } => {
            connections::connections(&host, port, watch, &format).await?;
        }
        Commands::Audit {
            file,
            tool,
            action,
            result,
            since,
            follow,
            format,
            limit,
        } => {
            audit::audit(&file, tool, action, result, since, follow, &format, limit).await?;
        }
        Commands::Policy {
            policy,
            request,
            format,
        } => {
            policy::policy(&policy, &request, &format).await?;
        }
    }

    Ok(())
}
