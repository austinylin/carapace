use carapace_protocol::{CliRequest, CliResponse};
use carapace_policy::{PolicyConfig, PolicyValidator, ArgvMatcher};
use std::collections::HashMap;
use tokio::process::Command;

/// Handles CLI command execution with policy enforcement
pub struct CliDispatcher {
    policy: PolicyConfig,
}

impl CliDispatcher {
    pub fn new() -> Self {
        CliDispatcher {
            policy: PolicyConfig {
                tools: HashMap::new(),
            },
        }
    }

    pub fn with_policy(policy: PolicyConfig) -> Self {
        CliDispatcher { policy }
    }

    /// Dispatch a CLI request, validate against policy, and execute
    pub async fn dispatch_cli(&self, req: CliRequest) -> anyhow::Result<CliResponse> {
        // Check if tool is allowed in policy
        let tool_config = self
            .policy
            .tools
            .get(&req.tool)
            .ok_or_else(|| anyhow::anyhow!("Tool '{}' not in policy", req.tool))?;

        // Get CLI policy
        let cli_policy = match tool_config {
            carapace_policy::ToolPolicy::Cli(policy) => policy,
            carapace_policy::ToolPolicy::Http(_) => {
                return Err(anyhow::anyhow!(
                    "Tool '{}' is HTTP-only, cannot handle CLI request",
                    req.tool
                ))
            }
        };

        // Validate argv against allow/deny patterns
        let matcher = ArgvMatcher::new(
            cli_policy.argv_allow_patterns.clone(),
            cli_policy.argv_deny_patterns.clone(),
        )?;

        if !matcher.matches(&req.argv) {
            return Err(anyhow::anyhow!(
                "CLI request denied by policy: argv={:?}",
                req.argv
            ));
        }

        // Validate binary path
        PolicyValidator::validate_binary_path(&cli_policy.binary)?;

        // Check for shell injection attempts in argv
        for arg in &req.argv {
            if PolicyValidator::has_dangerous_shell_chars(arg) {
                return Err(anyhow::anyhow!(
                    "Shell injection detected in argument: {}",
                    arg
                ));
            }
        }

        // Merge policy-injected env vars with request env (policy takes precedence)
        let mut merged_env = req.env.clone();
        for (key, value) in &cli_policy.env_inject {
            merged_env.insert(key.clone(), value.clone());
        }

        // Execute the command
        let output = self.execute_command(&cli_policy.binary, &req.argv, &merged_env).await?;

        Ok(CliResponse {
            id: req.id,
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    /// Execute a command with the given argv and environment
    async fn execute_command(
        &self,
        binary: &str,
        argv: &[String],
        env: &HashMap<String, String>,
    ) -> anyhow::Result<std::process::Output> {
        let mut cmd = Command::new(binary);

        // Add arguments
        for arg in argv {
            cmd.arg(arg);
        }

        // Set environment variables
        for (key, value) in env {
            cmd.env(key, value);
        }

        // Capture output
        let output = cmd.output().await?;

        Ok(output)
    }
}

impl Default for CliDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carapace_policy::CliPolicy;

    #[test]
    fn test_cli_dispatcher_creation() {
        let _dispatcher = CliDispatcher::new();
    }

    #[tokio::test]
    async fn test_tool_not_in_policy() {
        let dispatcher = CliDispatcher::new();
        let req = CliRequest {
            id: "test-1".to_string(),
            tool: "unknown".to_string(),
            argv: vec!["arg".to_string()],
            env: HashMap::new(),
            stdin: None,
            cwd: "/tmp".to_string(),
        };

        let result = dispatcher.dispatch_cli(req).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_denied_argv_pattern() {
        let mut policy = PolicyConfig {
            tools: HashMap::new(),
        };

        policy.tools.insert(
            "test".to_string(),
            carapace_policy::ToolPolicy::Cli(CliPolicy {
                binary: "/usr/bin/test".to_string(),
                argv_allow_patterns: vec!["list".to_string()],
                argv_deny_patterns: vec![],
                env_inject: HashMap::new(),
                cwd_allowed: None,
                timeout_secs: 30,
                audit: carapace_policy::AuditConfig::default(),
            }),
        );

        let dispatcher = CliDispatcher::with_policy(policy);
        let req = CliRequest {
            id: "test-1".to_string(),
            tool: "test".to_string(),
            argv: vec!["delete".to_string()],
            env: HashMap::new(),
            stdin: None,
            cwd: "/tmp".to_string(),
        };

        let result = dispatcher.dispatch_cli(req).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_shell_injection_detection() {
        let mut policy = PolicyConfig {
            tools: HashMap::new(),
        };

        policy.tools.insert(
            "test".to_string(),
            carapace_policy::ToolPolicy::Cli(CliPolicy {
                binary: "/usr/bin/test".to_string(),
                argv_allow_patterns: vec!["*".to_string()],
                argv_deny_patterns: vec![],
                env_inject: HashMap::new(),
                cwd_allowed: None,
                timeout_secs: 30,
                audit: carapace_policy::AuditConfig::default(),
            }),
        );

        let dispatcher = CliDispatcher::with_policy(policy);
        let req = CliRequest {
            id: "test-1".to_string(),
            tool: "test".to_string(),
            argv: vec!["safe; rm -rf /".to_string()],
            env: HashMap::new(),
            stdin: None,
            cwd: "/tmp".to_string(),
        };

        let result = dispatcher.dispatch_cli(req).await;
        assert!(result.is_err());
    }
}
