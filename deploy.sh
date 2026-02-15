#!/bin/bash
# Deployment script for Carapace signal-cli integration
#
# This script handles deployment of pre-built binaries from GitHub Actions.
# Requirements:
#  - gh (GitHub CLI) installed and configured
#  - SSH access to host and VM
#  - Latest commits pushed to GitHub (which triggers GitHub Actions build)
#
# Usage:
#   ./deploy.sh
#
# If GitHub Actions artifacts aren't available, the script will provide
# instructions for triggering a new build or building locally.

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
HOST="austin@austin-ubuntu-desktop.orca-puffin.ts.net"
VM="claw@claw.orca-puffin.ts.net"
WORK_DIR="/tmp/carapace-deploy-$$"

echo -e "${YELLOW}=== Carapace Deployment ===${NC}"
echo ""

# Get the latest successful Build run using JSON parsing
echo -e "${YELLOW}Finding latest successful build...${NC}"

LATEST=$(gh run list --limit 50 --json conclusion,name,databaseId,status -q '.[] | select(.conclusion=="success" and .name=="Build") | .databaseId' 2>/dev/null | head -1)

if [[ -z "$LATEST" ]]; then
    echo -e "${RED}ERROR: No successful 'Build' runs found${NC}"
    echo ""
    echo -e "${BLUE}To fix this:${NC}"
    echo "  1. Push a commit to trigger GitHub Actions:"
    echo -e "     ${YELLOW}git commit --allow-empty -m 'Trigger new build'${NC}"
    echo -e "     ${YELLOW}git push origin main${NC}"
    echo ""
    echo "  2. Wait for the Build workflow to complete (1-2 minutes)"
    echo ""
    echo "  3. Then run this script again:"
    echo -e "     ${YELLOW}./deploy.sh${NC}"
    echo ""
    echo -e "${BLUE}Monitor builds at:${NC}"
    echo "  https://github.com/austinylin/carapace/actions"
    echo ""
    echo "  Or check status with:"
    echo -e "    ${YELLOW}gh run list --limit 10${NC}"
    exit 1
fi

echo -e "${GREEN}✓ Latest successful build: #$LATEST${NC}"

# Try to download artifacts
echo -e "${YELLOW}Attempting to download artifacts from build #$LATEST...${NC}"
mkdir -p "$WORK_DIR"

# Download artifacts with detailed error output
DOWNLOAD_OUTPUT=$(gh run download "$LATEST" --dir "$WORK_DIR" 2>&1)
DOWNLOAD_EXIT=$?

if [[ $DOWNLOAD_EXIT -eq 0 ]] && find "$WORK_DIR" -name "*.tar.gz" -type f 2>/dev/null | grep -q "tar.gz"; then
    echo -e "${GREEN}✓ Artifacts downloaded${NC}"
else
    echo -e "${RED}ERROR: Failed to download artifacts from build #$LATEST${NC}"
    echo ""
    echo -e "${BLUE}Likely cause: Artifacts expired (GitHub retains artifacts for 90 days)${NC}"
    echo ""
    echo -e "${BLUE}To fix this:${NC}"
    echo "  1. Create a new build by pushing a commit:"
    echo -e "     ${YELLOW}git commit --allow-empty -m 'Trigger build for deployment'${NC}"
    echo -e "     ${YELLOW}git push origin main${NC}"
    echo ""
    echo "  2. Wait for the Build workflow to complete:"
    echo -e "     ${YELLOW}gh run list --limit 5${NC}"
    echo ""
    echo "  3. Once build #[new-number] shows 'success', run this script again:"
    echo -e "     ${YELLOW}./deploy.sh${NC}"
    echo ""
    echo -e "${BLUE}Debug info:${NC}"
    echo "  Build #$LATEST error:"
    echo "    $DOWNLOAD_OUTPUT"
    exit 1
fi

# Find and extract tar.gz
echo -e "${YELLOW}Extracting binaries...${NC}"
TAR_FILE=$(find "$WORK_DIR" -name "*.tar.gz" -type f 2>/dev/null | head -1)

if [[ -z "$TAR_FILE" ]]; then
    echo -e "${RED}ERROR: No .tar.gz file found in $WORK_DIR${NC}"
    echo "Contents:"
    find "$WORK_DIR" -type f 2>/dev/null | head -20
    echo ""
    echo "Subdirectories:"
    find "$WORK_DIR" -type d 2>/dev/null | head -20
    exit 1
fi

echo "  Found archive: $(basename "$TAR_FILE")"

# Extract to temp directory first
EXTRACT_TEMP="$WORK_DIR/extract-temp"
mkdir -p "$EXTRACT_TEMP"

cd "$EXTRACT_TEMP"
tar xzf "$TAR_FILE" 2>/dev/null || {
    echo -e "${RED}Failed to extract archive${NC}"
    exit 1
}
cd - > /dev/null

# Find binaries - they may be in a subdirectory created by tar
EXTRACT_DIR=$(find "$EXTRACT_TEMP" -type d -name "carapace-*" 2>/dev/null | head -1)
if [[ -z "$EXTRACT_DIR" ]]; then
    # If not found in expected dir, look anywhere in extract_temp
    EXTRACT_DIR=$(find "$EXTRACT_TEMP" -type f -name "carapace-server" -o -name "carapace-agent" | xargs dirname | sort | uniq | head -1)
fi

if [[ -z "$EXTRACT_DIR" ]]; then
    echo -e "${RED}ERROR: Could not find carapace directory in archive${NC}"
    echo ""
    echo "Archive contents:"
    find "$EXTRACT_TEMP" -type f 2>/dev/null | head -20
    echo ""
    echo "Directory structure:"
    find "$EXTRACT_TEMP" -type d 2>/dev/null | head -20
    exit 1
fi

if [[ ! -f "$EXTRACT_DIR/carapace-server" ]] || [[ ! -f "$EXTRACT_DIR/carapace-agent" ]]; then
    echo -e "${RED}ERROR: Binaries not found in $EXTRACT_DIR${NC}"
    echo ""
    echo "Directory contents:"
    ls -lah "$EXTRACT_DIR" 2>/dev/null || echo "  (directory doesn't exist)"
    exit 1
fi

CARAPACE_SERVER="$EXTRACT_DIR/carapace-server"
CARAPACE_AGENT="$EXTRACT_DIR/carapace-agent"

echo -e "${GREEN}✓ Binaries ready${NC}"
echo "  Server: $(ls -lh "$CARAPACE_SERVER" | awk '{print $5}')"
echo "  Agent:  $(ls -lh "$CARAPACE_AGENT" | awk '{print $5}')"

# Verify they're Linux binaries
echo -e "${YELLOW}Verifying binary formats...${NC}"
SERVER_TYPE=$(file "$CARAPACE_SERVER" | grep -o "ELF.*x86-64" || echo "UNKNOWN")
AGENT_TYPE=$(file "$CARAPACE_AGENT" | grep -o "ELF.*x86-64" || echo "UNKNOWN")

if [[ "$SERVER_TYPE" != *"ELF"* ]] || [[ "$AGENT_TYPE" != *"ELF"* ]]; then
    echo -e "${RED}ERROR: Binaries are not Linux ELF format${NC}"
    echo "  Server: $SERVER_TYPE"
    echo "  Agent: $AGENT_TYPE"
    exit 1
fi

echo -e "${GREEN}✓ Both are Linux ELF binaries${NC}"

# Stop services to avoid "text file busy" errors
echo -e "${YELLOW}Stopping services...${NC}"
ssh "$HOST" "sudo systemctl stop carapace-server.service" 2>&1 >/dev/null || true
ssh "$VM" "sudo systemctl stop carapace-agent.service" 2>&1 >/dev/null || true
sleep 2

# Deploy agent to VM
echo -e "${YELLOW}Deploying agent to VM...${NC}"
scp -q "$CARAPACE_AGENT" "$VM:/tmp/carapace-agent.new" || {
    echo -e "${RED}Failed to copy agent to VM${NC}"
    exit 1
}

ssh "$VM" "sudo cp /tmp/carapace-agent.new /usr/local/bin/carapace-agent && \
           sudo chmod 755 /usr/local/bin/carapace-agent && \
           rm /tmp/carapace-agent.new" 2>&1 >/dev/null || true

echo -e "${GREEN}✓ Agent deployed${NC}"

# Deploy server to host
echo -e "${YELLOW}Deploying server to host...${NC}"
scp -q "$CARAPACE_SERVER" "$HOST:/tmp/carapace-server.new" || {
    echo -e "${RED}Failed to copy server to host${NC}"
    exit 1
}

ssh "$HOST" "sudo cp /tmp/carapace-server.new /usr/local/bin/carapace-server && \
            sudo chmod 755 /usr/local/bin/carapace-server && \
            rm /tmp/carapace-server.new" 2>&1 >/dev/null || true

echo -e "${GREEN}✓ Server deployed${NC}"

# Clean up
rm -rf "$WORK_DIR" 2>/dev/null || true

# Restart services
echo -e "${YELLOW}Restarting services...${NC}"

echo -n "  Agent... "
ssh "$VM" "sudo systemctl start carapace-agent.service" 2>&1 >/dev/null || true
sleep 1
if ssh "$VM" "sudo systemctl is-active carapace-agent.service" 2>&1 | grep -q "active"; then
    echo -e "${GREEN}✓${NC}"
else
    echo -e "${RED}✗${NC}"
fi

echo -n "  Server... "
ssh "$HOST" "sudo systemctl start carapace-server.service" 2>&1 >/dev/null || true
sleep 1
if ssh "$HOST" "sudo systemctl is-active carapace-server.service" 2>&1 | grep -q "active"; then
    echo -e "${GREEN}✓${NC}"
else
    echo -e "${RED}✗${NC}"
fi

# Health checks
echo ""
echo -e "${YELLOW}Running health checks...${NC}"

echo -n "  /api/v1/check endpoint... "
if ssh "$VM" "timeout 3 curl -s http://127.0.0.1:8080/api/v1/check" 2>&1 | grep -q "OK"; then
    echo -e "${GREEN}✓${NC}"
else
    echo -e "${YELLOW}⚠${NC}"
fi

echo -n "  Server listening on 8765... "
if ssh "$HOST" "ss -tlnp 2>/dev/null | grep -q 8765" 2>&1; then
    echo -e "${GREEN}✓${NC}"
else
    echo -e "${YELLOW}⚠${NC}"
fi

echo -n "  Agent listening on 8080... "
if ssh "$VM" "ss -tlnp 2>/dev/null | grep -q 8080" 2>&1; then
    echo -e "${GREEN}✓${NC}"
else
    echo -e "${YELLOW}⚠${NC}"
fi

echo ""
echo -e "${GREEN}=== Deployment Complete ===${NC}"
echo ""
echo -e "${BLUE}Next steps:${NC}"
echo "  1. Test integration:"
echo -e "     ${YELLOW}ssh claw@claw.orca-puffin.ts.net 'curl -X POST http://127.0.0.1:8080/api/v1/rpc -H \"Content-Type: application/json\" -d \"{\\\"jsonrpc\\\":\\\"2.0\\\",\\\"id\\\":\\\"test\\\",\\\"method\\\":\\\"version\\\",\\\"params\\\":{}}\"'${NC}"
echo ""
echo "  2. View logs if needed:"
echo -e "     ${YELLOW}ssh austin@austin-ubuntu-desktop.orca-puffin.ts.net 'sudo journalctl -u carapace-server.service -f'${NC}"
echo -e "     ${YELLOW}ssh claw@claw.orca-puffin.ts.net 'sudo journalctl -u carapace-agent.service -f'${NC}"
