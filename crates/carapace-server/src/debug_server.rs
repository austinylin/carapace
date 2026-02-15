use anyhow::Result;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::ConnectionTracker;

/// Start HTTP debug server on specified address
pub async fn start_debug_server(
    addr: SocketAddr,
    connection_tracker: Arc<ConnectionTracker>,
) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;

    tracing::info!("Debug HTTP server listening on {}", addr);

    loop {
        let (socket, _) = listener.accept().await?;
        let tracker = connection_tracker.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(socket, tracker).await {
                eprintln!("Error handling debug client: {}", e);
            }
        });
    }
}

/// Handle a single HTTP client connection
async fn handle_client(
    socket: TcpStream,
    connection_tracker: Arc<ConnectionTracker>,
) -> Result<()> {
    let (reader, mut writer) = socket.into_split();
    let mut bufreader = BufReader::new(reader);
    let mut request_line = String::new();

    // Read the HTTP request line
    bufreader.read_line(&mut request_line).await?;

    // Parse request line: "GET /debug/health HTTP/1.1"
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return Ok(());
    }

    let method = parts[0];
    let path = parts[1];

    // Read headers until blank line
    let mut headers = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        bufreader.read_line(&mut line).await?;
        if line == "\n" || line == "\r\n" {
            break;
        }
        headers.push_str(&line);
    }

    // Generate response based on path
    let response = match (method, path) {
        ("GET", "/debug/health") => create_json_response(handle_health(&connection_tracker).await),
        ("GET", "/debug/connections") => {
            create_json_response(handle_connections(&connection_tracker).await)
        }
        _ => create_not_found_response(path),
    };

    writer.write_all(response.as_bytes()).await?;
    writer.flush().await?;

    Ok(())
}

/// Create an HTTP response with JSON body
fn create_json_response(body: serde_json::Value) -> String {
    let body_str = body.to_string();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body_str.len(),
        body_str
    )
}

/// Handle GET /debug/health
async fn handle_health(connection_tracker: &ConnectionTracker) -> serde_json::Value {
    let active_connections = connection_tracker.count().await;
    json!({
        "status": "healthy",
        "uptime_secs": 0,
        "active_connections": active_connections,
        "metrics": {
            "requests_processed": 0,
            "errors": 0
        }
    })
}

/// Handle GET /debug/connections
async fn handle_connections(connection_tracker: &ConnectionTracker) -> serde_json::Value {
    let connections = connection_tracker.get_all().await;
    let connection_list: Vec<serde_json::Value> = connections
        .iter()
        .map(|conn| {
            json!({
                "remote_addr": conn.remote_addr.to_string(),
                "connected_at": conn.connected_at.to_rfc3339()
            })
        })
        .collect();

    json!({
        "connections": connection_list
    })
}

/// Create a 404 Not Found response
fn create_not_found_response(path: &str) -> String {
    let body = json!({"error": "Not found", "path": path}).to_string();
    format!(
        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )
}
