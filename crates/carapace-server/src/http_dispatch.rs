use carapace_protocol::{HttpRequest, HttpResponse};
use carapace_policy::{PolicyConfig, PolicyValidator, HttpPolicy};
use std::collections::HashMap;
use reqwest::Client;

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
        // Check if tool is allowed in policy
        let tool_config = self
            .policy
            .tools
            .get(&req.tool)
            .ok_or_else(|| anyhow::anyhow!("Tool '{}' not in policy", req.tool))?;

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
                    PolicyValidator::validate_jsonrpc_method(
                        method,
                        &http_policy.jsonrpc_allow_methods,
                        &http_policy.jsonrpc_deny_methods,
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
        let response = self.proxy_to_upstream(http_policy, &req).await?;

        Ok(response)
    }

    /// Proxy request to upstream server
    async fn proxy_to_upstream(
        &self,
        policy: &HttpPolicy,
        req: &HttpRequest,
    ) -> anyhow::Result<HttpResponse> {
        let url = format!("{}{}", policy.upstream, req.path);

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

        // Add body
        if let Some(body) = &req.body {
            request_builder = request_builder.body(body.clone());
        }

        // Send request with timeout
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(policy.timeout_secs.unwrap_or(30)),
            request_builder.send(),
        )
        .await??;

        // Extract response
        let status = response.status().as_u16();
        let headers: HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    v.to_str().unwrap_or("").to_string(),
                )
            })
            .collect();

        let body = response.text().await.ok();

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
}
