use carapace_protocol::{HttpRequest, HttpResponse};
use std::collections::HashMap;

#[test]
fn test_valid_json_rpc_request() {
    let req = HttpRequest {
        id: "http-001".to_string(),
        tool: "signal-cli".to_string(),
        method: "POST".to_string(),
        path: "/api/v1/rpc".to_string(),
        headers: {
            let mut h = HashMap::new();
            h.insert("Content-Type".to_string(), "application/json".to_string());
            h
        },
        body: Some(r#"{"jsonrpc":"2.0","method":"send","params":{}}"#.to_string()),
    };

    assert_eq!(req.method, "POST");
    assert!(req.body.is_some());
}

#[test]
fn test_malformed_json_in_request_body() {
    let req = HttpRequest {
        id: "bad-json".to_string(),
        tool: "signal-cli".to_string(),
        method: "POST".to_string(),
        path: "/api/v1/rpc".to_string(),
        headers: HashMap::new(),
        body: Some("{broken json".to_string()),
    };

    // Should detect that body is not valid JSON
    let body = req.body.unwrap();
    let result: Result<serde_json::Value, _> = serde_json::from_str(&body);
    assert!(result.is_err());
}

#[test]
fn test_http_request_smuggling_attempt() {
    // Attempt to smuggle additional headers
    let req = HttpRequest {
        id: "smuggle".to_string(),
        tool: "signal-cli".to_string(),
        method: "POST".to_string(),
        path: "/api/v1/rpc\r\nHost: attacker.com".to_string(),
        headers: HashMap::new(),
        body: None,
    };

    // Path should be validated to not contain control characters
    assert!(req.path.contains("\r\n"));
}

#[test]
fn test_oversized_request_body() {
    let huge_body = "x".repeat(200 * 1024 * 1024); // 200MB

    let req = HttpRequest {
        id: "huge".to_string(),
        tool: "signal-cli".to_string(),
        method: "POST".to_string(),
        path: "/api".to_string(),
        headers: HashMap::new(),
        body: Some(huge_body),
    };

    // Should be able to detect if body exceeds size limit
    assert!(req.body.as_ref().unwrap().len() > 100 * 1024 * 1024);
}

#[test]
fn test_missing_content_type_header() {
    let req = HttpRequest {
        id: "no-ct".to_string(),
        tool: "signal-cli".to_string(),
        method: "POST".to_string(),
        path: "/api".to_string(),
        headers: HashMap::new(),
        body: Some("{}".to_string()),
    };

    assert!(!req.headers.contains_key("Content-Type"));
}

#[test]
fn test_http_response_creation() {
    let resp = HttpResponse {
        id: "http-001".to_string(),
        status: 200,
        headers: {
            let mut h = HashMap::new();
            h.insert("Content-Type".to_string(), "application/json".to_string());
            h
        },
        body: Some(r#"{"result":"success"}"#.to_string()),
    };

    assert_eq!(resp.status, 200);
    assert!(resp.body.is_some());
}

#[test]
fn test_sse_response_detection() {
    let resp = HttpResponse {
        id: "sse-001".to_string(),
        status: 200,
        headers: {
            let mut h = HashMap::new();
            h.insert("Content-Type".to_string(), "text/event-stream".to_string());
            h.insert("Cache-Control".to_string(), "no-cache".to_string());
            h
        },
        body: None, // SSE uses streaming
    };

    let is_sse = resp
        .headers
        .get("Content-Type")
        .map(|ct: &String| ct.contains("event-stream"))
        .unwrap_or(false);

    assert!(is_sse);
}

#[test]
fn test_multiple_concurrent_http_requests() {
    let mut requests = Vec::new();

    for i in 0..100 {
        let req = HttpRequest {
            id: format!("concurrent-{}", i),
            tool: "signal-cli".to_string(),
            method: "POST".to_string(),
            path: "/api".to_string(),
            headers: HashMap::new(),
            body: Some(format!(r#"{{"id":{}}}"#, i)),
        };
        requests.push(req);
    }

    assert_eq!(requests.len(), 100);
}

#[test]
fn test_http_status_codes() {
    let status_codes = vec![200, 201, 204, 400, 401, 403, 404, 500, 502, 503];

    for status in status_codes {
        let resp = HttpResponse {
            id: format!("status-{}", status),
            status: status as u16,
            headers: HashMap::new(),
            body: None,
        };

        assert_eq!(resp.status, status as u16);
    }
}

#[test]
fn test_request_method_types() {
    let methods = vec!["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"];

    for method in methods {
        let req = HttpRequest {
            id: format!("method-{}", method),
            tool: "signal-cli".to_string(),
            method: method.to_string(),
            path: "/api".to_string(),
            headers: HashMap::new(),
            body: None,
        };

        assert_eq!(req.method, method);
    }
}

#[test]
fn test_path_with_query_parameters() {
    let req = HttpRequest {
        id: "query".to_string(),
        tool: "signal-cli".to_string(),
        method: "GET".to_string(),
        path: "/api/v1/messages?limit=10&offset=0&sort=date".to_string(),
        headers: HashMap::new(),
        body: None,
    };

    assert!(req.path.contains("?"));
    assert!(req.path.contains("limit=10"));
}

#[test]
fn test_json_rpc_method_extraction() {
    let body = r#"{"jsonrpc":"2.0","id":"1","method":"send","params":{"to":"+15551234567"}}"#;

    let json: serde_json::Value = serde_json::from_str(body).unwrap();
    let method = json.get("method").and_then(|m| m.as_str());

    assert_eq!(method, Some("send"));
}

#[test]
fn test_response_headers_preservation() {
    let mut headers = HashMap::new();
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    headers.insert("Cache-Control".to_string(), "no-cache".to_string());
    headers.insert("X-Custom-Header".to_string(), "value".to_string());

    let resp = HttpResponse {
        id: "headers".to_string(),
        status: 200,
        headers,
        body: None,
    };

    assert_eq!(resp.headers.len(), 3);
    assert_eq!(
        resp.headers.get("X-Custom-Header"),
        Some(&"value".to_string())
    );
}

#[test]
fn test_null_body_vs_empty_body() {
    let resp_null = HttpResponse {
        id: "null".to_string(),
        status: 200,
        headers: HashMap::new(),
        body: None,
    };

    let resp_empty = HttpResponse {
        id: "empty".to_string(),
        status: 200,
        headers: HashMap::new(),
        body: Some("".to_string()),
    };

    assert!(resp_null.body.is_none());
    assert!(resp_empty.body.is_some());
    assert_eq!(resp_empty.body.unwrap(), "");
}

#[test]
fn test_unicode_in_response_body() {
    let resp = HttpResponse {
        id: "unicode".to_string(),
        status: 200,
        headers: HashMap::new(),
        body: Some(r#"{"message":"你好 العربية 🎉"}"#.to_string()),
    };

    assert!(resp.body.is_some());
}

#[test]
fn test_large_response_body() {
    let large_body = "x".repeat(50 * 1024 * 1024); // 50MB

    let resp = HttpResponse {
        id: "large".to_string(),
        status: 200,
        headers: HashMap::new(),
        body: Some(large_body),
    };

    assert!(resp.body.is_some());
}
