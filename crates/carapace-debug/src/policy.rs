use anyhow::{anyhow, Result};
use carapace_policy::{ArgvMatcher, PolicyConfig, PolicyValidator};
use serde_json::json;
use std::fs;
use std::path::Path;

/// Test a policy decision without running the full system
pub async fn policy(policy_file: &Path, request_json: &str, format: &str) -> Result<()> {
    // Load policy
    let policy = PolicyConfig::from_file(
        policy_file
            .to_str()
            .ok_or_else(|| anyhow!("Invalid policy file path"))?,
    )
    .map_err(|e| anyhow!("Failed to load policy: {}", e))?;

    // Parse request JSON
    let request: serde_json::Value = if request_json.starts_with('{') {
        // Inline JSON
        serde_json::from_str(request_json)?
    } else if Path::new(request_json).exists() {
        // JSON file
        let content = fs::read_to_string(request_json)?;
        serde_json::from_str(&content)?
    } else {
        return Err(anyhow!("Request must be inline JSON or path to JSON file"));
    };

    // Determine request type and test
    let result = if let Some(_method) = request.get("method").and_then(|v| v.as_str()) {
        // JSON-RPC method validation
        test_jsonrpc_method(&policy, &request)?
    } else if let Some(_argv) = request.get("argv").and_then(|v| v.as_array()) {
        // CLI validation
        test_cli_argv(&policy, &request)?
    } else {
        return Err(anyhow!(
            "Request must have either 'method' (JSON-RPC) or 'argv' (CLI) field"
        ));
    };

    // Output result
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        print_policy_result(&result);
    }

    Ok(())
}

fn test_jsonrpc_method(
    policy: &PolicyConfig,
    request: &serde_json::Value,
) -> Result<serde_json::Value> {
    let tool = request
        .get("tool")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Request must have 'tool' field"))?;

    let method = request
        .get("method")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Request must have 'method' field"))?;

    let body = serde_json::to_string(request)?;

    // Get tool policy
    let tool_config = policy
        .tools
        .get(tool)
        .ok_or_else(|| anyhow!("Tool '{}' not in policy", tool))?;

    let http_policy = match tool_config {
        carapace_policy::ToolPolicy::Http(policy) => policy,
        carapace_policy::ToolPolicy::Cli(_) => {
            return Ok(json!({
                "allowed": false,
                "reason": format!("Tool '{}' is CLI-only, cannot handle HTTP request", tool),
                "tool": tool,
                "method": method,
            }))
        }
    };

    // Validate method
    match PolicyValidator::validate_jsonrpc_method(
        method,
        &http_policy.jsonrpc_allow_methods,
        &http_policy.jsonrpc_deny_methods,
    ) {
        Ok(()) => {}
        Err(e) => {
            return Ok(json!({
                "allowed": false,
                "reason": format!("Method denied: {}", e),
                "tool": tool,
                "method": method,
            }))
        }
    }

    // Validate params
    match PolicyValidator::validate_jsonrpc_params(
        method,
        &body,
        &http_policy.jsonrpc_param_filters,
    ) {
        Ok(()) => Ok(json!({
            "allowed": true,
            "reason": "Both method and params passed policy validation",
            "tool": tool,
            "method": method,
        })),
        Err(e) => Ok(json!({
            "allowed": false,
            "reason": format!("Params denied: {}", e),
            "tool": tool,
            "method": method,
        })),
    }
}

fn test_cli_argv(policy: &PolicyConfig, request: &serde_json::Value) -> Result<serde_json::Value> {
    let tool = request
        .get("tool")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Request must have 'tool' field"))?;

    let argv = request
        .get("argv")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("Request must have 'argv' field as array"))?;

    let argv_strings: Vec<String> = argv
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();

    // Get tool policy
    let tool_config = policy
        .tools
        .get(tool)
        .ok_or_else(|| anyhow!("Tool '{}' not in policy", tool))?;

    let cli_policy = match tool_config {
        carapace_policy::ToolPolicy::Cli(policy) => policy,
        carapace_policy::ToolPolicy::Http(_) => {
            return Ok(json!({
                "allowed": false,
                "reason": format!("Tool '{}' is HTTP-only, cannot handle CLI request", tool),
                "tool": tool,
                "argv": argv_strings,
            }))
        }
    };

    // Validate argv using ArgvMatcher
    match ArgvMatcher::new(
        cli_policy.argv_allow_patterns.clone(),
        cli_policy.argv_deny_patterns.clone(),
    ) {
        Ok(matcher) => {
            let allowed = matcher.matches(&argv_strings);
            Ok(json!({
                "allowed": allowed,
                "reason": if allowed { "Argv passed policy validation" } else { "Argv denied by policy" },
                "tool": tool,
                "argv": argv_strings,
            }))
        }
        Err(e) => Ok(json!({
            "allowed": false,
            "reason": format!("Error creating matcher: {}", e),
            "tool": tool,
            "argv": argv_strings,
        })),
    }
}

fn print_policy_result(result: &serde_json::Value) {
    let allowed = result
        .get("allowed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let reason = result
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown");

    let tool = result
        .get("tool")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    println!("=== Policy Decision ===");
    println!("Tool: {}", tool);
    println!(
        "Decision: {}",
        if allowed { "✅ ALLOWED" } else { "❌ DENIED" }
    );
    println!("Reason: {}", reason);

    if let Some(method) = result.get("method").and_then(|v| v.as_str()) {
        println!("Method: {}", method);
    }

    if let Some(argv) = result.get("argv").and_then(|v| v.as_array()) {
        let args: Vec<String> = argv
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        println!("Arguments: {}", args.join(" "));
    }
}
