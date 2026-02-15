use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Unique request/response identifier
pub type RequestId = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    CliRequest(CliRequest),
    CliResponse(CliResponse),
    HttpRequest(HttpRequest),
    HttpResponse(HttpResponse),
    SseEvent(SseEvent),
    Error(ErrorMessage),
}

impl Message {
    pub fn id(&self) -> Option<&str> {
        match self {
            Message::CliRequest(req) => Some(&req.id),
            Message::CliResponse(res) => Some(&res.id),
            Message::HttpRequest(req) => Some(&req.id),
            Message::HttpResponse(res) => Some(&res.id),
            Message::SseEvent(evt) => Some(&evt.id),
            Message::Error(err) => err.id.as_deref(),
        }
    }
}

/// CLI tool invocation request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliRequest {
    pub id: RequestId,
    pub tool: String,
    pub argv: Vec<String>,
    pub env: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdin: Option<String>,
    pub cwd: String,
}

/// CLI tool response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliResponse {
    pub id: RequestId,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// HTTP request to proxy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRequest {
    pub id: RequestId,
    pub tool: String,
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

/// HTTP response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpResponse {
    pub id: RequestId,
    pub status: u16,
    pub headers: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

/// Server-sent event from upstream (streamed incrementally for SSE endpoints)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SseEvent {
    pub id: RequestId,  // Correlates to HttpRequest.id for streaming responses
    pub tool: String,
    pub event: String,
    pub data: String,
}

/// Error response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorMessage {
    pub id: Option<RequestId>,
    pub code: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_request_serialization() {
        let req = CliRequest {
            id: "test-001".to_string(),
            tool: "gh".to_string(),
            argv: vec!["pr".to_string(), "list".to_string()],
            env: HashMap::new(),
            stdin: None,
            cwd: "/home/user".to_string(),
        };

        let json = serde_json::to_string(&req).expect("serialization failed");
        let deserialized: CliRequest = serde_json::from_str(&json).expect("deserialization failed");

        assert_eq!(deserialized.id, req.id);
        assert_eq!(deserialized.tool, req.tool);
        assert_eq!(deserialized.argv, req.argv);
    }

    #[test]
    fn test_cli_request_with_stdin() {
        let req = CliRequest {
            id: "test-002".to_string(),
            tool: "curl".to_string(),
            argv: vec!["https://example.com".to_string()],
            env: HashMap::new(),
            stdin: Some("POST data".to_string()),
            cwd: "/tmp".to_string(),
        };

        let json = serde_json::to_string(&req).expect("serialization failed");
        assert!(json.contains("stdin"));

        let deserialized: CliRequest = serde_json::from_str(&json).expect("deserialization failed");
        assert_eq!(deserialized.stdin, Some("POST data".to_string()));
    }

    #[test]
    fn test_cli_response_serialization() {
        let resp = CliResponse {
            id: "test-001".to_string(),
            exit_code: 0,
            stdout: "response".to_string(),
            stderr: "".to_string(),
        };

        let json = serde_json::to_string(&resp).expect("serialization failed");
        let deserialized: CliResponse =
            serde_json::from_str(&json).expect("deserialization failed");

        assert_eq!(deserialized.exit_code, 0);
        assert_eq!(deserialized.stdout, "response".to_string());
    }

    #[test]
    fn test_http_request_serialization() {
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        let req = HttpRequest {
            id: "http-001".to_string(),
            tool: "signal-cli".to_string(),
            method: "POST".to_string(),
            path: "/api/v1/rpc".to_string(),
            headers,
            body: Some(r#"{"jsonrpc":"2.0","method":"send"}"#.to_string()),
        };

        let json = serde_json::to_string(&req).expect("serialization failed");
        let deserialized: HttpRequest =
            serde_json::from_str(&json).expect("deserialization failed");

        assert_eq!(deserialized.method, "POST");
        assert!(deserialized.body.is_some());
    }

    #[test]
    fn test_message_enum_serialization() {
        let cli_req = Message::CliRequest(CliRequest {
            id: "test".to_string(),
            tool: "gh".to_string(),
            argv: vec![],
            env: HashMap::new(),
            stdin: None,
            cwd: "/".to_string(),
        });

        let json = serde_json::to_string(&cli_req).expect("serialization failed");
        assert!(json.contains("\"type\":\"cli_request\""));

        let deserialized: Message = serde_json::from_str(&json).expect("deserialization failed");
        assert!(matches!(deserialized, Message::CliRequest(_)));
    }

    #[test]
    fn test_message_id_extraction() {
        let msg = Message::CliRequest(CliRequest {
            id: "req-123".to_string(),
            tool: "gh".to_string(),
            argv: vec![],
            env: HashMap::new(),
            stdin: None,
            cwd: "/".to_string(),
        });

        assert_eq!(msg.id(), Some("req-123"));
    }

    #[test]
    fn test_sse_event_with_id_correlation() {
        let event = Message::SseEvent(SseEvent {
            id: "sse-request-123".to_string(),
            tool: "signal-cli".to_string(),
            event: "message".to_string(),
            data: r#"{"from":"+15551234567","body":"Hello"}"#.to_string(),
        });

        // SseEvent now has an id for request correlation
        assert_eq!(event.id(), Some("sse-request-123"));
    }

    #[test]
    fn test_sse_event_serialization_with_id() {
        let event = SseEvent {
            id: "stream-001".to_string(),
            tool: "signal-cli".to_string(),
            event: "message".to_string(),
            data: r#"{"type":"message","sender":"+15551234567"}"#.to_string(),
        };

        let json = serde_json::to_string(&event).expect("serialization failed");
        assert!(json.contains("\"id\":\"stream-001\""));

        let deserialized: SseEvent = serde_json::from_str(&json).expect("deserialization failed");
        assert_eq!(deserialized.id, "stream-001");
        assert_eq!(deserialized.tool, "signal-cli");
    }

    #[test]
    fn test_large_stdout_payload() {
        let large_output = "x".repeat(1024 * 1024); // 1MB

        let resp = CliResponse {
            id: "test-large".to_string(),
            exit_code: 0,
            stdout: large_output.clone(),
            stderr: "".to_string(),
        };

        let json = serde_json::to_string(&resp).expect("serialization failed");
        let deserialized: CliResponse =
            serde_json::from_str(&json).expect("deserialization failed");

        assert_eq!(deserialized.stdout.len(), 1024 * 1024);
    }

    #[test]
    fn test_unicode_argv() {
        let req = CliRequest {
            id: "unicode-test".to_string(),
            tool: "gh".to_string(),
            argv: vec![
                "issue".to_string(),
                "create".to_string(),
                "--title".to_string(),
                "Testing 你好 🎉".to_string(),
            ],
            env: HashMap::new(),
            stdin: None,
            cwd: "/".to_string(),
        };

        let json = serde_json::to_string(&req).expect("serialization failed");
        let deserialized: CliRequest = serde_json::from_str(&json).expect("deserialization failed");

        assert_eq!(deserialized.argv[3], "Testing 你好 🎉");
    }

    #[test]
    fn test_negative_exit_code() {
        let resp = CliResponse {
            id: "test-negative".to_string(),
            exit_code: -1,
            stdout: "".to_string(),
            stderr: "Error".to_string(),
        };

        let json = serde_json::to_string(&resp).expect("serialization failed");
        let deserialized: CliResponse =
            serde_json::from_str(&json).expect("deserialization failed");

        assert_eq!(deserialized.exit_code, -1);
    }

    #[test]
    fn test_missing_optional_fields() {
        let json_no_stdin =
            r#"{"type":"cli_request","id":"test","tool":"gh","argv":[],"env":{},"cwd":"/"}"#;
        let req: CliRequest = serde_json::from_str(json_no_stdin).expect("deserialization failed");
        assert_eq!(req.stdin, None);

        let json_no_body = r#"{"type":"http_request","id":"test","tool":"signal-cli","method":"GET","path":"/","headers":{}}"#;
        let http_req: HttpRequest =
            serde_json::from_str(json_no_body).expect("deserialization failed");
        assert_eq!(http_req.body, None);
    }

    #[test]
    fn test_malformed_json_missing_fields() {
        let malformed = r#"{"type":"cli_request","id":"test","tool":"gh"}"#;
        let result: Result<CliRequest, _> = serde_json::from_str(malformed);
        assert!(result.is_err(), "Should fail with missing required fields");
    }

    #[test]
    fn test_wrong_field_types() {
        let malformed = r#"{"type":"cli_response","id":123,"exit_code":"not_a_number","stdout":"out","stderr":""}"#;
        let result: Result<CliResponse, _> = serde_json::from_str(malformed);
        assert!(result.is_err(), "Should fail with wrong types");
    }
}
