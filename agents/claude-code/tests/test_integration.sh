#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
PLUGIN_DIR="$REPO_ROOT/agents/claude-code"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"; kill "$SERVER_PID" 2>/dev/null || true' EXIT

# Config pointing at our test server
cat > "$TMP/memoryhub-config.json" <<EOF
{
  "url": "http://127.0.0.1:19876",
  "username": "testuser",
  "agent_id": "00000000-0000-0000-0000-000000000001"
}
EOF

# Start the server
MEMORYHUB_HOME="$TMP/mhdata" cargo run --manifest-path "$REPO_ROOT/Cargo.toml" \
  -- --host 127.0.0.1 --port 19876 --log-level error &
SERVER_PID=$!
# Wait for server to be ready (up to 30s)
for i in $(seq 1 30); do
  curl -sf http://127.0.0.1:19876/v1/health > /dev/null 2>&1 && break
  sleep 1
done

# Create a fake memory file in a flat temp location
# push-all is given --memory-dir directly so get_filename is not involved
MEMORY_DIR="$TMP/memory"
mkdir -p "$MEMORY_DIR"
echo "# Test Memory" > "$MEMORY_DIR/test.md"

# Test push-all
MEMORYHUB_CONFIG_PATH="$TMP/memoryhub-config.json" \
  python3 "$PLUGIN_DIR/memoryhub.py" push-all --memory-dir "$MEMORY_DIR"

# Verify via read API
RESPONSE=$(curl -sf -X POST http://127.0.0.1:19876/v1/memories/read \
  -H "Content-Type: application/json" \
  -d '{"username":"testuser","agent_id":"00000000-0000-0000-0000-000000000001","filename":"test.md"}')

echo "$RESPONSE" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert 'Test Memory' in d['content'], f'unexpected response: {d}'
print('Integration test passed.')
"
