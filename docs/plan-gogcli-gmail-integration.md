# Plan: gogcli Gmail Integration via Carapace

## Problem Statement

OpenClaw uses [gogcli](https://github.com/steipete/gogcli) (`gog` binary) as its interface to Google Workspace services, including Gmail. Today, OpenClaw's security model for `gog` relies on LLM guidance (SKILL.md) rather than hard enforcement -- a prompt-injected agent can invoke any `gog` subcommand regardless of what the skill description says.

Carapace can provide hard policy enforcement for `gog`, but Gmail introduces challenges beyond what the existing 1Password and Signal integrations handle:

1. **Structured output inspection**: Blocking "fetch emails with password reset links" requires inspecting *response content*, not just request arguments. Carapace currently only does pre-execution policy (argv matching).
2. **Rich command surface**: `gog gmail` has ~15 subcommands across search, send, labels, drafts, attachments, settings, and watch -- more complex than `op` or `signal-cli`.
3. **Credential model**: gogcli stores OAuth tokens in `~/.config/gog/` using OS keyring or encrypted file backend, different from 1Password's service account token.

## Architecture Overview

There are two distinct data flows to support: **ad-hoc CLI queries** (agent initiates) and **real-time push notifications** (Gmail initiates via Pub/Sub). Both need content filtering.

### Flow A: Ad-hoc CLI Queries (agent-initiated)

```
UNTRUSTED VM (OpenClaw agent)              TRUSTED HOST (credentials)
┌─────────────────────────────┐            ┌──────────────────────────────────────┐
│                             │            │                                      │
│  $ gog gmail search '...'   │            │  carapace-server                     │
│    ↓                        │            │    ↓                                 │
│  carapace-shim              │            │  1. argv policy check                │
│    ↓                        │   TCP      │  2. env_inject (GOG_ACCOUNT, etc.)   │
│  carapace-agent ──────────────────────►  │  3. execute: /usr/local/bin/gog      │
│                             │            │  4. ** response filter ** (NEW)       │
│                             │            │  5. return filtered CliResponse       │
│                             │            │                                      │
│                             │            │  ~/.config/gog/  (OAuth tokens)      │
│                             │            │  GOG_KEYRING_PASSWORD (in env_inject) │
└─────────────────────────────┘            └──────────────────────────────────────┘
```

### Flow B: Gmail Watch / Pub/Sub (Gmail-initiated, real-time)

This is how OpenClaw primarily uses Gmail. It's not optional -- the Gateway auto-starts
`gog gmail watch serve` on boot when configured and auto-renews the 7-day watch expiry.
OpenClaw has dedicated source files for this: `gmail-watcher.ts`, `gmail-watcher-lifecycle.ts`,
`gmail-ops.ts`. Real users run this in production.

```
Gmail inbox change
    │
    ▼
Google Pub/Sub push notification (minimal: email + historyId)
    │
    ▼
┌──────────────────────────── TRUSTED HOST ─────────────────────────────────────┐
│                                                                               │
│  gog gmail watch serve (long-lived daemon)                                    │
│    - Enriches notification via Gmail History API (from, subject, snippet, body)│
│    - Posts enriched JSON to local webhook                                      │
│    │                                                                          │
│    ▼                                                                          │
│  carapace-server webhook endpoint (NEW)                                       │
│    1. ** content filter ** on notification payload                             │
│    2. Drop/redact messages matching deny patterns (password resets, etc.)      │
│    3. Push filtered events as SseEvent to connected agents                    │
│                                                                               │
│  ~/.config/gog/  (OAuth tokens, never leave trusted host)                     │
└───────────────────────────────────────────┬───────────────────────────────────┘
                                            │ TCP (SseEvent)
                                            ▼
┌──────────────────────── UNTRUSTED VM ─────────────────────────────────────────┐
│                                                                               │
│  carapace-agent receives SseEvent                                             │
│    │                                                                          │
│    ▼                                                                          │
│  OpenClaw hooks endpoint (http://127.0.0.1:18789/hooks/gmail)                 │
│    - AI processes filtered email notification                                 │
│    - Summarize / classify / draft reply / route to chat                       │
│                                                                               │
└───────────────────────────────────────────────────────────────────────────────┘
```

Key insight: `gog gmail watch serve` posts enriched notifications (from, subject,
snippet, body) to a webhook URL. Instead of pointing it directly at OpenClaw's hooks
endpoint (which would bypass Carapace entirely), we point it at a Carapace webhook
receiver that applies content filters, then pushes filtered events over the existing
SseEvent channel to agents. The same `ResponseFilter` framework from Phase 2 handles
both flows.

## Phase 1: Basic gogcli Policy (argv + env_inject)

**Goal**: Get `gog gmail` working through Carapace with the same deny-first argv policy model used by 1Password.

### 1a. Example policy file

Create `examples/policies/gogcli-gmail.yaml`:

```yaml
tools:
  gog:
    type: cli
    binary: /usr/local/bin/gog

    argv_allow_patterns:
      # Search and read
      - "gmail search *"
      - "gmail thread get *"
      - "gmail get *"
      - "gmail labels list*"
      - "gmail history*"
      # Drafts (read-only)
      - "gmail drafts list*"
      # Attachments
      - "gmail attachment *"
      # Version / health check
      - "--version"

    argv_deny_patterns:
      # Block sending email
      - "gmail send *"
      # Block drafts that create content
      - "gmail drafts create *"
      - "gmail drafts update *"
      # Block settings modifications
      - "gmail settings *"
      # Block watch (requires separate architecture, see Phase 4)
      - "gmail watch *"
      # Block label modification
      - "gmail thread modify *"
      # Block all non-gmail commands (drive, calendar, etc.)
      - "drive *"
      - "calendar *"
      - "contacts *"
      - "sheets *"
      - "docs *"
      - "tasks *"
      - "slides *"
      - "forms *"
      - "chat *"
      - "classroom *"
      - "groups *"
      - "keep *"
      - "people *"
      - "apps-script *"
      # Block auth manipulation
      - "auth *"

    env_inject:
      GOG_ACCOUNT: "you@gmail.com"
      GOG_ENABLE_COMMANDS: "gmail"       # gogcli's own command allowlist (defense in depth)
      GOG_JSON: "1"                      # Force JSON output (required for response filtering)
      GOG_COLOR: "never"                 # No ANSI escapes in output
      GOG_NO_INPUT: "1"                  # Suppress interactive prompts
      GOG_KEYRING_BACKEND: "file"        # Use encrypted file backend (no GUI keyring on server)
      GOG_KEYRING_PASSWORD: "your-secret" # Decrypt token store
      HOME: "/home/server-user"          # So gog finds ~/.config/gog/

    timeout_secs: 60  # Gmail API can be slow for large searches

    audit:
      enabled: true
      log_argv: true
      redact_patterns: []
```

### 1b. Server-side gogcli setup

Document the one-time setup on the trusted host:

```bash
# Install gogcli
brew install steipete/tap/gogcli  # or download binary

# Store OAuth credentials
gog auth credentials /path/to/client_secret.json

# Authorize (one-time, interactive)
gog auth add you@gmail.com --services gmail

# Verify
GOG_JSON=1 gog gmail labels list
```

### 1c. Known issue: gogcli no-stdout bug

[gogcli Issue #18506](https://github.com/openclaw/openclaw/issues/18506) reports zero stdout in non-interactive environments. Carapace spawns processes without a TTY. Mitigations to investigate:
- Set `TERM=dumb` in env_inject
- Set `GOG_NO_INPUT=1`
- If the bug persists, may need to wrap execution with `script -qc` or use a pty

### 1d. Changes required

**No code changes for Phase 1.** This uses existing Carapace features:
- `CliPolicy` with argv allow/deny patterns
- `env_inject` for credential isolation
- Existing audit logging

Deliverables:
- [ ] `examples/policies/gogcli-gmail.yaml` policy file
- [ ] `docs/gogcli-setup.md` setup guide for trusted host
- [ ] Integration test verifying `gog` argv policy matching

---

## Phase 2: Response Filter Framework (the generalizable hook)

**Goal**: Add a response interceptor pipeline to the dispatch flow. This is the architectural piece that enables content filtering for Gmail, Signal, 1Password, and future tools.

### 2a. Design: `ResponseFilter` trait

Add to `carapace-policy`:

```rust
/// A filter that can inspect and transform responses after command execution.
/// Filters run in order; each receives the output of the previous filter.
/// Designed to be tool-agnostic -- the same trait works for CLI and HTTP responses.
#[async_trait]
pub trait ResponseFilter: Send + Sync {
    /// Filter a CLI response. Return the (possibly modified) response,
    /// or an error to block the response entirely.
    async fn filter_cli_response(
        &self,
        request: &CliRequest,
        response: CliResponse,
        context: &FilterContext,
    ) -> Result<CliResponse, PolicyError>;

    /// Filter an HTTP response. Same semantics as CLI.
    async fn filter_http_response(
        &self,
        request: &HttpRequest,
        response: HttpResponse,
        context: &FilterContext,
    ) -> Result<HttpResponse, PolicyError>;
}

/// Context passed to filters -- includes policy config, tool name, etc.
pub struct FilterContext {
    pub tool: String,
    pub filter_config: FilterConfig,
}
```

Key design decisions:
- **Trait-based, not config-only**: Filters are Rust traits, not just YAML patterns. This allows complex logic (JSON parsing, regex on nested fields) that glob patterns can't express.
- **Chain of responsibility**: Multiple filters can be composed. A `ContentDenyFilter` and a `RedactFilter` can both run on the same response.
- **Shared interface for CLI and HTTP**: Same trait handles both, since the content filtering logic (inspect JSON, check patterns) is the same regardless of transport.

### 2b. Policy config extension

Add `response_filters` to both `CliPolicy` and `HttpPolicy` in `config.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliPolicy {
    // ... existing fields ...

    /// Response filters applied after command execution, before returning to client.
    /// Filters run in order. If any filter returns an error, the response is blocked.
    #[serde(default)]
    pub response_filters: Vec<FilterConfig>,
}

/// Configuration for a single response filter.
/// The `filter_type` determines which ResponseFilter implementation is used.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "filter_type")]
pub enum FilterConfig {
    /// Deny responses containing specific content patterns.
    /// Inspects JSON fields in stdout and blocks or redacts matches.
    #[serde(rename = "content_deny")]
    ContentDeny(ContentDenyConfig),

    /// Redact specific fields from the response before returning to client.
    #[serde(rename = "field_redact")]
    FieldRedact(FieldRedactConfig),

    /// Limit response size (truncate stdout beyond a threshold).
    #[serde(rename = "max_output_size")]
    MaxOutputSize(MaxOutputSizeConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentDenyConfig {
    /// JSON field paths to inspect (dot-notation, with `[*]` for arrays).
    /// Examples: "subject", "messages[*].subject", "messages[*].body.text"
    pub fields: Vec<ContentFieldRule>,

    /// What to do when a deny pattern matches.
    /// - "block": Return an error instead of the response (default)
    /// - "redact": Replace the matching field value with "[REDACTED]"
    /// - "omit": Remove the matching array element entirely
    #[serde(default = "default_deny_action")]
    pub action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentFieldRule {
    /// JSON field path to inspect.
    pub field: String,

    /// Glob patterns that trigger denial. Case-insensitive matching.
    pub deny_patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldRedactConfig {
    /// Fields to unconditionally redact from responses.
    pub fields: Vec<String>,
    /// Replacement string (default: "[REDACTED]")
    #[serde(default = "default_redact_replacement")]
    pub replacement: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaxOutputSizeConfig {
    /// Maximum stdout size in bytes. Truncated with a warning if exceeded.
    pub max_bytes: usize,
}
```

### 2c. Dispatch integration

Modify `cli_dispatch.rs` to run filters after execution:

```rust
// In dispatch_cli(), after execute_command() and before returning:

let mut response = CliResponse {
    id: req.id.clone(),
    exit_code: output.status.code().unwrap_or(-1),
    stdout: String::from_utf8_lossy(&output.stdout).to_string(),
    stderr: String::from_utf8_lossy(&output.stderr).to_string(),
};

// Run response filters
for filter_config in &cli_policy.response_filters {
    let filter = build_filter(filter_config);
    let context = FilterContext {
        tool: req.tool.clone(),
        filter_config: filter_config.clone(),
    };
    response = filter.filter_cli_response(&req, response, &context).await?;
}

Ok(response)
```

Same pattern in `http_dispatch.rs` for HTTP responses. The `build_filter()` function maps `FilterConfig` variants to `ResponseFilter` trait implementations.

### 2d. Audit integration

When a filter blocks or modifies a response, log it:

```rust
// In the filter chain:
audit_logger.log_filter_action(
    request_id,
    tool,
    filter_type,    // "content_deny", "field_redact", etc.
    action_taken,   // "blocked", "redacted", "truncated"
    details,        // which field/pattern triggered it
);
```

### 2e. Changes required

Files to modify:
- `crates/carapace-policy/src/config.rs` -- Add `response_filters`, `FilterConfig`, `ContentDenyConfig`, etc.
- `crates/carapace-policy/src/lib.rs` -- Re-export new types
- `crates/carapace-policy/src/filter.rs` (new) -- `ResponseFilter` trait + built-in implementations
- `crates/carapace-server/src/cli_dispatch.rs` -- Run filter chain after execution (lines 87-92)
- `crates/carapace-server/src/http_dispatch.rs` -- Run filter chain after proxy response
- `crates/carapace-server/src/listener.rs` -- Pass filter audit events through
- `crates/carapace-server/src/audit.rs` -- Add `log_filter_action()`

Deliverables:
- [ ] `ResponseFilter` trait in `carapace-policy`
- [ ] `ContentDenyFilter` implementation (JSON field inspection + glob deny)
- [ ] `FieldRedactFilter` implementation
- [ ] `MaxOutputSizeFilter` implementation
- [ ] `FilterConfig` serde types in policy config
- [ ] Integration into `cli_dispatch.rs` and `http_dispatch.rs`
- [ ] Audit logging for filter actions
- [ ] Unit tests for each filter type
- [ ] Integration test: policy with `content_deny` filter blocks matching response

---

## Phase 3: Gmail Content Filters

**Goal**: Use the Phase 2 framework to implement Gmail-specific content policies.

### 3a. Password reset email filtering

Policy config:

```yaml
tools:
  gog:
    type: cli
    binary: /usr/local/bin/gog
    # ... argv patterns from Phase 1 ...

    response_filters:
      - filter_type: content_deny
        fields:
          - field: "messages[*].subject"
            deny_patterns:
              - "*password reset*"
              - "*reset your password*"
              - "*verification code*"
              - "*security code*"
              - "*one-time password*"
              - "*OTP*"
              - "*2FA*"
              - "*two-factor*"
              - "*confirm your email*"
              - "*verify your email*"
              - "*sign-in attempt*"
              - "*login attempt*"
          - field: "messages[*].snippet"
            deny_patterns:
              - "*reset your password*"
              - "*verification code*"
        action: omit  # Remove matching messages from the array

      - filter_type: field_redact
        fields:
          - "messages[*].body.attachments"  # Don't leak attachment contents
        replacement: "[ATTACHMENT_REDACTED]"

      - filter_type: max_output_size
        max_bytes: 1048576  # 1MB cap on gmail responses
```

### 3b. How gogcli JSON output maps to filter fields

`gog gmail search` with `GOG_JSON=1` returns structured JSON. The filter field paths correspond to the JSON structure:

```json
{
  "threads": [
    {
      "id": "...",
      "messages": [
        {
          "id": "...",
          "subject": "Reset your password",
          "from": "noreply@example.com",
          "snippet": "Click here to reset your password...",
          "body": {
            "text": "...",
            "html": "...",
            "attachments": [...]
          }
        }
      ]
    }
  ]
}
```

The `ContentDenyFilter` needs to:
1. Parse stdout as JSON
2. Navigate to `messages[*].subject` (iterate all messages across all threads)
3. Match each subject against deny patterns (case-insensitive glob)
4. Based on `action`, either block the whole response, redact the field, or omit the matching message

### 3c. Changes required

No structural code changes beyond Phase 2. This phase is about:
- [ ] Verifying gogcli JSON output format matches field paths
- [ ] Writing the example policy with Gmail-specific deny patterns
- [ ] Integration tests with sample gogcli JSON output
- [ ] Documentation of common Gmail filter patterns

---

## Phase 4: Generalizing for Signal and 1Password

The Phase 2 `ResponseFilter` framework works for any tool. Here's how it applies to the existing integrations:

### 4a. Signal (HTTP tool) -- filter incoming message content

```yaml
tools:
  signal-cli:
    type: http
    upstream: "http://127.0.0.1:18080"
    # ... existing jsonrpc config ...

    response_filters:
      - filter_type: content_deny
        fields:
          - field: "result.body"
            deny_patterns:
              - "*password reset*"
              - "*verification code*"
        action: redact
```

This filters *incoming* Signal messages fetched via the API. The same pattern that blocks password reset emails in Gmail blocks them in Signal responses.

### 4b. 1Password (CLI tool) -- prevent secret exfiltration

```yaml
tools:
  op:
    type: cli
    binary: /usr/bin/op
    # ... existing argv config ...

    response_filters:
      - filter_type: content_deny
        fields:
          - field: "value"
            deny_patterns:
              - "*BEGIN RSA PRIVATE KEY*"
              - "*BEGIN OPENSSH PRIVATE KEY*"
        action: block

      - filter_type: field_redact
        fields:
          - "fields[*].value"  # Redact actual secret values
        replacement: "[SECRET_REDACTED - use op directly]"
```

### 4c. Shared pattern library

Over time, common deny patterns will emerge across tools. Consider a shared patterns file:

```yaml
# patterns/security-sensitive.yaml
security_content_patterns:
  - "*password reset*"
  - "*reset your password*"
  - "*verification code*"
  - "*security code*"
  - "*one-time password*"
  - "*OTP*"
  - "*2FA*"
  - "*two-factor*"
```

This can be referenced from tool policies via `!include` or a policy-level `pattern_sets` config. Not needed for initial implementation but worth keeping the door open.

---

## Phase 5: Gmail Watch / Pub/Sub Integration

This is not a "future nice-to-have" -- it's how OpenClaw primarily uses Gmail. The
Gateway auto-starts `gog gmail watch serve` on boot when configured. OpenClaw has
dedicated source files (`gmail-watcher.ts`, `gmail-watcher-lifecycle.ts`, `gmail-ops.ts`,
`gmail-setup-utils.ts`) and auto-renews the 7-day watch expiry. Skipping this phase
means Carapace can't protect the most common Gmail flow.

### 5a. Webhook receiver in carapace-server

Add a lightweight HTTP endpoint to carapace-server that receives POST webhooks from
`gog gmail watch serve`. This runs alongside the existing TCP listener.

```rust
// New: carapace-server/src/webhook_receiver.rs
//
// Starts a small HTTP server (e.g., axum or hyper) on a configurable port.
// Receives POST /hooks/{tool} with JSON body.
// Validates hook token.
// Runs the payload through the same ResponseFilter chain.
// Pushes filtered events as SseEvent to connected agents.

pub struct WebhookReceiver {
    /// Port to listen on (e.g., 8790)
    listen_port: u16,
    /// Shared secret for validating incoming webhooks
    hook_token: String,
    /// Response filters from the tool's policy config
    filters: Vec<Box<dyn ResponseFilter>>,
    /// Channel to push SseEvents to connected agents
    sse_tx: mpsc::UnboundedSender<Message>,
}
```

### 5b. Configuration

Extend the policy config to support webhook-triggered tools:

```yaml
tools:
  gog-gmail-watch:
    type: webhook                        # NEW tool type
    hook_token: "your-shared-secret"
    listen_port: 8790                    # Where carapace-server listens for webhooks

    response_filters:                    # Same filter framework from Phase 2
      - filter_type: content_deny
        fields:
          - field: "messages[*].subject"
            deny_patterns:
              - "*password reset*"
              - "*verification code*"
        action: omit

    audit:
      enabled: true
      log_body: false                    # Email content is sensitive
```

Then `gog gmail watch serve` points at Carapace:

```bash
gog gmail watch serve \
  --hook-url http://127.0.0.1:8790/hooks/gog-gmail-watch \
  --hook-token "your-shared-secret" \
  --include-body \
  --max-bytes 20000
```

### 5c. Event flow through Carapace

1. `gog gmail watch serve` receives Pub/Sub notification, enriches it with Gmail History API
2. Posts enriched JSON to `http://127.0.0.1:8790/hooks/gog-gmail-watch`
3. Carapace validates the hook token
4. Carapace runs the payload through `response_filters` (content deny, field redact, etc.)
5. If the payload survives filtering, wraps it as an `SseEvent`:
   ```rust
   SseEvent {
       id: generate_event_id(),
       tool: "gog-gmail-watch".to_string(),
       event: "gmail_notification".to_string(),
       data: filtered_payload_json,
   }
   ```
6. Pushes SseEvent through existing `sse_event_tx` channel to connected agents
7. Agent receives SseEvent, forwards to OpenClaw's hooks endpoint

### 5d. Agent-side webhook forwarding

The agent needs a small addition: when it receives an SseEvent for a webhook tool, it
forwards the event data as an HTTP POST to a configured local endpoint (OpenClaw's
`http://127.0.0.1:18789/hooks/gmail`).

```yaml
# Agent config (on untrusted VM)
webhook_forwarding:
  gog-gmail-watch:
    target_url: "http://127.0.0.1:18789/hooks/gmail"
    target_token: "OPENCLAW_HOOK_TOKEN"
```

### 5e. Why not just point `gog watch serve` directly at OpenClaw?

If the trusted host runs `gog gmail watch serve --hook-url http://VM:18789/hooks/gmail`,
the notifications bypass Carapace entirely. No content filtering, no audit logging, and
the webhook token gets shared across the trust boundary. Running it through Carapace
means:
- Content filters block password reset emails before they reach the agent
- All notifications are audit logged
- The hook token stays on the trusted host
- Credentials (OAuth tokens) stay on the trusted host

### 5f. Fallback: Agent-side polling

For simpler deployments that don't need real-time push, the agent can poll via Phase 1's
CLI flow:

```bash
# Cron-style polling every 5 minutes (uses existing Phase 1 infrastructure)
gog gmail search 'newer_than:5m' --max 20
```

Higher latency, higher API quota cost, but works without the webhook infrastructure.

### 5g. Changes required

New code:
- [ ] `carapace-server/src/webhook_receiver.rs` -- HTTP webhook listener
- [ ] `carapace-policy/src/config.rs` -- Add `WebhookPolicy` variant to `ToolPolicy` enum
- [ ] `carapace-agent/src/webhook_forwarder.rs` -- Forward SseEvents as webhooks
- [ ] `carapace-agent/src/config.rs` -- Add `webhook_forwarding` config

Modified code:
- [ ] `carapace-server/src/main.rs` -- Start webhook receiver alongside TCP listener
- [ ] `carapace-server/src/listener.rs` -- Accept SseEvents from webhook receiver

Deliverables:
- [ ] Webhook receiver with token validation
- [ ] Response filter integration for webhook payloads
- [ ] Agent-side webhook forwarding
- [ ] Integration test: webhook -> filter -> SseEvent -> agent -> forwarded POST
- [ ] Example config for Gmail Watch through Carapace

---

## Implementation Order

| Phase | Effort | Dependencies | Deliverables |
|-------|--------|--------------|--------------|
| 1     | Small  | None         | Policy file, setup docs, integration test |
| 2     | Medium | None         | ResponseFilter trait, 3 built-in filters, dispatch integration |
| 3     | Small  | Phase 2      | Gmail-specific filter config, tests with sample JSON |
| 4     | Small  | Phase 2      | Updated Signal/1Password example policies with filters |
| 5     | Medium | Phase 2      | Webhook receiver, agent forwarding, Gmail Watch pipeline |

Phase 1 and Phase 2 can be developed in parallel -- Phase 1 is pure policy config, Phase 2 is the code change. Phase 5 should follow Phase 2 closely, since the Pub/Sub pipeline is the primary way OpenClaw uses Gmail (not ad-hoc CLI queries).

## Key Design Decisions

### Why a trait, not just YAML config?

YAML config handles the 80% case (glob patterns on JSON fields). But real-world content filtering will need:
- Regex patterns (not just globs)
- Nested JSON traversal with array iteration
- Custom logic (e.g., "block if subject matches AND sender is not in allowlist")
- Future: ML-based content classification

The trait provides the escape hatch. Built-in filters handle YAML-configured cases; custom filters can be added in Rust for complex logic.

### Why filter responses, not requests?

For Gmail, the dangerous data is in the *response* (email content), not the request (search query). A query like `gmail search 'is:unread'` is benign -- it's the returned emails that might contain password reset links. Pre-execution argv filtering can't see the response.

This is different from Signal's `send` method where the *request* contains the dangerous data (recipient). Both pre-execution (argv/param) and post-execution (response content) filtering are needed; they complement each other.

### Why `GOG_JSON=1` is mandatory

Response filtering requires structured output. Without JSON mode, gogcli returns human-formatted tables that are unreliable to parse. `env_inject` with `GOG_JSON=1` ensures the server always gets parseable JSON, regardless of what the client requested. This is non-negotiable for content filtering to work.

### Why case-insensitive deny patterns

Email subjects are mixed case. "Reset Your Password", "RESET YOUR PASSWORD", and "reset your password" should all match. The `ContentDenyFilter` should do case-insensitive glob matching (convert both pattern and value to lowercase before matching). This differs from `ArgvMatcher` which is case-sensitive.

## Open Questions

1. **gogcli JSON schema stability**: Is the JSON output format documented/stable, or could it change across versions? Need to pin a gogcli version or handle schema evolution.

2. **Performance of response filtering**: For large Gmail search results (hundreds of messages), parsing and inspecting every message's subject/body adds latency. Is streaming inspection needed, or is buffered JSON parsing acceptable?

3. **Filter bypass via encoding**: Could an attacker craft an email with encoded/obfuscated subject lines that bypass glob patterns? E.g., Unicode homoglyphs, base64-encoded subjects. May need normalization before matching.

4. **Attachment handling**: `gog gmail thread get --download` saves files to disk. Carapace currently returns stdout/stderr but doesn't intercept filesystem writes. Need to either block `--download` via argv deny, or add a post-execution file scanning filter.

5. **Webhook receiver and the `type: webhook` policy**: Adding a third `ToolPolicy` variant (alongside `cli` and `http`) is the cleanest design, but it touches the core policy enum. Alternative: model it as a special case of `type: http` with an `inbound: true` flag. The dedicated variant is clearer but has more blast radius.

6. **OpenClaw's Gmail Watch auth token in URL bug**: [Issue #11024](https://github.com/openclaw/openclaw/issues/11024) documents a CVSS 8.3 vulnerability where OpenClaw embeds the push endpoint auth token in the URL query string. Carapace's webhook receiver should validate the token via a header (`X-Hook-Token` or `Authorization`), not a query parameter. Worth noting in docs.

7. **Prompt injection via email content**: Malicious emails can contain text designed to manipulate the AI agent (e.g., "IGNORE ALL PREVIOUS INSTRUCTIONS"). This is orthogonal to content filtering (which blocks sensitive data, not adversarial prompts) but is a real attack vector. OpenClaw has `allowUnsafeExternalContent` (default: false) which wraps external content with safety guards. Carapace could add a `prompt_injection_guard` filter type that wraps email content in safety delimiters, but this is a deeper topic for later.

8. **Streaming pull alternative**: A community-developed approach uses Pub/Sub streaming pull via gRPC instead of push webhooks -- the client opens an outbound connection and Google sends messages through it. This eliminates the need for inbound ports (no Tailscale Funnel). Could be a simpler deployment model for Phase 5 but requires a custom gRPC client rather than using `gog gmail watch serve`.
