# CLAUDE.md - Development Guide for Claude Code

This document optimizes Carapace development for Claude Code, capturing project structure, workflows, and best practices discovered through implementation.

## 🚨 CRITICAL DEVELOPMENT RULES (ALWAYS FOLLOW)

**These rules are non-negotiable. Follow them on every task.**

1. **Tests First, Always**
   - Write tests BEFORE implementing features (TDD)
   - Write tests to reproduce bugs BEFORE debugging
   - Integration tests that mirror real-world scenarios are best
   - Use `#[tokio::test]` for async tests, never use `#[ignore]`
   - Example: Connection dropping → write test for it → then fix

2. **No Shortcuts Ever**
   - No hardcoded values, debug code, or temporary hacks
   - No test files with `#[ignore]` attributes
   - No random println!() debugging left in code
   - Do it correctly the first time, even if slower

3. **GitHub First, Deploy Second**
   - ALWAYS commit and push to GitHub BEFORE deploying
   - GitHub CI/CD must pass (tests + clippy) before assuming it works
   - Use `git push origin main` and verify GitHub Actions succeed
   - THEN deploy using the deploy script

4. **Always Use deploy.sh**
   - Never manually copy binaries or restart services
   - Let the deploy script handle: stop services, copy files, restart, verify
   - Only exception: if you need to test something very specific locally first
   - Deploy script automatically downloads latest GitHub Actions artifacts

5. **Integration Tests Over Unit Tests**
   - Unit tests in `mod tests` are good but insufficient
   - Integration tests in `tests/` directory with real TCP connections are critical
   - Tests must verify actual system behavior, not mocked behavior
   - Example: Don't mock connection.rs, use actual TcpListener + TcpStream

**Why these rules exist:**
- Previous mistakes: deployed broken code, ignored tests, skipped GitHub CI/CD
- Complex project with security implications → no room for shortcuts
- Tests catch integration bugs unit tests miss (like TCP framing)
- CI/CD catches linting issues local checks miss (like clippy with --all-targets)

---

## Quick Navigation

### Project Structure
```
crates/
├── carapace-server/        # Server daemon - policy enforcement, audit logging
├── carapace-agent/         # Client-side agent - TCP connection, request multiplexing
├── carapace-shim/          # Tool name resolver - symlinked as tool names
├── carapace-policy/        # Policy parsing & matching (glob patterns)
├── carapace-protocol/      # Message framing & serialization (length-prefixed JSON)
├── carapace-debug/         # Debugging toolkit - sniff, audit, health, policy test
└── carapace-multiplexer/   # Request/response correlation by message ID
examples/                   # Example policy files
.github/workflows/          # CI/CD pipelines (test.yml, build.yml)
```

### Key Files to Know

**Architecture & Design:**
- `README.md` - Comprehensive guide with OpenClaw use case prominently featured
- `.github/workflows/test.yml` - Unit tests, integration tests, rustfmt, clippy
- `.github/workflows/build.yml` - Release binary builds for x86_64-gnu target

**Server (Policy Enforcement):**
- `crates/carapace-server/src/main.rs` - TCP listener with `--listen` flag, policy loading
- `crates/carapace-server/src/cli_dispatch.rs` - CLI execution with env_inject merging (lines 70-77 critical)
- `crates/carapace-server/src/audit.rs` - Structured JSON audit logging
- `crates/carapace-policy/src/config.rs` - Policy YAML parsing with `from_file()` method

**Agent (Client-Side):**
- `crates/carapace-agent/src/connection.rs` - TCP connection with auto-reconnection (every 5 sec health check)
- `crates/carapace-agent/src/config.rs` - Reads env vars: CARAPACE_SERVER_HOST, CARAPACE_SERVER_PORT
- `crates/carapace-agent/src/main.rs` - Health monitor task, signal handling
- `crates/carapace-agent/src/cli_handler.rs` - Forwards environment from shim to server

**Shim (Tool Resolver):**
- `crates/carapace-shim/src/main.rs` - Extracts tool name from argv[0], sends CliRequest via Unix socket

**Protocol:**
- `crates/carapace-protocol/src/messages.rs` - CliRequest/CliResponse serialization
- `crates/carapace-protocol/src/framing.rs` - Length-prefixed message encoding/decoding

**Debugging:**
- `crates/carapace-debug/src/main.rs` - Multi-command debug CLI
- `DEBUGGING.md` - Complete guide with examples for all debug commands
- `DEBUG_TOOLS_READY.md` - Quick deployment checklist

## Testing & CI/CD

### Local Testing
```bash
# Run all tests (238 tests total)
cargo test

# Run specific test suite
cargo test --lib -p carapace-server
cargo test --test policy_enforcement_test

# Check formatting (required for CI)
cargo fmt -- --check

# Run clippy with -D warnings (required for CI)
cargo clippy --all-targets --all-features -- -D warnings

# Build release binaries
cargo build --release
```

### GitHub Actions Status
✅ **Tests**: Pass all unit + integration tests, rustfmt, clippy
✅ **Build**: Builds x86_64-unknown-linux-gnu release binaries
- Removed musl target (requires complex OpenSSL vendoring)
- Creates tar.gz archives with binaries + examples + README

### CI/CD Gotchas
- `cargo fmt` must pass before any code commit
- `cargo clippy -- -D warnings` treats all warnings as errors
- Test structs use `#[allow(dead_code)]` because they represent full audit log schema
- All 238 tests must pass (no skips)

## Key Implementation Details

### env_inject Feature (Critical)
**Problem**: VM can't access host credentials; server needs to inject them
**Solution**: Policy defines env vars that override request env vars

**Flow**:
1. Shim captures client environment, sends via Unix socket
2. Agent forwards env to server via TCP in CliRequest.env
3. **Server merges**: `policy.env_inject` overrides `request.env` (lines 70-77 in cli_dispatch.rs)
4. Server executes command with merged environment

**Testing**: Use `printenv-test` tool with TEST_ENV_VAR and OP_SERVICE_ACCOUNT_TOKEN injection

### Auto-Reconnection
- Agent runs health monitor task every 5 seconds
- Checks `connection.is_healthy()`
- Auto-reconnects with exponential backoff if unhealthy
- Can be manually restarted: `sudo systemctl restart carapace-agent.service`

### Message Protocol
- Length-prefixed JSON: `[4-byte length][JSON payload]`
- Prevents shell injection (message-based, not shell-based)
- Used by agent ↔ server and shim ↔ agent communication

### Policy Matching
- **Deny-first semantics**: `argv_deny_patterns` take precedence over `argv_allow_patterns`
- **Glob patterns**: Uses `glob::Pattern` for shell-style matching (`*`, `?`, `[abc]`)
- **Exact vs fuzzy**: `item get *` matches `item get X` but not `item delete X`

## Deployment Pattern

**OpenClaw Use Case** (the primary design goal):
1. **Host (trusted)**: Runs carapace-server with policy, has access to credentials
2. **VM (untrusted)**: Runs carapace-agent, makes requests through policy gateway
3. **Network**: Encrypted via Tailscale WireGuard
4. **Audit**: All operations logged to `/var/log/carapace/audit.log` (JSON format)

**Example Policy** (examples/policies/1password.yaml):
```yaml
tools:
  op:
    type: cli
    binary: /usr/bin/op
    argv_allow_patterns:
      - "item get *"
      - "item list*"
      - "vault list*"
    argv_deny_patterns:
      - "item create *"
      - "item edit *"
      - "item delete *"
    env_inject: {}
    timeout_secs: 30
    audit:
      enabled: true
      log_argv: true
```

## Common Workflows

### Adding a New CLI Tool
1. Create tool binary symlink on VM: `sudo ln -sf /usr/local/bin/carapace-shim /usr/local/bin/mytool`
2. Add policy in `/etc/carapace/policy.yaml`:
   ```yaml
   mytool:
     type: cli
     binary: /usr/bin/mytool
     argv_allow_patterns: ["read *"]
     argv_deny_patterns: ["write *", "delete *"]
     timeout_secs: 30
     audit: {enabled: true, log_argv: true}
   ```
3. Restart server: `sudo systemctl restart carapace-server.service`
4. Test from VM: `mytool read something`

### Using the Debug Toolkit

The `carapace-debug` CLI provides efficient, token-saving debugging without log tailing:

```bash
# Check if server is running
carapace-debug health

# See active connections in real-time
carapace-debug connections --watch 5

# 🌟 SNIFF TCP messages (see exact protocol flow)
carapace-debug sniff --filter HttpRequest

# Query audit log (structured, not grep)
carapace-debug audit --tool signal-cli --since 5m

# Test policy decision without running full system
carapace-debug policy /etc/carapace/policy.yaml '{
  "tool":"signal-cli",
  "method":"send",
  "params":{"recipientNumber":"+12246192888"}
}'
```

**Debugging Workflow for Message Flow Issues:**
1. Run `carapace-debug health` → confirm server is responding
2. Run `carapace-debug connections` → verify agent is connected
3. Run `carapace-debug sniff --filter HttpRequest` → see messages in real-time
4. Send a test message from VM
5. If message appears in sniff: problem is downstream (dispatch/policy)
6. If message doesn't appear: problem is upstream (framing/transmission)

**See `DEBUGGING.md` for complete guide with all examples.**

### Debugging Policy Issues (Old Method - Not Recommended)
1. Check policy file syntax: `cat /etc/carapace/policy.yaml`
2. Use `carapace-debug policy` to test decisions (faster, more accurate)
3. Check server logs: `sudo journalctl -u carapace-server.service -f` (last resort)
4. Verify TCP connection: `nc -zv host.example.com 8765`

### Debugging Test Failures
- Run tests locally: `cargo test --lib` (fast, no integration)
- Run integration tests: `cargo test --test '*'`
- Check specific crate: `cargo test -p carapace-policy`
- Enable backtrace: `RUST_BACKTRACE=1 cargo test`
- Check formatting: `cargo fmt -- --check`
- Check clippy: `cargo clippy --all-targets --all-features`

## Code Patterns & Conventions

### Error Handling
- Use custom `Result<T>` type from each crate's error module
- Server errors → AuditEntry with policy_result="deny"
- Agent errors → reconnection attempt
- Shim errors → exit with code 1, print to stderr

### Testing
- All new code includes unit tests in `#[cfg(test)]` mod
- Integration tests in `tests/` directory
- Test data structures use `#[allow(dead_code)]` for schema documentation
- Mock objects for connection tests

### Logging
- Use `tracing` crate with structured logging
- Set CARAPACE_LOG_LEVEL=debug for verbose output
- Audit logs are immutable JSON, separate from app logs

### Type Safety
- Strongly typed config (PolicyConfig, CliPolicy, etc.)
- Enum types for action_type ("cli" vs "http")
- No stringly-typed critical paths

## Critical Code Sections

### cli_dispatch.rs: env_inject merging (Lines 70-77)
```rust
// Merge policy-injected env vars with request env (policy takes precedence)
let mut merged_env = req.env.clone();
for (key, value) in &cli_policy.env_inject {
    merged_env.insert(key.clone(), value.clone());
}
// Execute the command
let output = self.execute_command(&cli_policy.binary, &req.argv, &merged_env).await?;
```
**Why critical**: Without this, env_inject policy is parsed but never applied

### connection.rs: health monitor loop (Main thread)
```rust
tokio::spawn(async move {
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        if !connection_monitor.is_healthy().await {
            tracing::warn!("Connection unhealthy, attempting automatic reconnection");
            if let Err(e) = connection_monitor.reconnect_if_needed().await {
                tracing::error!("Auto-reconnection failed: {}", e);
            }
        }
    }
});
```
**Why critical**: Enables automatic recovery from network failures

### matcher.rs: deny-first semantics (match_argv function)
```rust
pub fn matches_deny(&self, argv: &[String]) -> bool {
    for pattern in &self.deny_patterns {
        if pattern.matches(argv) {
            return true;  // Deny takes precedence
        }
    }
    false
}

pub fn is_allowed(&self, argv: &[String]) -> bool {
    if self.matches_deny(argv) {
        return false;  // Deny overrides allow
    }
    // Check allow patterns...
}
```
**Why critical**: Policy enforcement correctness - deny must always win

## Git Workflow

### Before Committing
```bash
cargo fmt              # Format code
cargo test             # Run all tests (238 should pass)
cargo clippy --all-targets --all-features -- -D warnings  # Check linting
```

### Commit Message Pattern
```
Short description (< 50 chars)

Optional longer explanation of why this change.
- Bullet point for implementation detail
- Another detail

🤖 Generated with Claude Code

Co-Authored-By: Claude Haiku 4.5 <noreply@anthropic.com>
```

### Push to GitHub
```bash
git push origin main
# GitHub Actions will automatically:
# 1. Run tests (40-60 sec)
# 2. Run build for x86_64-gnu (1-2 min)
# Check status: gh run list --limit 5
```

## Quick Reference Commands

```bash
# Development
cargo build                          # Debug build
cargo build --release               # Optimized binaries
cargo test                           # All tests
cargo test -p carapace-server       # Single crate tests
cargo fmt                            # Auto-format
cargo clippy --all-targets --all-features -- -D warnings  # Linting with warnings-as-errors

# Debugging (no log tailing!)
carapace-debug health                # Server health check
carapace-debug connections           # Active connections
carapace-debug sniff                 # Capture TCP messages (key for troubleshooting)
carapace-debug audit --tool X        # Query audit log
carapace-debug policy policy.yaml '{request}'  # Test policy decision

# Local Testing
RUST_LOG=debug cargo run --bin carapace-server      # Server with debug logs
RUST_LOG=debug cargo run --bin carapace-agent       # Agent with debug logs

# GitHub Actions
gh run list --limit 5                # Show recent workflow runs
gh run view <RUN_ID> --log           # View detailed logs
gh workflow list                     # Show available workflows

# Git
git log --oneline -5                 # Recent commits
git diff main                        # Changes not yet committed
git status                           # Working tree status
```

## Known Limitations & Future Work

### Current Limitations
- ✋ Early-stage software, not battle-tested
- No policy hot-reload (requires server restart)
- Limited to Linux + systemd
- Length-prefixed JSON framing (not ideal for streaming large data)
- Manual reconnection fallback if health check fails

### Future Improvements
- Policy hot-reload via SIGHUP
- Support for macOS/Windows
- Streaming response support for large outputs
- Per-client rate limiting (currently per-tool only)
- More sophisticated timing attack mitigation
- musl target support (removed for now due to OpenSSL complexity)

## Architecture Philosophy

**Design principle**: Trust policy, not transport
- Carapace assumes Tailscale/VPN encryption
- Focuses on **authorization** (what operations allowed), not authentication
- Policy enforcement is the security boundary
- Audit logging provides forensic capability, not prevention

**Dependency philosophy**:
- Use async/await (tokio) for I/O
- Use serde for structured data
- Prefer small, focused crates
- Avoid heavy frameworks

**Testing philosophy**:
- Unit tests in-crate (fast, no I/O)
- Integration tests verify end-to-end flows
- Test data structures document schema
- All tests must pass in CI/CD before merge

## Resources

- RFC: OpenClaw problem statement in README.md
- Example deployment: examples/policies/1password.yaml
- Example signal-cli policy: examples/policies/signal-cli.yaml
- Threat model: README.md "Security Considerations" section
- Deployment guide: README.md "Deployment Example" sections
- Debugging guide: DEBUGGING.md (complete with examples)
- Quick debug checklist: DEBUG_TOOLS_READY.md

---

**Last Updated**: 2026-02-15
**Crates**: 6 (added carapace-debug)
**Test Coverage**: All passing
**CI/CD Status**: ✅ Tests & Build workflows passing
**Debug Toolkit**: ✅ Ready (5 subcommands: health, connections, sniff, audit, policy)
