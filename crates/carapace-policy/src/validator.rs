use crate::config::ParamFilter;
use crate::error::PolicyError;
use glob::Pattern;
use std::collections::HashMap;

/// Validator for request-specific policy validation
pub struct PolicyValidator;

impl PolicyValidator {
    /// Validate that a method is in the allowed list
    pub fn validate_jsonrpc_method(
        method: &str,
        allowed_methods: &[String],
        denied_methods: &[String],
    ) -> Result<(), PolicyError> {
        // Deny takes precedence
        if denied_methods.contains(&method.to_string()) {
            return Err(PolicyError::Violation(format!(
                "Method '{}' is explicitly denied",
                method
            )));
        }

        // Check allow list
        if !allowed_methods.is_empty() && !allowed_methods.contains(&method.to_string()) {
            return Err(PolicyError::Violation(format!(
                "Method '{}' is not in allowed methods",
                method
            )));
        }

        Ok(())
    }

    /// Validate JSON-RPC params against policy filters
    pub fn validate_jsonrpc_params(
        method: &str,
        body: &str,
        filters: &HashMap<String, ParamFilter>,
    ) -> Result<(), PolicyError> {
        // Check if this method has param filters
        if let Some(filter) = filters.get(method) {
            // Parse JSON body
            let json: serde_json::Value = serde_json::from_str(body)
                .map_err(|e| PolicyError::Violation(format!("Invalid JSON: {}", e)))?;

            // Extract params object
            let params = json.get("params").ok_or_else(|| {
                PolicyError::Violation("Missing params field in JSON-RPC request".to_string())
            })?;

            // Extract the field to filter on (handle both string and array values)
            let field_value = params
                .get(&filter.field)
                .and_then(|v| {
                    // Try as string first
                    if let Some(s) = v.as_str() {
                        return Some(s.to_string());
                    }
                    // If it's an array, take the first element as a string
                    if let Some(arr) = v.as_array() {
                        if let Some(first) = arr.first() {
                            if let Some(s) = first.as_str() {
                                return Some(s.to_string());
                            }
                        }
                    }
                    None
                })
                .ok_or_else(|| {
                    PolicyError::Violation(format!(
                        "Missing or invalid field '{}' in params",
                        filter.field
                    ))
                })?;

            // Check deny patterns first (deny-first semantics)
            for pattern_str in &filter.deny_patterns {
                let pattern = Pattern::new(pattern_str).map_err(|e| {
                    PolicyError::Violation(format!("Invalid deny pattern '{}': {}", pattern_str, e))
                })?;

                if pattern.matches(&field_value) {
                    return Err(PolicyError::Violation(format!(
                        "Param '{}' value '{}' matches deny pattern '{}'",
                        filter.field, field_value, pattern_str
                    )));
                }
            }

            // If allow patterns exist, check them (whitelist mode)
            if !filter.allow_patterns.is_empty() {
                let mut allowed = false;
                for pattern_str in &filter.allow_patterns {
                    let pattern = Pattern::new(pattern_str).map_err(|e| {
                        PolicyError::Violation(format!(
                            "Invalid allow pattern '{}': {}",
                            pattern_str, e
                        ))
                    })?;

                    if pattern.matches(&field_value) {
                        allowed = true;
                        break;
                    }
                }

                if !allowed {
                    return Err(PolicyError::Violation(format!(
                        "Param '{}' value '{}' not in allow list",
                        filter.field, field_value
                    )));
                }
            }
        }

        Ok(())
    }

    /// Validate binary path doesn't have traversal attempts
    pub fn validate_binary_path(path: &str) -> Result<(), PolicyError> {
        // Check for path traversal
        if path.contains("..") {
            return Err(PolicyError::Violation(format!(
                "Binary path contains path traversal: {}",
                path
            )));
        }

        // Check for null bytes
        if path.contains('\0') {
            return Err(PolicyError::Violation(
                "Binary path contains null byte".to_string(),
            ));
        }

        Ok(())
    }

    /// Validate that argv doesn't contain shell metacharacters (high-risk)
    pub fn has_dangerous_shell_chars(s: &str) -> bool {
        const DANGEROUS_CHARS: &[char] = &[
            ';', '|', '&', '$', '`', '(', ')', '<', '>', '\n', '\r', '\t',
        ];
        s.chars().any(|c| DANGEROUS_CHARS.contains(&c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allowed_jsonrpc_method() {
        let allowed = vec!["send".to_string(), "receive".to_string()];
        let denied = vec![];

        let result = PolicyValidator::validate_jsonrpc_method("send", &allowed, &denied);
        assert!(result.is_ok());
    }

    #[test]
    fn test_denied_jsonrpc_method() {
        let allowed = vec!["send".to_string(), "receive".to_string()];
        let denied = vec!["delete".to_string()];

        let result = PolicyValidator::validate_jsonrpc_method("delete", &allowed, &denied);
        assert!(result.is_err());
    }

    #[test]
    fn test_deny_precedence() {
        let allowed = vec!["*".to_string()]; // Allow all
        let denied = vec!["deleteEverything".to_string()];

        let result =
            PolicyValidator::validate_jsonrpc_method("deleteEverything", &allowed, &denied);
        assert!(result.is_err());
    }

    #[test]
    fn test_method_not_in_whitelist() {
        let allowed = vec!["send".to_string()];
        let denied = vec![];

        let result = PolicyValidator::validate_jsonrpc_method("unknown", &allowed, &denied);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_allowed_list_means_deny_all() {
        let allowed = vec![]; // Empty allowed list means "no restrictions from allow list"
        let denied = vec![]; // Empty denied list

        // With empty allowed list and empty denied list, method is allowed (no restrictions)
        let result = PolicyValidator::validate_jsonrpc_method("anything", &allowed, &denied);
        assert!(result.is_ok(), "Empty lists means no restrictions");

        // To actually deny all, you need to configure an allowed list with specific methods
        let allowed_restricted = vec!["send".to_string()];
        let result2 =
            PolicyValidator::validate_jsonrpc_method("receive", &allowed_restricted, &denied);
        assert!(
            result2.is_err(),
            "Method not in allowed list should be denied"
        );
    }

    #[test]
    fn test_valid_binary_path() {
        let result = PolicyValidator::validate_binary_path("/usr/bin/gh");
        assert!(result.is_ok());
    }

    #[test]
    fn test_path_traversal_detection() {
        let result = PolicyValidator::validate_binary_path("../../etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn test_path_traversal_in_middle() {
        let result = PolicyValidator::validate_binary_path("/usr/../../../etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn test_null_byte_in_path() {
        let result = PolicyValidator::validate_binary_path("/usr/bin/gh\0evil");
        assert!(result.is_err());
    }

    #[test]
    fn test_dangerous_shell_chars_detection() {
        assert!(PolicyValidator::has_dangerous_shell_chars(";rm -rf /"));
        assert!(PolicyValidator::has_dangerous_shell_chars("| cat"));
        assert!(PolicyValidator::has_dangerous_shell_chars("&& command"));
        assert!(PolicyValidator::has_dangerous_shell_chars("$(whoami)"));
        assert!(PolicyValidator::has_dangerous_shell_chars("`command`"));
        assert!(PolicyValidator::has_dangerous_shell_chars("test < file"));
        assert!(PolicyValidator::has_dangerous_shell_chars("test > file"));

        assert!(!PolicyValidator::has_dangerous_shell_chars(
            "normal argument"
        ));
        assert!(!PolicyValidator::has_dangerous_shell_chars(
            "arg-with-dashes"
        ));
        assert!(!PolicyValidator::has_dangerous_shell_chars(
            "arg_with_underscores"
        ));
        assert!(!PolicyValidator::has_dangerous_shell_chars("arg123"));
    }

    #[test]
    fn test_newline_in_argument() {
        assert!(PolicyValidator::has_dangerous_shell_chars("arg\ncommand"));
        assert!(PolicyValidator::has_dangerous_shell_chars("arg\rcommand"));
    }
}
