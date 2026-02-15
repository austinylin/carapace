use carapace_protocol::CliRequest;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use uuid::Uuid;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Get argv[0] to extract tool name
    let argv0 = std::env::args().next().unwrap_or_default();
    let tool_name = extract_tool_name(&argv0);

    // Collect all arguments (skip argv[0])
    let argv: Vec<String> = std::env::args().skip(1).collect();

    // Get current working directory
    let cwd = std::env::current_dir()?.to_str().unwrap_or("/").to_string();

    // Collect environment variables
    let env: HashMap<String, String> = std::env::vars().collect();

    // Create CLI request
    let request_id = Uuid::new_v4().to_string();
    let cli_req = CliRequest {
        id: request_id.clone(),
        tool: tool_name.clone(),
        argv,
        env,
        stdin: None,
        cwd,
    };

    // Connect to agent socket
    let socket_path = get_agent_socket_path();
    let mut stream = match UnixStream::connect(&socket_path).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "Error: Could not connect to carapace agent at {}: {}",
                socket_path, e
            );
            std::process::exit(1);
        }
    };

    // Serialize request to JSON
    let request_json = serde_json::to_vec(&cli_req)?;

    // Send request
    stream.write_all(&request_json).await?;

    // Read response
    let mut response_buf = vec![0u8; 65536];
    let n = stream.read(&mut response_buf).await?;

    if n == 0 {
        eprintln!("Error: No response from agent");
        std::process::exit(1);
    }

    // Parse response
    let response_json: serde_json::Value = serde_json::from_slice(&response_buf[..n])?;

    // Extract fields
    let exit_code = response_json["exit_code"].as_i64().unwrap_or(-1) as i32;

    let stdout = response_json["stdout"].as_str().unwrap_or("");

    let stderr = response_json["stderr"].as_str().unwrap_or("");

    // Print output
    if !stdout.is_empty() {
        print!("{}", stdout);
    }

    if !stderr.is_empty() {
        eprint!("{}", stderr);
    }

    // Exit with response code
    std::process::exit(exit_code);
}

/// Extract tool name from argv[0]
fn extract_tool_name(argv0: &str) -> String {
    PathBuf::from(argv0)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string()
}

/// Get the path to the agent socket
fn get_agent_socket_path() -> String {
    // Try to get from environment variable first
    std::env::var("CARAPACE_AGENT_SOCKET").unwrap_or_else(|_| {
        // Default to /tmp/carapace-agent.sock
        "/tmp/carapace-agent.sock".to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_tool_name_from_path() {
        assert_eq!(extract_tool_name("/usr/bin/gh"), "gh");
        assert_eq!(extract_tool_name("gh"), "gh");
        assert_eq!(extract_tool_name("/usr/local/bin/my-tool"), "my-tool");
        assert_eq!(extract_tool_name(""), "unknown");
        assert_eq!(extract_tool_name("/"), "unknown");
    }

    #[test]
    fn test_agent_socket_path_default() {
        // Clear the env var if it exists, then get path
        let _ = std::env::remove_var("CARAPACE_AGENT_SOCKET");
        let path = get_agent_socket_path();
        assert_eq!(path, "/tmp/carapace-agent.sock");
    }

    #[test]
    fn test_agent_socket_path_from_env() {
        std::env::set_var("CARAPACE_AGENT_SOCKET", "/custom/path/socket");
        let path = get_agent_socket_path();
        assert_eq!(path, "/custom/path/socket");
    }
}
