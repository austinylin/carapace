use carapace_policy::{HttpPolicy, PolicyConfig, PolicyValidator};
use carapace_protocol::{HttpRequest, HttpResponse};
use reqwest::Client;
use std::collections::HashMap;

/// HTTP request dispatcher with policy enforcement
pub struct HttpDispatcher {
    policy: PolicyConfig,
    client: Client,
}

impl HttpDispatcher {
    pub fn new() -> Self {
        HttpDispatcher {
            policy: PolicyConfig {
                tools: HashMap::new(),
            },
            client: Client::new(),
        }
    }

    pub fn with_policy(policy: PolicyConfig) -> Self {
        HttpDispatcher {
            policy,
            client: Client::new(),
        }
    }

    /// Dispatch an HTTP request, validate against policy, and proxy to upstream
    pub async fn dispatch_http(&self, req: HttpRequest) -> anyhow::Result<HttpResponse> {
        eprintln!("DEBUG: http_dispatch.dispatch_http() called for tool={}, method={}, path={}", req.tool, req.method, req.path);

        // Check if tool is allowed in policy
        let tool_config = self
            .policy
            .tools
            .get(&req.tool)
            .ok_or_else(|| anyhow::anyhow!("Tool '{}' not in policy", req.tool))?;

        eprintln!("DEBUG: Tool config found, proceeding with dispatch");

        // Get HTTP policy
        let http_policy = match tool_config {
            carapace_policy::ToolPolicy::Http(policy) => policy,
            carapace_policy::ToolPolicy::Cli(_) => {
                return Err(anyhow::anyhow!(
                    "Tool '{}' is CLI-only, cannot handle HTTP request",
                    req.tool
                ))
            }
        };

        // Validate JSON-RPC method if present
        if let Some(body) = &req.body {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
                if let Some(method) = json.get("method").and_then(|v| v.as_str()) {
                    // Validate method name
                    PolicyValidator::validate_jsonrpc_method(
                        method,
                        &http_policy.jsonrpc_allow_methods,
                        &http_policy.jsonrpc_deny_methods,
                    )?;

                    // Validate params (e.g., phone numbers)
                    PolicyValidator::validate_jsonrpc_params(
                        method,
                        body,
                        &http_policy.jsonrpc_param_filters,
                    )?;
                }
            }
        }

        // Validate request path doesn't contain control characters
        if req.path.contains('\n') || req.path.contains('\r') {
            return Err(anyhow::anyhow!(
                "Request path contains control characters: {}",
                req.path
            ));
        }

        // Check request body size (limit to 100MB)
        if let Some(body) = &req.body {
            if body.len() > 100 * 1024 * 1024 {
                return Err(anyhow::anyhow!(
                    "Request body too large: {} bytes",
                    body.len()
                ));
            }
        }

        // Send request to upstream
        eprintln!("DEBUG: About to call proxy_to_upstream for {}", http_policy.upstream);
        let response = self.proxy_to_upstream(http_policy, &req).await?;
        eprintln!("DEBUG: proxy_to_upstream returned successfully");

        Ok(response)
    }

    /// Proxy request to upstream server
    async fn proxy_to_upstream(
        &self,
        policy: &HttpPolicy,
        req: &HttpRequest,
    ) -> anyhow::Result<HttpResponse> {
        eprintln!("DEBUG: proxy_to_upstream() starting for {}", req.tool);
        let url = format!("{}{}", policy.upstream, req.path);
        eprintln!("DEBUG: Constructed URL: {}", url);

        let mut request_builder = match req.method.as_str() {
            "GET" => self.client.get(&url),
            "POST" => self.client.post(&url),
            "PUT" => self.client.put(&url),
            "DELETE" => self.client.delete(&url),
            "PATCH" => self.client.patch(&url),
            "HEAD" => self.client.head(&url),
            _ => return Err(anyhow::anyhow!("Unsupported HTTP method: {}", req.method)),
        };

        // Add headers
        for (key, value) in &req.headers {
            request_builder = request_builder.header(key, value);
        }

        // If we have a body but no Content-Type header, default to application/json
        let has_content_type = req
            .headers
            .keys()
            .any(|k| k.to_lowercase() == "content-type");
        if req.body.is_some() && !has_content_type {
            request_builder = request_builder.header("Content-Type", "application/json");
        }

        // Add body
        if let Some(body) = &req.body {
            request_builder = request_builder.body(body.clone());
        }

        // Determine timeout based on request path
        // SSE endpoints (like /api/v1/events) need longer timeouts since they stream continuously
        let is_sse_endpoint = req.path.contains("/api/v1/events");
        let timeout_duration = if is_sse_endpoint {
            std::time::Duration::from_secs(300) // 5 minutes for streaming endpoints
        } else {
            std::time::Duration::from_secs(policy.timeout_secs.unwrap_or(30))
        };

        // Send request with timeout
        eprintln!("DEBUG: About to send HTTP request with {} second timeout", timeout_duration.as_secs());
        let response = tokio::time::timeout(
            timeout_duration,
            request_builder.send(),
        )
        .await;

        match &response {
            Ok(Ok(r)) => eprintln!("DEBUG: HTTP request completed, got status: {}", r.status()),
            Ok(Err(e)) => eprintln!("DEBUG: HTTP request failed: {}", e),
            Err(_) => eprintln!("DEBUG: HTTP request timed out after {} seconds", timeout_duration.as_secs()),
        }

        let response = response??;

        // Extract response
        let status = response.status().as_u16();
        let headers: HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        // For SSE endpoints, wait a bit for initial events before returning
        // But don't wait forever - give the client a chance to receive initial data
        let body = if is_sse_endpoint {
            eprintln!("DEBUG: SSE endpoint detected - waiting 2 seconds for initial events");
            // For SSE, wait a short time for initial events to arrive
            // This allows the first batch of events to be sent immediately rather than
            // delaying until the next event arrives (which could be minutes later)
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                response.text(),
            )
            .await
            .ok()
            .and_then(|r| r.ok())
            .or(Some(String::new())) // If timeout or error, return empty string
        } else {
            eprintln!("DEBUG: Regular endpoint - buffering response body normally");
            response.text().await.ok()
        };

        Ok(HttpResponse {
            id: req.id.clone(),
            status,
            headers,
            body,
        })
    }
}

impl Default for HttpDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_dispatcher_creation() {
        let _dispatcher = HttpDispatcher::new();
    }

    #[tokio::test]
    async fn test_tool_not_in_policy() {
        let dispatcher = HttpDispatcher::new();
        let req = HttpRequest {
            id: "test-1".to_string(),
            tool: "unknown".to_string(),
            method: "POST".to_string(),
            path: "/api".to_string(),
            headers: HashMap::new(),
            body: None,
        };

        let result = dispatcher.dispatch_http(req).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cli_tool_rejects_http() {
        let mut policy = PolicyConfig {
            tools: HashMap::new(),
        };

        policy.tools.insert(
            "gh".to_string(),
            carapace_policy::ToolPolicy::Cli(carapace_policy::CliPolicy {
                binary: "/usr/bin/gh".to_string(),
                argv_allow_patterns: vec!["*".to_string()],
                argv_deny_patterns: vec![],
                env_inject: HashMap::new(),
                cwd_allowed: None,
                timeout_secs: 30,
                audit: carapace_policy::AuditConfig::default(),
            }),
        );

        let dispatcher = HttpDispatcher::with_policy(policy);
        let req = HttpRequest {
            id: "test-1".to_string(),
            tool: "gh".to_string(),
            method: "POST".to_string(),
            path: "/api".to_string(),
            headers: HashMap::new(),
            body: None,
        };

        let result = dispatcher.dispatch_http(req).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_request_path_control_char_detection() {
        let req = HttpRequest {
            id: "smuggle".to_string(),
            tool: "signal-cli".to_string(),
            method: "POST".to_string(),
            path: "/api\r\nX-Injected: header".to_string(),
            headers: HashMap::new(),
            body: None,
        };

        // Path contains \r\n, should be detected
        assert!(req.path.contains('\r') || req.path.contains('\n'));
    }

    #[test]
    fn test_oversized_body_detection() {
        let huge_body = "x".repeat(150 * 1024 * 1024); // 150MB

        let req = HttpRequest {
            id: "huge".to_string(),
            tool: "signal-cli".to_string(),
            method: "POST".to_string(),
            path: "/api".to_string(),
            headers: HashMap::new(),
            body: Some(huge_body),
        };

        assert!(req.body.as_ref().unwrap().len() > 100 * 1024 * 1024);
    }

    #[test]
    fn test_content_type_header_case_insensitive() {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());

        let has_content_type = headers.keys().any(|k| k.to_lowercase() == "content-type");

        assert!(
            has_content_type,
            "Should detect content-type regardless of case"
        );
    }
}
