use anyhow::Result;
use serde_json::json;
use std::net::SocketAddr;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// Start HTTP debug server on specified address
pub async fn start_debug_server(addr: SocketAddr) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;

    tracing::info!("Debug HTTP server listening on {}", addr);

    loop {
        let (socket, _) = listener.accept().await?;
        tokio::spawn(async move {
            if let Err(e) = handle_client(socket).await {
                eprintln!("Error handling debug client: {}", e);
            }
        });
    }
}

/// Handle a single HTTP client connection
async fn handle_client(socket: TcpStream) -> Result<()> {
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
        ("GET", "/debug/health") => create_json_response(handle_health()),
        ("GET", "/debug/connections") => create_json_response(handle_connections()),
        ("GET", "/debug/audit") => create_json_response(handle_audit()),
        ("POST", "/debug/policy") => create_json_response(handle_policy()),
        _ => create_json_response(json!({"error": "Not found"})),
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
fn handle_health() -> serde_json::Value {
    json!({
        "status": "healthy",
        "uptime_secs": 0,
        "active_connections": 0,
        "metrics": {
            "requests_processed": 0,
            "errors": 0
        }
    })
}

/// Handle GET /debug/connections
fn handle_connections() -> serde_json::Value {
    json!({
        "connections": []
    })
}

/// Handle GET /debug/audit
fn handle_audit() -> serde_json::Value {
    json!({
        "entries": [],
        "total": 0
    })
}

/// Handle POST /debug/policy
fn handle_policy() -> serde_json::Value {
    json!({
        "decision": "not_implemented",
        "reason": "Policy evaluation endpoint not yet implemented"
    })
}
