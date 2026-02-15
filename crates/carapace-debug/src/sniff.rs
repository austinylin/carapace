use anyhow::{anyhow, Result};
use bytes::BytesMut;
use std::io::Read;
use std::net::TcpStream;
use std::time::Duration;

/// Sniff TCP messages between agent and server
pub async fn sniff(host: &str, port: u16, filter: Option<String>, _max_size: usize) -> Result<()> {
    let addr = format!("{}:{}", host, port);
    println!("Connecting to {} to sniff messages...", addr);

    // Try to connect - this will show if the server is listening
    match TcpStream::connect(&addr) {
        Ok(mut stream) => {
            // Set read timeout so we don't wait forever
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;

            println!("Connected! Listening for messages...\n");
            println!("Tip: Send a message from the agent to see protocol messages");
            println!("Tip: Use --filter HttpRequest to see only specific message types\n");
            println!("{}", "=".repeat(70));

            let mut buffer = BytesMut::with_capacity(4096);
            let mut message_count = 0;

            loop {
                let mut read_buf = [0; 1024];

                match stream.read(&mut read_buf) {
                    Ok(0) => {
                        println!("\nConnection closed");
                        break;
                    }
                    Ok(n) => {
                        buffer.extend_from_slice(&read_buf[..n]);

                        // Try to parse length-prefixed JSON messages
                        while buffer.len() >= 4 {
                            // Read 4-byte length prefix (big-endian)
                            let len_bytes = [buffer[0], buffer[1], buffer[2], buffer[3]];
                            let msg_len = u32::from_be_bytes(len_bytes) as usize;

                            if buffer.len() >= 4 + msg_len {
                                // Extract message
                                let msg_bytes = buffer[4..4 + msg_len].to_vec();
                                let _ = buffer.split_to(4 + msg_len);

                                // Try to parse as JSON
                                if let Ok(msg_str) = String::from_utf8(msg_bytes) {
                                    if let Ok(json) =
                                        serde_json::from_str::<serde_json::Value>(&msg_str)
                                    {
                                        // Apply filter
                                        let msg_type = detect_message_type(&json);

                                        if let Some(ref f) = filter {
                                            if !msg_type.contains(f) {
                                                continue;
                                            }
                                        }

                                        message_count += 1;
                                        print_message(message_count, msg_type, &json);
                                    }
                                }
                            } else {
                                break;
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        // Timeout - still connected, waiting for data
                        continue;
                    }
                    Err(e) => {
                        eprintln!("Error reading from socket: {}", e);
                        break;
                    }
                }
            }

            println!("\nTotal messages captured: {}", message_count);
        }
        Err(e) => {
            eprintln!("Failed to connect to {}: {}", addr, e);
            eprintln!("\nTroubleshooting:");
            eprintln!("  1. Is carapace-server running? Check: systemctl status carapace-server");
            eprintln!(
                "  2. Is it listening on port {}? Check: ss -tlnp | grep {}",
                port, port
            );
            eprintln!("  3. Can you reach it? Try: nc -zv {} {}", host, port);
            return Err(anyhow!("Cannot connect to server"));
        }
    }

    Ok(())
}

fn detect_message_type(json: &serde_json::Value) -> String {
    if json.get("tool").is_some() && json.get("argv").is_some() {
        "CliRequest".to_string()
    } else if json.get("tool").is_some() && json.get("method").is_some() {
        "HttpRequest".to_string()
    } else if json.get("stdout").is_some() {
        "CliResponse".to_string()
    } else if json.get("status").is_some() && json.get("body").is_some() {
        "HttpResponse".to_string()
    } else if json.get("code").is_some() && json.get("message").is_some() {
        "Error".to_string()
    } else if json.get("data").is_some() && json.get("event").is_some() {
        "SseEvent".to_string()
    } else {
        "Unknown".to_string()
    }
}

fn print_message(count: usize, msg_type: String, json: &serde_json::Value) {
    println!("\n[Message #{}] {}", count, msg_type);
    println!("{}", "-".repeat(70));

    match msg_type.as_str() {
        "CliRequest" => {
            if let Some(tool) = json.get("tool").and_then(|v| v.as_str()) {
                println!("Tool: {}", tool);
            }
            if let Some(argv) = json.get("argv").and_then(|v| v.as_array()) {
                let args: Vec<String> = argv
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
                println!("Arguments: {}", args.join(" "));
            }
        }
        "HttpRequest" => {
            if let Some(tool) = json.get("tool").and_then(|v| v.as_str()) {
                println!("Tool: {}", tool);
            }
            if let Some(method) = json.get("method").and_then(|v| v.as_str()) {
                println!("Method: {}", method);
            }
            if let Some(path) = json.get("path").and_then(|v| v.as_str()) {
                println!("Path: {}", path);
            }
            if let Some(body) = json.get("body").and_then(|v| v.as_str()) {
                let preview = if body.len() > 100 {
                    format!("{}...", &body[..100])
                } else {
                    body.to_string()
                };
                println!("Body: {}", preview);
            }
        }
        "CliResponse" => {
            if let Some(exit_code) = json.get("exit_code").and_then(|v| v.as_i64()) {
                println!("Exit code: {}", exit_code);
            }
            if let Some(stdout) = json.get("stdout").and_then(|v| v.as_str()) {
                let preview = if stdout.len() > 100 {
                    format!("{}...", &stdout[..100])
                } else {
                    stdout.to_string()
                };
                println!("Stdout: {}", preview);
            }
        }
        "HttpResponse" => {
            if let Some(status) = json.get("status").and_then(|v| v.as_u64()) {
                println!("Status: {}", status);
            }
            if let Some(body) = json.get("body").and_then(|v| v.as_str()) {
                let preview = if body.len() > 100 {
                    format!("{}...", &body[..100])
                } else {
                    body.to_string()
                };
                println!("Body: {}", preview);
            }
        }
        "Error" => {
            if let Some(code) = json.get("code").and_then(|v| v.as_str()) {
                println!("Error code: {}", code);
            }
            if let Some(message) = json.get("message").and_then(|v| v.as_str()) {
                println!("Error message: {}", message);
            }
        }
        _ => {
            println!(
                "Data: {}",
                serde_json::to_string_pretty(json).unwrap_or_default()
            );
        }
    }
}
