use carapace_policy::{ParamFilter, PolicyValidator};
use std::collections::HashMap;

#[test]
fn test_param_filter_allow_pattern() {
    let mut filters = HashMap::new();
    filters.insert(
        "send".to_string(),
        ParamFilter {
            field: "recipientNumber".to_string(),
            allow_patterns: vec!["+1555*".to_string()],
            deny_patterns: vec![],
        },
    );

    let body = r#"{"jsonrpc":"2.0","id":"1","method":"send","params":{"recipientNumber":"+15551234567"}}"#;

    let result = PolicyValidator::validate_jsonrpc_params("send", body, &filters);
    assert!(result.is_ok());
}

#[test]
fn test_param_filter_deny_pattern() {
    let mut filters = HashMap::new();
    filters.insert(
        "send".to_string(),
        ParamFilter {
            field: "recipientNumber".to_string(),
            allow_patterns: vec!["+1*".to_string()],
            deny_patterns: vec!["+15551234567".to_string()],
        },
    );

    let body = r#"{"jsonrpc":"2.0","id":"1","method":"send","params":{"recipientNumber":"+15551234567"}}"#;

    let result = PolicyValidator::validate_jsonrpc_params("send", body, &filters);
    assert!(result.is_err());
}

#[test]
fn test_param_filter_deny_takes_precedence() {
    let mut filters = HashMap::new();
    filters.insert(
        "send".to_string(),
        ParamFilter {
            field: "recipientNumber".to_string(),
            allow_patterns: vec!["+1555*".to_string()], // Allow range
            deny_patterns: vec!["+15551234567".to_string()], // Deny specific
        },
    );

    let body = r#"{"jsonrpc":"2.0","id":"1","method":"send","params":{"recipientNumber":"+15551234567"}}"#;

    let result = PolicyValidator::validate_jsonrpc_params("send", body, &filters);
    assert!(result.is_err(), "Deny should take precedence over allow");
}

#[test]
fn test_param_filter_no_filter_for_method() {
    let filters = HashMap::new(); // No filters defined

    let body = r#"{"jsonrpc":"2.0","id":"1","method":"receive","params":{}}"#;

    let result = PolicyValidator::validate_jsonrpc_params("receive", body, &filters);
    assert!(result.is_ok(), "No filter = allow all");
}

#[test]
fn test_param_filter_missing_field() {
    let mut filters = HashMap::new();
    filters.insert(
        "send".to_string(),
        ParamFilter {
            field: "recipientNumber".to_string(),
            allow_patterns: vec!["+1*".to_string()],
            deny_patterns: vec![],
        },
    );

    let body = r#"{"jsonrpc":"2.0","id":"1","method":"send","params":{"message":"hello"}}"#;

    let result = PolicyValidator::validate_jsonrpc_params("send", body, &filters);
    assert!(result.is_err(), "Missing required field should fail");
}

#[test]
fn test_param_filter_multiple_allowed_patterns() {
    let mut filters = HashMap::new();
    filters.insert(
        "send".to_string(),
        ParamFilter {
            field: "recipientNumber".to_string(),
            allow_patterns: vec!["+1555*".to_string(), "+1777*".to_string(), "+1888*".to_string()],
            deny_patterns: vec![],
        },
    );

    // Test first pattern
    let body1 = r#"{"jsonrpc":"2.0","id":"1","method":"send","params":{"recipientNumber":"+15559999999"}}"#;
    assert!(PolicyValidator::validate_jsonrpc_params("send", body1, &filters).is_ok());

    // Test second pattern
    let body2 = r#"{"jsonrpc":"2.0","id":"1","method":"send","params":{"recipientNumber":"+17778888888"}}"#;
    assert!(PolicyValidator::validate_jsonrpc_params("send", body2, &filters).is_ok());

    // Test third pattern
    let body3 = r#"{"jsonrpc":"2.0","id":"1","method":"send","params":{"recipientNumber":"+18889999999"}}"#;
    assert!(PolicyValidator::validate_jsonrpc_params("send", body3, &filters).is_ok());

    // Test non-matching pattern
    let body4 = r#"{"jsonrpc":"2.0","id":"1","method":"send","params":{"recipientNumber":"+19999999999"}}"#;
    assert!(PolicyValidator::validate_jsonrpc_params("send", body4, &filters).is_err());
}

#[test]
fn test_param_filter_allow_without_deny() {
    let mut filters = HashMap::new();
    filters.insert(
        "send".to_string(),
        ParamFilter {
            field: "recipientNumber".to_string(),
            allow_patterns: vec!["+1555*".to_string()],
            deny_patterns: vec![], // No deny patterns
        },
    );

    // Matching allow pattern
    let body1 = r#"{"jsonrpc":"2.0","id":"1","method":"send","params":{"recipientNumber":"+15551234567"}}"#;
    assert!(PolicyValidator::validate_jsonrpc_params("send", body1, &filters).is_ok());

    // Non-matching allow pattern
    let body2 = r#"{"jsonrpc":"2.0","id":"1","method":"send","params":{"recipientNumber":"+18009999999"}}"#;
    assert!(PolicyValidator::validate_jsonrpc_params("send", body2, &filters).is_err());
}

#[test]
fn test_param_filter_deny_without_allow() {
    let mut filters = HashMap::new();
    filters.insert(
        "send".to_string(),
        ParamFilter {
            field: "recipientNumber".to_string(),
            allow_patterns: vec![], // No allow patterns - allow all except denied
            deny_patterns: vec!["+15551234567".to_string(), "+18005551212".to_string()],
        },
    );

    // In deny list
    let body1 = r#"{"jsonrpc":"2.0","id":"1","method":"send","params":{"recipientNumber":"+15551234567"}}"#;
    assert!(PolicyValidator::validate_jsonrpc_params("send", body1, &filters).is_err());

    // Not in deny list (should be allowed)
    let body2 = r#"{"jsonrpc":"2.0","id":"1","method":"send","params":{"recipientNumber":"+19999999999"}}"#;
    assert!(PolicyValidator::validate_jsonrpc_params("send", body2, &filters).is_ok());
}

#[test]
fn test_param_filter_wildcard_patterns() {
    let mut filters = HashMap::new();
    filters.insert(
        "send".to_string(),
        ParamFilter {
            field: "recipientNumber".to_string(),
            allow_patterns: vec!["+1*".to_string()], // Any US number
            deny_patterns: vec!["*5551234*".to_string()], // Block 555-1234-XXXX range
        },
    );

    // Valid US number not in deny pattern
    let body1 = r#"{"jsonrpc":"2.0","id":"1","method":"send","params":{"recipientNumber":"+14155552222"}}"#;
    assert!(PolicyValidator::validate_jsonrpc_params("send", body1, &filters).is_ok());

    // Valid US number but in deny pattern
    let body2 = r#"{"jsonrpc":"2.0","id":"1","method":"send","params":{"recipientNumber":"+15551234567"}}"#;
    assert!(PolicyValidator::validate_jsonrpc_params("send", body2, &filters).is_err());

    // Non-US number
    let body3 = r#"{"jsonrpc":"2.0","id":"1","method":"send","params":{"recipientNumber":"+441234567890"}}"#;
    assert!(PolicyValidator::validate_jsonrpc_params("send", body3, &filters).is_err());
}

#[test]
fn test_param_filter_malformed_json() {
    let mut filters = HashMap::new();
    filters.insert(
        "send".to_string(),
        ParamFilter {
            field: "recipientNumber".to_string(),
            allow_patterns: vec!["+1*".to_string()],
            deny_patterns: vec![],
        },
    );

    let body = "not json at all";

    let result = PolicyValidator::validate_jsonrpc_params("send", body, &filters);
    assert!(result.is_err());
}

#[test]
fn test_param_filter_missing_params_object() {
    let mut filters = HashMap::new();
    filters.insert(
        "send".to_string(),
        ParamFilter {
            field: "recipientNumber".to_string(),
            allow_patterns: vec!["+1*".to_string()],
            deny_patterns: vec![],
        },
    );

    let body = r#"{"jsonrpc":"2.0","id":"1","method":"send"}"#;

    let result = PolicyValidator::validate_jsonrpc_params("send", body, &filters);
    assert!(result.is_err());
}

#[test]
fn test_param_filter_integer_field_value() {
    let mut filters = HashMap::new();
    filters.insert(
        "receive".to_string(),
        ParamFilter {
            field: "timeout".to_string(),
            allow_patterns: vec![], // No patterns - non-string field
            deny_patterns: vec![],
        },
    );

    let body = r#"{"jsonrpc":"2.0","id":"1","method":"receive","params":{"timeout":30}}"#;

    let result = PolicyValidator::validate_jsonrpc_params("receive", body, &filters);
    // Should fail because timeout is an integer, not a string
    assert!(result.is_err());
}
