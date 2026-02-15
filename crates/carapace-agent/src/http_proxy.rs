use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
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
            .route("/rpc", post(handle_rpc))
            .route("/api/:tool/:path", post(handle_http))
            .fallback(post(handle_fallback)) // Catch-all for other paths like /api/v1/rpc
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
        body: Some(body_str),
    };

    // Register waiter for response
    let rx = multiplexer.register_waiter(request_id).await;

    // Send request to server via SSH connection
    let msg = Message::HttpRequest(http_req);
    connection.send(msg).await.map_err(|e| {
        tracing::error!("Failed to send HTTP request: {}", e);
        HttpProxyError::NoResponse
    })?;

    // Wait for response with 60 second timeout
    // Note: For true streaming SSE, server should send SseEvent messages instead of buffering response
    let response_result = timeout(tokio::time::Duration::from_secs(60), rx)
        .await
        .map_err(|_| HttpProxyError::NoResponse)?;

    match response_result {
        Ok(Message::HttpResponse(resp)) => {
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
        Ok(_) => Err(HttpProxyError::WrongResponseType),
        Err(_) => Err(HttpProxyError::NoResponse),
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

    // Extract tool from path (simplified)
    let tool = "unknown".to_string();

    // Create HttpRequest
    let http_req = HttpRequest {
        id: request_id.clone(),
        tool,
        method,
        path,
        headers: HashMap::new(),
        body: body_str,
    };

    // Register waiter for response
    let rx = multiplexer.register_waiter(request_id).await;

    // Send request to server via SSH connection
    let msg = Message::HttpRequest(http_req);
    connection.send(msg).await.map_err(|e| {
        tracing::error!("Failed to send HTTP request: {}", e);
        HttpProxyError::NoResponse
    })?;

    // Wait for response with 60 second timeout
    // Note: For true streaming SSE, server should send SseEvent messages instead of buffering response
    let response_result = timeout(tokio::time::Duration::from_secs(60), rx)
        .await
        .map_err(|_| HttpProxyError::NoResponse)?;

    match response_result {
        Ok(Message::HttpResponse(resp)) => {
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
        Ok(_) => Err(HttpProxyError::WrongResponseType),
        Err(_) => Err(HttpProxyError::NoResponse),
    }
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
        body: Some(body_str),
    };

    // Register waiter for response
    let rx = multiplexer.register_waiter(request_id).await;

    // Send request to server via connection
    let msg = Message::HttpRequest(http_req);
    connection.send(msg).await.map_err(|e| {
        tracing::error!("Failed to send HTTP request: {}", e);
        HttpProxyError::NoResponse
    })?;

    // Wait for response with 60 second timeout
    let response_result = timeout(tokio::time::Duration::from_secs(60), rx)
        .await
        .map_err(|_| HttpProxyError::NoResponse)?;

    match response_result {
        Ok(Message::HttpResponse(resp)) => {
            let body = resp.body.unwrap_or_default();
            Ok((
                StatusCode::from_u16(resp.status).unwrap_or(StatusCode::OK),
                body,
            )
                .into_response())
        }
        Ok(_) => Err(HttpProxyError::WrongResponseType),
        Err(_) => Err(HttpProxyError::NoResponse),
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
