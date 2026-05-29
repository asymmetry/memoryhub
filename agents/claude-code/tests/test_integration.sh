#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
PLUGIN_DIR="$REPO_ROOT/agents/claude-code"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"; kill "$SERVER_PID" 2>/dev/null || true' EXIT

# Config pointing at our test server, placed where Path.home() will find it
mkdir -p "$TMP/.claude"
cat > "$TMP/.claude/memoryhub.json" <<EOF
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

# Create a fake memory file under ~/.claude/projects/<hash>/memory so push-all finds it
MEMORY_DIR="$TMP/.claude/projects/proj-hash/memory"
mkdir -p "$MEMORY_DIR"
echo "# Test Memory" > "$MEMORY_DIR/test.md"

# Test push-all (walks the given project dir under $HOME/.claude/projects)
HOME="$TMP" python3 "$PLUGIN_DIR/memoryhub.py" push-all --project-dir "$TMP/.claude/projects/proj-hash"

# Verify via read API; filename is the path relative to ~/.claude/projects
RESPONSE=$(curl -sf -X POST http://127.0.0.1:19876/v1/memories/read \
  -H "Content-Type: application/json" \
  -d '{"username":"testuser","agent_id":"00000000-0000-0000-0000-000000000001","filename":"proj-hash/memory/test.md"}')

echo "$RESPONSE" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert 'Test Memory' in d['content'], f'unexpected response: {d}'
print('Integration test passed.')
"
