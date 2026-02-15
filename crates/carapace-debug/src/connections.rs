use anyhow::{anyhow, Result};
use std::time::Duration;
use tokio::time::sleep;

/// Show active connections
pub async fn connections(
    host: &str,
    port: u16,
    watch_interval: Option<u64>,
    format: &str,
) -> Result<()> {
    loop {
        let url = format!("http://{}:{}/debug/connections", host, port);

        match reqwest::get(&url).await {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(data) => {
                    if format == "json" {
                        println!("{}", serde_json::to_string_pretty(&data)?);
                    } else {
                        // Text format with table
                        print_connections_table(&data);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to parse connections response: {}", e);
                }
            },
            Err(e) => {
                eprintln!("Failed to connect: {}", e);
                if watch_interval.is_none() {
                    return Err(anyhow!("Failed to connect to server"));
                }
            }
        }

        if let Some(interval) = watch_interval {
            println!("\n--- Refresh in {} seconds (Ctrl+C to exit) ---", interval);
            sleep(Duration::from_secs(interval)).await;
            // Clear screen
            println!("\x1B[2J\x1B[H");
        } else {
            break;
        }
    }

    Ok(())
}

fn print_connections_table(data: &serde_json::Value) {
    println!("=== Active Connections ===");

    if let Some(connections) = data.get("connections").and_then(|v| v.as_array()) {
        if connections.is_empty() {
            println!("No active connections");
            return;
        }

        println!(
            "{:<20} {:<20} {:<15} {:<15}",
            "Remote", "State", "Messages", "Bytes RX/TX"
        );
        println!("{}", "-".repeat(70));

        for conn in connections {
            let remote = conn
                .get("remote_addr")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let state = conn
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let messages = conn
                .get("message_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let bytes_rx = conn
                .get("bytes_received")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let bytes_tx = conn.get("bytes_sent").and_then(|v| v.as_u64()).unwrap_or(0);

            println!(
                "{:<20} {:<20} {:<15} {}/{}",
                remote, state, messages, bytes_rx, bytes_tx
            );
        }
    } else {
        println!("No connection data available");
    }
}
