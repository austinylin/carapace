use carapace_protocol::{Message, HttpRequest, HttpResponse};
use std::sync::Arc;
use axum::{
    extract::State,
    http::{Request, StatusCode},
    body::Body,
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use uuid::Uuid;
use std::collections::HashMap;

use crate::multiplexer::Multiplexer;
use crate::error::Result as AgentResult;

/// HTTP proxy that converts HTTP requests to protocol messages
pub struct HttpProxy {
    multiplexer: Arc<Multiplexer>,
    port: u16,
}

impl HttpProxy {
    pub fn new(multiplexer: Arc<Multiplexer>, port: u16) -> Self {
        HttpProxy {
            multiplexer,
            port,
        }
    }

    /// Start listening for HTTP requests
    pub async fn listen(&self) -> AgentResult<()> {
        let multiplexer = self.multiplexer.clone();

        // Build router
        let app = Router::new()
            .route("/rpc", post(handle_rpc))
            .route("/api/:tool/:path", post(handle_http))
            .with_state(multiplexer);

        // Bind to localhost:port
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", self.port)).await?;
        tracing::info!("HTTP proxy listening on 127.0.0.1:{}", self.port);

        axum::serve(listener, app).await?;

        Ok(())
    }
}

impl Default for HttpProxy {
    fn default() -> Self {
        Self::new(Arc::new(crate::multiplexer::Multiplexer::new()), 8080)
    }
}

/// Handle JSON-RPC requests
async fn handle_rpc(
    State(multiplexer): State<Arc<Multiplexer>>,
    request: Request<Body>,
) -> std::result::Result<Response, HttpProxyError> {
    // Read body
    let body_bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| HttpProxyError::InvalidBody)?;

    let body_str = String::from_utf8_lossy(&body_bytes).to_string();

    // Parse JSON-RPC to extract tool name (from context or body)
    let json: serde_json::Value = serde_json::from_str(&body_str)
        .map_err(|_| HttpProxyError::MalformedJson)?;

    // Extract method to determine tool (simplified - assume body has tool field)
    let tool = json.get("tool")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let request_id = Uuid::new_v4().to_string();

    // Create HttpRequest
    let http_req = HttpRequest {
        id: request_id.clone(),
        tool,
        method: "POST".to_string(),
        path: "/rpc".to_string(),
        headers: {
            let mut h = HashMap::new();
            h.insert("Content-Type".to_string(), "application/json".to_string());
            h
        },
        body: Some(body_str),
    };

    // Register waiter for response
    let rx = multiplexer.register_waiter(request_id).await;

    // Send request
    // In real implementation, would send to server via connection
    // For now, simulate response
    tokio::spawn({
        let multiplexer = multiplexer.clone();
        async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

            let resp = Message::HttpResponse(HttpResponse {
                id: http_req.id,
                status: 200,
                headers: {
                    let mut h = HashMap::new();
                    h.insert("Content-Type".to_string(), "application/json".to_string());
                    h
                },
                body: Some(r#"{"jsonrpc":"2.0","result":"ok"}"#.to_string()),
            });

            multiplexer.handle_response(resp).await;
        }
    });

    // Wait for response
    match rx.await {
        Ok(Message::HttpResponse(resp)) => {
            // Check if this is an SSE response
            if is_sse_response(&resp.headers) {
                // For SSE, body will be streamed line-by-line
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
    State(multiplexer): State<Arc<Multiplexer>>,
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

    // Simulate response
    tokio::spawn({
        let multiplexer = multiplexer.clone();
        async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

            let resp = Message::HttpResponse(HttpResponse {
                id: http_req.id,
                status: 200,
                headers: HashMap::new(),
                body: Some("OK".to_string()),
            });

            multiplexer.handle_response(resp).await;
        }
    });

    // Wait for response
    match rx.await {
        Ok(Message::HttpResponse(resp)) => {
            // Check if this is an SSE response
            if is_sse_response(&resp.headers) {
                // For SSE, body will be streamed line-by-line
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
            HttpProxyError::WrongResponseType => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Wrong response type",
            ),
            HttpProxyError::NoResponse => (
                StatusCode::GATEWAY_TIMEOUT,
                "No response from server",
            ),
        };

        (status, error_message).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_proxy_creation() {
        let _proxy = HttpProxy::default();
    }

    #[test]
    fn test_http_proxy_with_custom_port() {
        let multiplexer = Arc::new(Multiplexer::new());
        let proxy = HttpProxy::new(multiplexer, 9090);
        assert_eq!(proxy.port, 9090);
    }
}
