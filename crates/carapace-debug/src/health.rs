use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::time::Instant;

/// Query server health endpoint
pub async fn health(host: &str, port: u16, format: &str) -> Result<()> {
    let url = format!("http://{}:{}/debug/health", host, port);

    let start = Instant::now();

    match reqwest::get(&url).await {
        Ok(resp) => {
            let elapsed = start.elapsed();

            match resp.json::<Value>().await {
                Ok(data) => {
                    match format {
                        "json" => {
                            println!("{}", serde_json::to_string_pretty(&data)?);
                        }
                        _ => {
                            // Text format
                            println!("=== Carapace Server Health ===");
                            println!("Response time: {:.2}ms", elapsed.as_secs_f64() * 1000.0);

                            if let Some(status) = data.get("status").and_then(|v| v.as_str()) {
                                println!("Status: {}", status);
                            }

                            if let Some(metrics) = data.get("metrics").and_then(|v| v.as_object()) {
                                println!("\nMetrics:");
                                for (key, value) in metrics {
                                    println!("  {}: {}", key, value);
                                }
                            }

                            if let Some(connections) =
                                data.get("active_connections").and_then(|v| v.as_u64())
                            {
                                println!("Active connections: {}", connections);
                            }

                            if let Some(uptime) = data.get("uptime_secs").and_then(|v| v.as_u64()) {
                                println!("Uptime: {} seconds", uptime);
                            }
                        }
                    }
                    Ok(())
                }
                Err(e) => {
                    eprintln!("Failed to parse health response: {}", e);
                    Err(anyhow!("Failed to parse health response"))
                }
            }
        }
        Err(e) => {
            if format == "json" {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "status": "error",
                        "error": e.to_string()
                    }))?
                );
            } else {
                eprintln!("Failed to connect to server at {}: {}", url, e);
            }
            Err(anyhow!("Health check failed: {}", e))
        }
    }
}
