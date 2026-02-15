# Carapace 🐚

A capability daemon system for secure, policy-based execution of privileged operations across trust boundaries using message-based communication.

**⚠️ EARLY STAGE - HERE BE DRAGONS ⚠️**

This project is in active development and **NOT production-ready**. While the core architecture and features are functional, this has not been battle-tested in production environments. Use at your own risk.

## Purpose: OpenClaw Use Case

Carapace was built to solve the **OpenClaw** problem:

> How do you run untrusted or potentially compromised code in an isolated VM while still letting it securely access host resources (secrets, APIs, CLIs)?

**The Problem:**
- You need to run development tools or experimental code on a VM
- That code needs access to secrets (1Password, API keys, etc.)
- You can't store credentials on the VM (it's untrusted)
- You can't give the VM direct access to host resources (too much privilege)
- You need full audit trail of what the code accessed

**The Solution (Carapace):**
- Create a **capability-based gateway** between untrusted VM and trusted host
- Define exactly what operations are allowed via policy (read-only, specific commands)
- Proxy requests through the gateway with full auditing
- Credentials stay on the host, never exposed to VM

**Example: 1Password CLI**

Your untrusted VM needs to access secrets:

```
VM: $ op item list
    ↓ (intercepted by carapace-shim)

Shim sends: {"tool": "op", "argv": ["item", "list"]}
    ↓ (over Tailscale encrypted connection)

Server: Validates against policy
  ✓ "item list*" is in allow_patterns
  ✗ "item delete" is in deny_patterns
  ✓ Request allowed
    ↓
Server: Executes as host user with credentials
  $ op item list (with OP_SESSION from env_inject)
    ↓
Server: Returns {"exit_code": 0, "stdout": "...items..."}
    ↓ (back over Tailscale)

VM: Receives list of items
```

**Result:** The VM process has **zero direct access to credentials** - it can only ask for operations defined in the policy. All operations are audited.

## Overview

Carapace enables secure remote execution of CLI commands and HTTP requests with:

- **Policy-based authorization**: Define what commands/APIs are allowed via YAML policies
- **Message-based protocol**: Length-prefixed JSON framing prevents shell injection
- **Audit logging**: All operations logged with timestamps and policy decisions
- **Environment variable injection**: Securely pass credentials through `env_inject`
- **Network isolation**: Works over TCP (e.g., Tailscale, VPN) with optional encryption
- **Automatic reconnection**: Agent auto-detects and reconnects to server
- **Rate limiting**: Configurable per-tool request throttling (HTTP)
- **Multiple transport modes**: Unix sockets (SSH), TCP, or HTTP proxy

## Architecture

```
Client Machine (untrusted)        Host Machine (trusted)
     User                              System Services
       ↓                                      ↓
  carapace-shim                       carapace-server
  (symlinked as tool)                 (systemd service)
       ↓                                      ↓
  /tmp/carapace-agent.sock  ←→  127.0.0.1:8765 (TCP)
       ↑                                      ↑
  carapace-agent                   Policy Enforcement
  (systemd service)                  + Audit Logging
       ↓                                      ↓
  (send request via                (execute with policy,
   Unix socket)                     env injection, auditing)
```

## Quick Start

### Prerequisites

- Rust 1.70+ (for building)
- Linux (systemd for service management)
- Tailscale, VPN, or SSH tunnel (for network isolation)

### Building

```bash
cargo build --release

# Binaries in target/release/:
# - carapace-server    (main server daemon)
# - carapace-agent     (client-side agent)
# - carapace-shim      (tool name resolver)
```

### Deployment Example: OpenClaw + 1Password

This example shows the OpenClaw use case: An untrusted development VM needs to access 1Password secrets from a trusted host machine.

**Setup:**
- **Host (trusted)**: Ubuntu machine where 1Password is installed and user is logged in
- **VM (untrusted)**: Development VM (possibly compromised or low-trust) that needs to run `op` commands

**Goal:** Let the VM run read-only 1Password operations without exposing credentials.

#### 1. Host Setup (Ubuntu)

```bash
# Install 1Password CLI
curl -sS https://downloads.1password.com/linux/keys/1password.asc | sudo gpg --dearmor --output /usr/share/keyrings/1password-archive-keyring.gpg
echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/1password-archive-keyring.gpg] https://downloads.1password.com/linux/debian/$(dpkg --print-architecture) stable main" | sudo tee /etc/apt/sources.list.d/1password.list
sudo apt update && sudo apt install -y 1password-cli

# Create policy
sudo mkdir -p /etc/carapace
sudo tee /etc/carapace/policy.yaml <<EOF
tools:
  op:
    type: cli
    binary: /usr/bin/op
    argv_allow_patterns:
      - "item get *"
      - "item list*"
      - "vault list*"
      - "--version"
    argv_deny_patterns:
      - "item create *"
      - "item edit *"
      - "item delete *"
    env_inject:
      OP_SERVICE_ACCOUNT_TOKEN: "ops_..."  # Your 1Password service account token
      HOME: "/home/austin"                  # Override to host user home
    timeout_secs: 30
    audit:
      enabled: true
      log_argv: true
EOF

# Deploy server
sudo cp target/release/carapace-server /usr/local/bin/
sudo chmod +x /usr/local/bin/carapace-server

# Create systemd service
sudo tee /etc/systemd/system/carapace-server.service <<EOF
[Unit]
Description=Carapace Server
After=network.target

[Service]
Type=simple
User=$USER
Environment="CARAPACE_POLICY_FILE=/etc/carapace/policy.yaml"
Environment="CARAPACE_LOG_LEVEL=info"
ExecStart=/usr/local/bin/carapace-server --listen 0.0.0.0:8765
Restart=on-failure
RestartSec=5s
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable carapace-server.service
sudo systemctl start carapace-server.service
```

#### 2. Client Setup (untrusted VM)

```bash
# Deploy binaries
sudo cp target/release/carapace-agent /usr/local/bin/
sudo cp target/release/carapace-shim /usr/local/bin/
sudo chmod +x /usr/local/bin/carapace-{agent,shim}

# Create symlink to shim (all tool names resolve through it)
sudo ln -sf /usr/local/bin/carapace-shim /usr/local/bin/op

# Configure agent
sudo mkdir -p /etc/carapace
sudo tee /etc/carapace/agent.env <<EOF
CARAPACE_SERVER_HOST=host.tailscale.net
CARAPACE_SERVER_PORT=8765
CARAPACE_CLI_SOCKET=/tmp/carapace-agent.sock
CARAPACE_LOG_LEVEL=info
EOF

# Create systemd service
sudo tee /etc/systemd/system/carapace-agent.service <<EOF
[Unit]
Description=Carapace Agent
After=network.target

[Service]
Type=simple
User=$USER
EnvironmentFile=/etc/carapace/agent.env
ExecStart=/usr/local/bin/carapace-agent
Restart=on-failure
RestartSec=5s
NoNewPrivileges=true

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable carapace-agent.service
sudo systemctl start carapace-agent.service
```

#### 3. Test

```bash
# From client machine
op --version
op item list
op item get "My Password"

# These will be denied (defined in policy)
op item create --category=login --title="Test"  # DENIED
op item delete "some-item"                       # DENIED
```

Check audit logs on host:
```bash
sudo tail -f /var/log/carapace/audit.log
```

## Configuration

### Server Policy (`/etc/carapace/policy.yaml`)

```yaml
tools:
  tool_name:
    type: cli                          # or: http
    binary: /path/to/binary            # For CLI tools

    argv_allow_patterns:               # Glob patterns (allow-list)
      - "get *"
      - "list"

    argv_deny_patterns:                # Glob patterns (deny takes precedence)
      - "delete *"
      - "create *"

    env_inject:                        # Inject env vars (policy precedence)
      HOME: "/home/targetuser"
      SECRET_TOKEN: "***"

    cwd_allowed:                       # Optional: allowed working directories
      - "/tmp"
      - "/home/user"

    timeout_secs: 30                   # Command timeout

    audit:
      enabled: true
      log_argv: true                   # Log command arguments
      redact_patterns:                 # Patterns to redact in logs
        - "--session"
        - "token"

  http_service:
    type: http
    upstream: "http://localhost:8080"  # Target HTTP service

    jsonrpc_allow_methods:             # For JSON-RPC services
      - "method_name"

    rate_limit:
      max_requests: 100
      window_secs: 60
```

### Agent Configuration

Set via environment variables or `/etc/carapace/agent.env`:

```bash
CARAPACE_SERVER_HOST=host.example.com
CARAPACE_SERVER_PORT=8765
CARAPACE_CLI_SOCKET=/tmp/carapace-agent.sock
CARAPACE_HTTP_PORT=8080
CARAPACE_LOG_LEVEL=info|debug|warn|error
CARAPACE_LOG_JSON=true|false
```

## Features

### Policy Enforcement

- **Deny-first semantics**: Deny patterns take precedence over allow patterns
- **Glob pattern matching**: Shell-style wildcards (`*`, `?`, `[abc]`)
- **Redaction**: Sensitive patterns redacted from audit logs

### Environment Variable Injection

Securely pass credentials through `env_inject`:

```yaml
tools:
  mytool:
    env_inject:
      DATABASE_PASSWORD: "secret123"
      API_KEY: "sk_live_..."
      HOME: "/root"  # Override home directory
```

Variables in the policy override those from the client, ensuring credentials stay on the host.

### Audit Logging

JSON-formatted audit logs with:
- Timestamp
- Tool name
- Command arguments (optional)
- Environment variable names (values redacted)
- Policy decision (allow/deny)
- Exit code
- Execution duration

### Automatic Reconnection

The agent monitors TCP connection health every 5 seconds and automatically reconnects if needed.

### Rate Limiting (HTTP only)

Limit requests per tool:
```yaml
tools:
  api_service:
    type: http
    rate_limit:
      max_requests: 100
      window_secs: 60
```

## Security Considerations

### Threat Model: OpenClaw Use Case

Carapace assumes this threat model:

- **Untrusted client machine** (VM): May be compromised, may run untrusted code
- **Trusted host machine**: Not fully compromised (attacker is unprivileged)
- **Network path**: Encrypted (Tailscale, VPN, SSH tunnel) - not exposed to internet
- **Credentials**: Stored **only on host**, never sent to untrusted machines

**Protection goal**: The untrusted VM can access specific resources (1Password, APIs, CLIs) only through policy-enforced capabilities, never directly.

### What Carapace Protects Against

✅ **Shell injection**: Message-based protocol prevents `; rm -rf /` attacks
✅ **Credential exposure**: Policy-based env_inject keeps secrets on host, not sent to VM
✅ **Unauthorized operations**: Deny patterns block destructive commands (e.g., `item create`, `item delete`)
✅ **Direct access**: VM can't bypass Carapace to access host resources directly
✅ **Audit trail**: All operations logged immutably with timestamps and policy decisions
✅ **Network sniffing**: Encrypted by Tailscale/VPN (Carapace assumes secure transport)

### What Carapace Does NOT Protect Against

❌ **Compromised host**: If the server machine is fully compromised, attackers can read policies/credentials
❌ **Network eavesdropping**: Requires Tailscale, VPN, or SSH tunnel - raw TCP over internet is **NOT secure**
❌ **Privilege escalation**: The server runs with user privileges (configure with `User=` in systemd)
❌ **Logic bugs**: This is early-stage software - test thoroughly before production use
❌ **Resource exhaustion**: Rate limiting is per-tool, but no per-client limits
❌ **Timing attacks**: If policy evaluation time leaks information, attacker might infer decisions

### Best Practices

1. **Use network isolation**: Deploy over Tailscale, VPN, or SSH tunnels (not raw TCP over internet)
2. **Principle of least privilege**: Run server with minimal user/permissions
3. **Tight policies**: Use specific allow patterns, not broad wildcards
4. **Monitor logs**: Regularly review audit logs for suspicious activity
5. **Rotate credentials**: Change service account tokens/passwords periodically
6. **Test policies**: Verify allow/deny patterns work as expected before deployment

## Troubleshooting

### Agent can't connect to server

```bash
# Check server is running
ssh user@host "sudo systemctl status carapace-server"

# Test network connectivity
nc -zv host.example.com 8765

# Check Tailscale/firewall rules allow traffic
tailscale status
```

### Commands timeout

Increase `timeout_secs` in policy:
```yaml
tools:
  mytool:
    timeout_secs: 60  # Was 30
```

### Audit logs not appearing

Check server has write access to `/var/log/carapace/`:
```bash
sudo ls -la /var/log/carapace/
sudo tail -f /var/log/carapace/audit.log
```

### Policy changes not applied

Restart server:
```bash
sudo systemctl restart carapace-server.service
```

## Known Limitations

- **Early stage**: Not production-tested
- **Manual reconnection fallback**: If health check fails, manual restart may be needed
- **No policy hot-reload**: Requires server restart to apply policy changes
- **Limited platforms**: Linux + systemd (could be ported to other OSes)
- **Message framing**: Length-prefixed JSON (not ideal for streaming large data)

## Development

```bash
# Run tests
cargo test

# Build debug binaries
cargo build

# Build release binaries
cargo build --release

# Run with debug logging
RUST_LOG=debug cargo run --bin carapace-server
```

### Project Structure

```
crates/
├── carapace-agent/      # Client-side agent (relays requests via TCP)
├── carapace-server/     # Server daemon (policy enforcement, execution)
├── carapace-shim/       # Tool name resolver (symlinked as tools)
├── carapace-policy/     # Policy parser and enforcement logic
├── carapace-protocol/   # Message framing and serialization
└── carapace-multiplexer/ # Request/response correlation
examples/
└── policies/            # Example policy files
```

## Contributing

This is an early-stage experimental project. Contributions welcome!

- Bug reports and feature requests: GitHub Issues
- Code improvements: Pull requests (please include tests)
- Documentation: Improvements to README and inline comments

## Disclaimer

⚠️ **USE AT YOUR OWN RISK** ⚠️

This software is provided as-is for experimental use. The authors make no warranties about security, reliability, or suitability for any purpose. Do NOT use in production environments until thoroughly tested and audited.

The threat model assumes:
- The host machine is trusted and not fully compromised
- Network traffic is encrypted (Tailscale, VPN, SSH tunnel)
- Policies are carefully crafted and tested
- Audit logs are monitored and retained

## License

[Your License Here - e.g., Apache 2.0, MIT, etc.]

## Authors

- Austin Lin

---

**Questions?** Check the examples/ directory or open an issue on GitHub.
