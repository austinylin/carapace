use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use carapace_protocol::{HttpRequest, Message};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::connection::Connection;
use crate::error::Result as AgentResult;
use crate::multiplexer::Multiplexer;
use tokio::time::timeout;

/// HTTP proxy that converts HTTP requests to protocol messages
pub struct HttpProxy {
    multiplexer: Arc<Multiplexer>,
    connection: Arc<Connection>,
    port: u16,
}

impl HttpProxy {
    pub fn new(multiplexer: Arc<Multiplexer>, connection: Arc<Connection>, port: u16) -> Self {
        HttpProxy {
            multiplexer,
            connection,
            port,
        }
    }

    /// Start listening for HTTP requests
    pub async fn listen(&self) -> AgentResult<()> {
        let multiplexer = self.multiplexer.clone();
        let connection = self.connection.clone();

        // Build router with both multiplexer and connection as state
        let app_state = (multiplexer, connection);
        let app = Router::new()
            .route("/api/v1/rpc", post(handle_api_v1_rpc)) // Explicit route for signal-cli RPC
            .route("/api/v1/events", get(handle_events)) // Explicit route for SSE
            .route("/api/v1/check", get(handle_check)) // Explicit route for health check
            .route("/rpc", post(handle_rpc)) // Generic /rpc endpoint
            .route("/api/:tool/:path", post(handle_http)) // Generic tool:path routing
            .fallback(post(handle_fallback)) // Catch-all for other /api/v1/* paths
            .with_state(app_state);

        // Bind to localhost:port
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", self.port)).await?;
        tracing::info!("HTTP proxy listening on 127.0.0.1:{}", self.port);

        axum::serve(listener, app).await?;

        Ok(())
    }
}

// Note: Default impl removed as Connection requires SSH server
// Use explicit HttpProxy::new() instead

/// HTTP proxy state type: (Multiplexer, Connection)
type ProxyState = (Arc<Multiplexer>, Arc<Connection>);

/// Handle JSON-RPC requests
async fn handle_rpc(
    State((multiplexer, connection)): State<ProxyState>,
    request: Request<Body>,
) -> std::result::Result<Response, HttpProxyError> {
    // Extract the original request URI path before reading body
    let path = request.uri().path().to_string();

    // Read body
    let body_bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| HttpProxyError::InvalidBody)?;

    let body_str = String::from_utf8_lossy(&body_bytes).to_string();

    // Parse JSON-RPC to extract tool name (from context or body)
    let json: serde_json::Value =
        serde_json::from_str(&body_str).map_err(|_| HttpProxyError::MalformedJson)?;

    // Extract method to determine tool (simplified - assume body has tool field)
    let tool = json
        .get("tool")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let request_id = Uuid::new_v4().to_string();

    // Remove "tool" field from JSON body (it's for Carapace, not the upstream service)
    let mut json_body = json.clone();
    if let serde_json::Value::Object(ref mut obj) = json_body {
        obj.remove("tool");
    }
    let body_without_tool = serde_json::to_string(&json_body).unwrap_or_else(|_| body_str.clone());

    // Create HttpRequest with original path
    let http_req = HttpRequest {
        id: request_id.clone(),
        tool,
        method: "POST".to_string(),
        path,
        headers: {
            let mut h = HashMap::new();
            h.insert("Content-Type".to_string(), "application/json".to_string());
            h
        },
        body: Some(body_without_tool),
    };

    // Register waiter for response
    let mut rx = multiplexer.register_waiter(request_id).await;

    // Send request to server via SSH connection
    let msg = Message::HttpRequest(http_req);
    connection.send(msg).await.map_err(|e| {
        tracing::error!("Failed to send HTTP request: {}", e);
        HttpProxyError::NoResponse
    })?;

    // Wait for response with 60 second timeout
    let msg = timeout(tokio::time::Duration::from_secs(60), rx.recv())
        .await
        .map_err(|_| HttpProxyError::NoResponse)?;

    match msg {
        Some(Message::HttpResponse(resp)) => {
            // Check if this is an SSE response
            if is_sse_response(&resp.headers) {
                // For SSE, body will be streamed line-by-line
                // Note: Current implementation buffers the entire response
                // For true streaming with very large responses, server should send SseEvent messages
                let events = resp
                    .body
                    .as_deref()
                    .unwrap_or("")
                    .lines()
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");

                Ok((
                    StatusCode::from_u16(resp.status).unwrap_or(StatusCode::OK),
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    events,
                )
                    .into_response())
            } else {
                let body = resp.body.unwrap_or_default();
                Ok((
                    StatusCode::from_u16(resp.status).unwrap_or(StatusCode::OK),
                    body,
                )
                    .into_response())
            }
        }
        Some(_) => Err(HttpProxyError::WrongResponseType),
        None => Err(HttpProxyError::NoResponse),
    }
}

/// Handle signal-cli RPC requests at /api/v1/rpc
/// OpenClaw sends JSON-RPC to this endpoint without a "tool" field,
/// so we hardcode tool="signal-cli" since this endpoint is signal-cli specific
async fn handle_api_v1_rpc(
    State((multiplexer, connection)): State<ProxyState>,
    request: Request<Body>,
) -> std::result::Result<Response, HttpProxyError> {
    // Extract the original request URI path before reading body
    let path = request.uri().path().to_string();

    // Read body
    let body_bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| HttpProxyError::InvalidBody)?;

    let body_str = String::from_utf8_lossy(&body_bytes).to_string();

    // Parse JSON-RPC to extract tool name (from body, or default to signal-cli)
    let json: serde_json::Value =
        serde_json::from_str(&body_str).map_err(|_| HttpProxyError::MalformedJson)?;

    // Extract method to determine tool (or use signal-cli as default since this is /api/v1/rpc)
    let tool = json
        .get("tool")
        .and_then(|v| v.as_str())
        .unwrap_or("signal-cli")
        .to_string();

    let request_id = Uuid::new_v4().to_string();

    // Remove "tool" field from JSON body (it's for Carapace, not the upstream service)
    let mut json_body = json.clone();
    if let serde_json::Value::Object(ref mut obj) = json_body {
        obj.remove("tool");
    }
    let body_without_tool = serde_json::to_string(&json_body).unwrap_or_else(|_| body_str.clone());

    // Create HttpRequest with original path
    let http_req = HttpRequest {
        id: request_id.clone(),
        tool,
        method: "POST".to_string(),
        path,
        headers: {
            let mut h = HashMap::new();
            h.insert("Content-Type".to_string(), "application/json".to_string());
            h
        },
        body: Some(body_without_tool),
    };

    // Register waiter for response
    let mut rx = multiplexer.register_waiter(request_id).await;

    // Send request to server via SSH connection
    let msg = Message::HttpRequest(http_req);
    connection.send(msg).await.map_err(|e| {
        tracing::error!("Failed to send HTTP request: {}", e);
        HttpProxyError::NoResponse
    })?;

    // Wait for response with 60 second timeout
    let msg = timeout(tokio::time::Duration::from_secs(60), rx.recv())
        .await
        .map_err(|_| HttpProxyError::NoResponse)?;

    match msg {
        Some(Message::HttpResponse(resp)) => {
            let body = resp.body.unwrap_or_default();
            Ok((
                StatusCode::from_u16(resp.status).unwrap_or(StatusCode::OK),
                body,
            )
                .into_response())
        }
        Some(_) => Err(HttpProxyError::WrongResponseType),
        None => Err(HttpProxyError::NoResponse),
    }
}

/// Handle generic HTTP requests
async fn handle_http(
    State((multiplexer, connection)): State<ProxyState>,
    request: Request<Body>,
) -> std::result::Result<Response, HttpProxyError> {
    let method = request.method().to_string();
    let path = request.uri().path().to_string();

    // Read body if present
    let body_bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| HttpProxyError::InvalidBody)?;

    let body_str = if body_bytes.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(&body_bytes).to_string())
    };

    let request_id = Uuid::new_v4().to_string();

    // Extract tool from JSON body if present, and strip "tool" field before forwarding
    let (tool, final_body) = if let Some(body) = &body_str {
        if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(body) {
            let tool = json
                .get("tool")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();

            // Remove "tool" field (it's for Carapace, not the upstream service)
            if let serde_json::Value::Object(ref mut obj) = json {
                obj.remove("tool");
            }
            let body_without_tool = serde_json::to_string(&json).unwrap_or_else(|_| body.clone());

            (tool, Some(body_without_tool))
        } else {
            ("unknown".to_string(), Some(body.clone()))
        }
    } else {
        ("unknown".to_string(), None)
    };

    // Create HttpRequest
    let http_req = HttpRequest {
        id: request_id.clone(),
        tool,
        method,
        path,
        headers: HashMap::new(),
        body: final_body,
    };

    // Register waiter for response
    let mut rx = multiplexer.register_waiter(request_id).await;

    // Send request to server via SSH connection
    let msg = Message::HttpRequest(http_req);
    connection.send(msg).await.map_err(|e| {
        tracing::error!("Failed to send HTTP request: {}", e);
        HttpProxyError::NoResponse
    })?;

    // Wait for response with 60 second timeout
    let msg = timeout(tokio::time::Duration::from_secs(60), rx.recv())
        .await
        .map_err(|_| HttpProxyError::NoResponse)?;

    match msg {
        Some(Message::HttpResponse(resp)) => {
            // Check if this is an SSE response
            if is_sse_response(&resp.headers) {
                // For SSE, body will be streamed line-by-line
                // Note: Current implementation buffers the entire response
                // For true streaming with very large responses, server should send SseEvent messages
                let events = resp
                    .body
                    .as_deref()
                    .unwrap_or("")
                    .lines()
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");

                Ok((
                    StatusCode::from_u16(resp.status).unwrap_or(StatusCode::OK),
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    events,
                )
                    .into_response())
            } else {
                let body = resp.body.unwrap_or_default();
                Ok((
                    StatusCode::from_u16(resp.status).unwrap_or(StatusCode::OK),
                    body,
                )
                    .into_response())
            }
        }
        Some(_) => Err(HttpProxyError::WrongResponseType),
        None => Err(HttpProxyError::NoResponse),
    }
}

/// Handle SSE events endpoint (GET /api/v1/events)
async fn handle_events(
    State((multiplexer, connection)): State<ProxyState>,
    request: Request<Body>,
) -> std::result::Result<Response, HttpProxyError> {
    let path = request.uri().path().to_string();
    let query_string = request
        .uri()
        .query()
        .map(|q| format!("?{}", q))
        .unwrap_or_default();
    let full_path = format!("{}{}", path, query_string);

    let request_id = Uuid::new_v4().to_string();

    // Create HttpRequest for GET request
    let http_req = HttpRequest {
        id: request_id.clone(),
        tool: "signal-cli".to_string(),
        method: "GET".to_string(),
        path: full_path,
        headers: HashMap::new(),
        body: None,
    };

    // Register waiter for response
    let mut rx = multiplexer.register_waiter(request_id).await;

    // Send request to server
    let msg = Message::HttpRequest(http_req);
    connection.send(msg).await.map_err(|e| {
        tracing::error!("Failed to send HTTP request: {}", e);
        HttpProxyError::NoResponse
    })?;

    // Wait for response with 300 second timeout (long-lived SSE connection)
    let msg = timeout(tokio::time::Duration::from_secs(300), rx.recv())
        .await
        .map_err(|_| HttpProxyError::NoResponse)?;

    match msg {
        Some(Message::HttpResponse(resp)) => {
            let body = resp.body.unwrap_or_default();
            Ok((
                StatusCode::from_u16(resp.status).unwrap_or(StatusCode::OK),
                [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                body,
            )
                .into_response())
        }
        Some(_) => Err(HttpProxyError::WrongResponseType),
        None => Err(HttpProxyError::NoResponse),
    }
}

/// Handle health check endpoint (GET /api/v1/check)
async fn handle_check() -> std::result::Result<Response, HttpProxyError> {
    Ok((StatusCode::OK, "OK").into_response())
}

/// Detect if response is SSE (Server-Sent Events)
fn is_sse_response(headers: &HashMap<String, String>) -> bool {
    headers
        .get("Content-Type")
        .map(|ct| ct.contains("text/event-stream"))
        .unwrap_or(false)
}

/// Error types for HTTP proxy
#[derive(Debug)]
enum HttpProxyError {
    InvalidBody,
    MalformedJson,
    WrongResponseType,
    NoResponse,
}

impl IntoResponse for HttpProxyError {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            HttpProxyError::InvalidBody => (StatusCode::BAD_REQUEST, "Invalid request body"),
            HttpProxyError::MalformedJson => (StatusCode::BAD_REQUEST, "Malformed JSON"),
            HttpProxyError::WrongResponseType => {
                (StatusCode::INTERNAL_SERVER_ERROR, "Wrong response type")
            }
            HttpProxyError::NoResponse => (StatusCode::GATEWAY_TIMEOUT, "No response from server"),
        };

        (status, error_message).into_response()
    }
}

/// Fallback handler for paths like /api/v1/rpc that aren't explicitly routed
async fn handle_fallback(
    State((multiplexer, connection)): State<ProxyState>,
    request: Request<Body>,
) -> std::result::Result<Response, HttpProxyError> {
    // Extract the original request URI path
    let path = request.uri().path().to_string();

    // Read body
    let body_bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| HttpProxyError::InvalidBody)?;

    let body_str = String::from_utf8_lossy(&body_bytes).to_string();

    // Parse JSON-RPC to extract tool name from body
    let json: serde_json::Value =
        serde_json::from_str(&body_str).map_err(|_| HttpProxyError::MalformedJson)?;

    // Extract tool from JSON body (e.g., {"tool": "signal-cli", ...})
    let tool = json
        .get("tool")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let request_id = Uuid::new_v4().to_string();
    let method = "POST".to_string();

    // Remove "tool" field from JSON body (it's for Carapace, not the upstream service)
    let mut json_body = json.clone();
    if let serde_json::Value::Object(ref mut obj) = json_body {
        obj.remove("tool");
    }
    let body_without_tool = serde_json::to_string(&json_body).unwrap_or_else(|_| body_str.clone());

    // Create HttpRequest with tool from body and original path
    let http_req = HttpRequest {
        id: request_id.clone(),
        tool,
        method,
        path,
        headers: {
            let mut h = HashMap::new();
            h.insert("Content-Type".to_string(), "application/json".to_string());
            h
        },
        body: Some(body_without_tool),
    };

    // Register waiter for response
    let mut rx = multiplexer.register_waiter(request_id).await;

    // Send request to server via connection
    let msg = Message::HttpRequest(http_req);
    connection.send(msg).await.map_err(|e| {
        tracing::error!("Failed to send HTTP request: {}", e);
        HttpProxyError::NoResponse
    })?;

    // Wait for response with 60 second timeout
    let msg = timeout(tokio::time::Duration::from_secs(60), rx.recv())
        .await
        .map_err(|_| HttpProxyError::NoResponse)?;

    match msg {
        Some(Message::HttpResponse(resp)) => {
            let body = resp.body.unwrap_or_default();
            Ok((
                StatusCode::from_u16(resp.status).unwrap_or(StatusCode::OK),
                body,
            )
                .into_response())
        }
        Some(_) => Err(HttpProxyError::WrongResponseType),
        None => Err(HttpProxyError::NoResponse),
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_http_proxy_state_type() {
        // Note: Can't easily test HttpProxy without real Connection
        // Real testing is done in integration tests
        // This test just verifies the module compiles
    }
}
