# Carapace Deployment Guide

This guide covers deploying Carapace with real-time SSE streaming support for production use cases like OpenClaw.

## Overview

The Carapace system consists of:
- **carapace-server**: Central policy enforcement daemon (runs on host with credentials)
- **carapace-agent**: Client-side agent (runs on VM/container without credentials)
- **carapace-shim**: Tool name resolver (symlinked as tool names on VM)

Data flow: Tool → Shim → Agent → Server (policy) → Upstream tool

## System Requirements

- Linux with systemd
- Rust toolchain (for building) or pre-built binaries
- signal-cli daemon (for Signal integration, optional)
- Tailscale VPN (recommended for network security)

## Pre-Deployment: Build Release Binaries

```bash
# Build optimized binaries
cargo build --release

# Binaries are in target/release/:
# - carapace-server
# - carapace-agent
# - carapace-shim
# - carapace-debug
```

## Host Deployment (austin-ubuntu-desktop)

### 1. Install Binaries

```bash
# Create installation directory
sudo mkdir -p /usr/local/bin
sudo mkdir -p /etc/carapace

# Copy binaries
sudo cp target/release/carapace-server /usr/local/bin/
sudo cp target/release/carapace-debug /usr/local/bin/
sudo chmod +x /usr/local/bin/carapace-*

# Verify installation
carapace-server --version
carapace-debug --version
```

### 2. Install signal-cli Daemon (if using Signal integration)

```bash
# Download signal-cli 0.13.24 (same version as VM)
wget https://github.com/AsamK/signal-cli/releases/download/v0.13.24/signal-cli-0.13.24-Linux.tar.gz
tar xf signal-cli-0.13.24-Linux.tar.gz
sudo mv signal-cli-0.13.24 /opt/signal-cli
sudo ln -sf /opt/signal-cli/bin/signal-cli /usr/local/bin/signal-cli

# Migrate account keys from VM (restore from backup)
# The key migration happens via account restoration, not shown here

# Create systemd service
sudo tee /etc/systemd/system/signal-cli-daemon.service <<'EOF'
[Unit]
Description=signal-cli Daemon
After=network.target

[Service]
Type=simple
User=austin
# Signal account number - update to your number
ExecStart=/usr/local/bin/signal-cli -a +12242120288 daemon --http 127.0.0.1:18080 --no-receive-stdout
Restart=on-failure
RestartSec=5s
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable signal-cli-daemon.service
sudo systemctl start signal-cli-daemon.service

# Verify
curl http://127.0.0.1:18080/api/v1/about
```

### 3. Create Carapace Policy

Create `/etc/carapace/policy.yaml`:

```yaml
tools:
  signal-cli:
    type: http
    upstream: "http://127.0.0.1:18080"
    jsonrpc_allow_methods:
      - send
      - receive
      - listMessages
      - version
    jsonrpc_deny_methods:
      - deleteEverything
      - removeAccount
    jsonrpc_param_filters:
      send:
        field: "recipientNumber"
        allow_patterns:
          - "+1*"  # Allow all US numbers
        deny_patterns:
          - "+15551234567"  # Block spam numbers (customize)
    rate_limit:
      max_requests: 100
      window_secs: 60
    timeout_secs: 30
    audit:
      enabled: true
      log_body: false
```

### 4. Create Carapace Server Systemd Service

Create `/etc/systemd/system/carapace-server.service`:

```ini
[Unit]
Description=Carapace Policy Server
After=network.target signal-cli-daemon.service
Wants=signal-cli-daemon.service

[Service]
Type=simple
User=austin
Group=austin
Environment="CARAPACE_LOG_LEVEL=info"
Environment="CARAPACE_LOG_JSON=true"
Environment="CARAPACE_POLICY_FILE=/etc/carapace/policy.yaml"
ExecStart=/usr/local/bin/carapace-server --listen 127.0.0.1:8765
Restart=on-failure
RestartSec=5s
StandardOutput=journal
StandardError=journal

# Security hardening
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/var/log/carapace

[Install]
WantedBy=multi-user.target
EOF

# Create audit log directory
sudo mkdir -p /var/log/carapace
sudo chown austin:austin /var/log/carapace
sudo chmod 750 /var/log/carapace

# Enable and start
sudo systemctl daemon-reload
sudo systemctl enable carapace-server.service
sudo systemctl start carapace-server.service

# Verify
sudo systemctl status carapace-server.service
journalctl -u carapace-server.service -f
```

### 5. Verify Host Deployment

```bash
# Check server is listening
ss -tlnp | grep 8765
# Should show: 127.0.0.1:8765 (LISTEN)

# Check audit log is working
ls -la /var/log/carapace/audit.log

# Test with debug toolkit
carapace-debug health --port 8765
# Should show: Server is healthy, 1 connection

# Test signal-cli integration
curl -X POST http://127.0.0.1:18080/api/v1/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":"1","method":"version","params":{}}'
# Should return version info
```

## VM Deployment (claw.orca-puffin.ts.net)

### 1. Install Agent Binary

```bash
# On host: Copy agent to VM
scp target/release/carapace-agent claw@claw.orca-puffin.ts.net:~

# On VM:
ssh claw@claw.orca-puffin.ts.net
sudo mv carapace-agent /usr/local/bin/
sudo chmod +x /usr/local/bin/carapace-agent
```

### 2. Create Agent Systemd Service

Create `/etc/systemd/system/carapace-agent.service`:

```ini
[Unit]
Description=Carapace Agent - Capability Daemon
After=network.target

[Service]
Type=simple
User=claw
Environment="CARAPACE_SERVER_HOST=austin-ubuntu-desktop.orca-puffin.ts.net"
Environment="CARAPACE_SERVER_PORT=8765"
Environment="CARAPACE_HTTP_PORT=8080"
Environment="CARAPACE_HTTP_LISTEN_ADDR=127.0.0.1"
Environment="CARAPACE_LOG_LEVEL=info"
Environment="CARAPACE_LOG_JSON=true"
ExecStart=/usr/local/bin/carapace-agent
Restart=on-failure
RestartSec=5s
StandardOutput=journal
StandardError=journal

# Security hardening
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
EOF

# Enable and start
sudo systemctl daemon-reload
sudo systemctl enable carapace-agent.service
sudo systemctl start carapace-agent.service

# Verify
sudo systemctl status carapace-agent.service
sudo journalctl -u carapace-agent.service -f
```

### 3. Install Shim Symlinks

The carapace-shim allows tools to be transparently proxied through Carapace.

```bash
# Copy shim binary
sudo cp target/release/carapace-shim /usr/local/bin/
sudo chmod +x /usr/local/bin/carapace-shim

# For each tool you want to proxy, create a symlink
# This causes the tool to be intercepted and sent to carapace-agent

# Example: proxy signal-cli through Carapace
# (This replaces direct signal-cli access with policy-enforced version)
# sudo ln -sf /usr/local/bin/carapace-shim /usr/local/bin/signal-cli

# Note: Be careful with symlinks - they redirect ALL calls to that tool
# through Carapace. Only use for tools where you want policy enforcement.
```

### 4. Verify VM Deployment

```bash
# Check agent is running
sudo systemctl status carapace-agent.service

# Check it can reach server
curl http://127.0.0.1:8080/health
# Should return: {"status":"ok","connections":1}

# Check OpenClaw can make requests through agent
# (Agent listens at 127.0.0.1:8080 where OpenClaw expects signal-cli)
curl -X POST http://127.0.0.1:8080/api/v1/rpc \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":"1","method":"version","params":{}}'
```

## Monitoring and Troubleshooting

### Check Server Health

```bash
# Using debug toolkit (fastest way)
carapace-debug health

# Using systemd
sudo systemctl status carapace-server.service
sudo journalctl -u carapace-server.service -n 50

# Direct TCP test
nc -zv localhost 8765
```

### Monitor Active Connections

```bash
# Real-time connection monitoring
carapace-debug connections --watch 5

# Using netstat
ss -tlnp | grep 8765
```

### Debug SSE Streaming

```bash
# 1. Watch messages in real-time
carapace-debug sniff --filter SseEvent

# 2. In another terminal, send a message through agent that triggers SSE
# This should show SseEvent messages arriving in real-time (<100ms latency)

# 3. Verify no 2-second buffering
# Events should arrive continuously, not in batches
```

### Test SSE Streaming Performance

```bash
# Generate test SSE events and verify delivery
carapace-debug sse-test --count 10 --interval-ms 100

# While running, in another terminal:
carapace-debug sniff --filter SseEvent

# Verify all 10 events arrive quickly with proper spacing
```

### Query Audit Logs

```bash
# Show all requests
carapace-debug audit

# Show signal-cli requests
carapace-debug audit --tool signal-cli

# Show denials (policy blocks)
carapace-debug audit --result deny

# Show recent activity
carapace-debug audit --since 5m

# Follow mode (tail new entries)
carapace-debug audit --follow
```

### Check Policy Decisions

```bash
# Test if a request would be allowed
carapace-debug policy /etc/carapace/policy.yaml '{
  "tool": "signal-cli",
  "method": "send",
  "params": {"recipientNumber": "+15551234567"}
}'

# Expected: shows allow/deny decision and why
```

## Troubleshooting Guide

### Server won't start

```bash
# Check policy file syntax
cat /etc/carapace/policy.yaml

# Check server logs
sudo journalctl -u carapace-server.service -f

# Verify signal-cli daemon is running (if configured)
sudo systemctl status signal-cli-daemon.service

# Check port isn't already in use
ss -tlnp | grep 8765
```

### Agent can't reach server

```bash
# Verify network connectivity
ping -c 1 austin-ubuntu-desktop.orca-puffin.ts.net

# Check Tailscale is connected (if using VPN)
tailscale status

# Verify server is listening
ssh austin@austin-ubuntu-desktop.orca-puffin.ts.net \
  "ss -tlnp | grep 8765"

# Check agent logs
sudo journalctl -u carapace-agent.service -f

# Manual TCP test from VM
nc -zv austin-ubuntu-desktop.orca-puffin.ts.net 8765
```

### SSE events not arriving in real-time

```bash
# 1. Verify streaming is enabled (should be default)
carapace-debug sniff --filter SseEvent
# Should show SseEvent messages, not just HttpResponse

# 2. Check latency
# Events should arrive in <100ms, not 2+ seconds
carapace-debug sse-test --count 5 --interval-ms 100
# Run in one terminal, sniff in another
# Verify all 5 events arrive quickly

# 3. Check server implementation
# If seeing HttpResponse instead of SseEvent, check:
# - listener.rs: is sse_event_tx being passed?
# - http_dispatch.rs: is bytes_stream() being used?
# - Rebuild and deploy latest code

# 4. Check agent implementation
# If events arrive at server but not at client:
# - http_proxy.rs: is handle_events() looping on rx.recv()?
# - Rebuild agent and restart carapace-agent.service
```

### Audit logs not appearing

```bash
# Verify audit log exists and is writable
ls -la /var/log/carapace/audit.log

# Check permissions
sudo chown austin:austin /var/log/carapace/audit.log

# Verify audit is enabled in policy
grep -A 3 "audit:" /etc/carapace/policy.yaml

# Check server logs for audit errors
sudo journalctl -u carapace-server.service | grep -i audit

# Manually query audit log
carapace-debug audit --limit 10
```

## Performance Testing

### Measure SSE Latency

```bash
# Before: 2+ seconds (old buffering)
# After: <100ms (real-time streaming)

time carapace-debug sse-test --count 20 --interval-ms 50

# Should complete in ~1 second (20 events × 50ms = 1000ms)
# Plus a small overhead
```

### Load Testing

```bash
# Simulate concurrent requests (coming soon)
# For now, monitor under load:

carapace-debug connections --watch 2  # Watch connection count
carapace-debug audit --follow         # Watch audit log grow
```

## Updating Carapace

### Update Server

```bash
# Rebuild
cargo build --release

# Stop server
sudo systemctl stop carapace-server.service

# Update binary
sudo cp target/release/carapace-server /usr/local/bin/
sudo chmod +x /usr/local/bin/carapace-server

# Restart
sudo systemctl start carapace-server.service
sudo systemctl status carapace-server.service
```

### Update Agent

```bash
# Rebuild
cargo build --release

# Copy to VM
scp target/release/carapace-agent claw@claw.orca-puffin.ts.net:~

# On VM:
ssh claw@claw.orca-puffin.ts.net
sudo mv carapace-agent /usr/local/bin/
sudo systemctl restart carapace-agent.service
sudo systemctl status carapace-agent.service
```

### Update Policy

```bash
# Edit policy
sudo nano /etc/carapace/policy.yaml

# Restart server (no hot-reload yet)
sudo systemctl restart carapace-server.service

# Verify changes took effect
carapace-debug policy /etc/carapace/policy.yaml '{...}'
```

## Rollback Procedure

If something breaks:

```bash
# 1. Identify last known good version
git log --oneline -5

# 2. Checkout previous version
git checkout <commit-hash>

# 3. Rebuild
cargo build --release

# 4. Redeploy
sudo cp target/release/carapace-server /usr/local/bin/
sudo systemctl restart carapace-server.service

# 5. Verify
carapace-debug health
```

## Security Considerations

### Credential Protection

- **Server**: Has access to signal-cli (with message encryption keys)
  - Runs as unprivileged `austin` user (not root)
  - Audit logging captures what credentials are used
  - Policy enforces which operations are allowed

- **Agent/VM**: No direct access to credentials
  - Sends requests to server over secure channel
  - Policy decisions happen on server, not agent
  - Audit logs don't contain sensitive data by default

### Network Security

- Use Tailscale WireGuard for encryption (configured in setup)
- Avoid exposing carapace ports on internet
- Firewall: Only allow agent to reach server, not vice versa

### Audit Trail

All requests logged to `/var/log/carapace/audit.log`:
```json
{
  "timestamp": "2026-02-15T...",
  "request_id": "...",
  "tool": "signal-cli",
  "action_type": "http",
  "policy_result": "allow",
  "method": "send",
  "phone_number": "+15551234567",
  ...
}
```

### Hardening

- Run servers as unprivileged users (not root)
- Use systemd security directives: `NoNewPrivileges`, `PrivateTmp`, `ProtectSystem`
- Restrict audit log file permissions (750)
- Use strong policy rules (deny by default)

## Next Steps

After deployment:

1. **Verify with OpenClaw**: Integration tests with actual OpenClaw instance
2. **Monitor in production**: Check audit logs daily for policy violations
3. **Performance baseline**: Measure response times and throughput
4. **Document customizations**: Record any policy changes for your use case
5. **Disaster recovery**: Test backup/restore of audit logs

## Support

For issues:

1. Check systemd logs: `journalctl -u carapace-*`
2. Use debug toolkit: `carapace-debug health`, `carapace-debug sniff`
3. Query audit logs: `carapace-debug audit --since 5m`
4. Test policy: `carapace-debug policy /etc/carapace/policy.yaml '{...}'`

---

**Last Updated**: 2026-02-15
**Tested with**: signal-cli 0.13.24, Rust 1.70+, Linux 5.10+
