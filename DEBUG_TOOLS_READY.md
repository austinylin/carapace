# 🎯 Carapace Debug Toolkit - READY TO DEPLOY

Built and tested. Ready to use for diagnosing the Signal message flow issue.

## What's Built

### 1. **carapace-debug CLI Binary**
- Location: `target/release/carapace-debug`
- Size: ~15MB (includes all dependencies)
- Status: ✅ Compiled and ready

### 2. Five Debug Commands

| Command | Purpose | Use When |
|---------|---------|----------|
| `health` | Check server status | Server is up? |
| `connections` | Monitor active connections | How many clients connected? |
| **`sniff`** | Capture TCP messages | WHERE ARE THE MESSAGES? |
| `audit` | Query audit log | What got allowed/denied? |
| `policy` | Test policy decisions | Is the policy wrong? |

## For Your Signal Issue: THE KEY COMMAND

```bash
carapace-debug sniff
```

This will:
1. Connect to carapace-server (localhost:8765)
2. Listen for all messages flowing through
3. Decode and display each message in real-time
4. Show message type, tool, method, path, body (first 100 chars)

**What you should see when agent sends a message:**
```
[Message #1] HttpRequest
----------------------------------------------------------------------
Tool: signal-cli
Method: POST
Path: /api/v1/send
Body: {"jsonrpc":"2.0","method":"send"...
```

**If you don't see it:** Message isn't reaching the server. That's the problem.

## Deployment Steps (5 minutes)

### 1. Copy binary to host
```bash
scp target/release/carapace-debug austin@austin-ubuntu-desktop.orca-puffin.ts.net:/tmp/
ssh austin@austin-ubuntu-desktop.orca-puffin.ts.net "sudo mv /tmp/carapace-debug /usr/local/bin/ && sudo chmod +x /usr/local/bin/carapace-debug"
```

### 2. Test it works
```bash
ssh austin@austin-ubuntu-desktop.orca-puffin.ts.net "carapace-debug health"
# Expected: ✅ Confirms server is running
```

### 3. Start capturing messages
```bash
# Terminal 1 on host - start sniffing
ssh austin@austin-ubuntu-desktop.orca-puffin.ts.net
carapace-debug sniff --filter HttpRequest

# Terminal 2 on VM - send a Signal message
ssh claw@claw.orca-puffin.ts.net
# (Have user send via OpenClaw)

# Terminal 1 should show the message immediately
```

## Quick Reference

### Test if server is listening
```bash
carapace-debug health
```

### See all connections
```bash
carapace-debug connections
```

### Capture ALL messages
```bash
carapace-debug sniff
```

### Capture only HttpRequest
```bash
carapace-debug sniff --filter HttpRequest
```

### Query policy without running system
```bash
carapace-debug policy /etc/carapace/policy.yaml '{
  "tool": "signal-cli",
  "method": "send",
  "params": {"recipientNumber": "+12246192888"}
}'
```

### Check audit log for requests in last 5 minutes
```bash
carapace-debug audit --tool signal-cli --since 5m
```

## What This Solves

Previous workflow (broken):
- SSH in → tail logs → grep → ssh again → tail more → repeat
- **Problem:** Context burning, hard to see patterns, slow feedback loop

New workflow (efficient):
- `carapace-debug sniff` → see messages in real-time
- `carapace-debug policy test` → verify policy logic instantly
- `carapace-debug audit query` → structured search instead of grep
- **Result:** Fast diagnosis, no context overhead

## Next Steps

1. Deploy to host
2. Run `carapace-debug sniff`
3. Send a Signal message from OpenClaw on VM
4. Observe if message appears in sniff output
5. If message appears: problem is in server dispatch logic
6. If message doesn't appear: problem is in message transmission (protocol/framing)

This will immediately tell us where to focus.

---

**Built:** 2026-02-15
**All tests passing:** ✅
**Ready for deployment:** ✅
