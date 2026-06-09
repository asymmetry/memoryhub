#!/usr/bin/env bash

set -e -o pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
PLUGIN_DIR="$REPO_ROOT/plugins/claude-code"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"; kill "$SERVER_PID" 2>/dev/null || true' EXIT

ROOT_TOKEN="mh_roottoken"
AGENT_ID="00000000-0000-0000-0000-000000000001"

# Build the hook CLI binary; capture.py/recall.py invoke `memoryhub-mcp` from PATH.
cargo build --manifest-path "$REPO_ROOT/memoryhub-mcp/Cargo.toml"
export PATH="$REPO_ROOT/target/debug:$PATH"

# Server config: mock LLM + in-memory auth db.
mkdir -p "$TMP/mhdata"
cat > "$TMP/mhdata/config.toml" <<EOF
[llm]
provider = "mock"
embedding_provider = "mock"
api_key_env = "UNUSED"
embedding_api_key_env = "UNUSED"
model = "mock"
embedding_model = "mock"
embedding_dim = 4

[auth]
db_path = ":memory:"
EOF

MEMORYHUB_HOME="$TMP/mhdata" MEMORYHUB_ADMIN_TOKEN="$ROOT_TOKEN" \
  cargo run --manifest-path "$REPO_ROOT/memoryhub/Cargo.toml" \
  --features _test -- --host 127.0.0.1 --port 19876 --log-level error &
SERVER_PID=$!
for i in $(seq 1 30); do
  curl -sf http://127.0.0.1:19876/v1/health > /dev/null 2>&1 && break
  sleep 1
done

# Bootstrap a user + token.
curl -sf -X POST http://127.0.0.1:19876/v1/admin/users \
  -H "Authorization: Bearer $ROOT_TOKEN" -H "Content-Type: application/json" \
  -d '{"username":"testuser","role":"user"}' > /dev/null
USER_TOKEN=$(curl -sf -X POST http://127.0.0.1:19876/v1/admin/users/testuser/tokens \
  -H "Authorization: Bearer $ROOT_TOKEN" -H "Content-Type: application/json" \
  -d '{"name":"integration"}' | python3 -c "import sys, json; print(json.load(sys.stdin)['token'])")

# Hook environment: the binary reads url/token/agent_id from env.
export MEMORYHUB_URL="http://127.0.0.1:19876"
export MEMORYHUB_TOKEN="$USER_TOKEN"
export MEMORYHUB_AGENT_ID="$AGENT_ID"
export HOME="$TMP"

# Test memory files.
MEMORY_DIR="$TMP/.claude/projects/proj-hash/memory"
mkdir -p "$MEMORY_DIR"
echo "# Test Memory" > "$MEMORY_DIR/test.md"

# Drive the capture hook.
python3 "$PLUGIN_DIR/hooks/capture.py" <<EOF
{"tool_calls": [{"tool_name": "Write", "tool_input": {"file_path": "$MEMORY_DIR/test.md"}}]}
EOF

# Verify via the read API.
RESPONSE=$(curl -sf -X POST http://127.0.0.1:19876/v1/memories/read \
  -H "Authorization: Bearer $USER_TOKEN" -H "Content-Type: application/json" \
  -d "{\"agent_id\":\"$AGENT_ID\",\"project\":\"proj-hash\",\"filename\":\"memory/test.md\"}")
echo "$RESPONSE" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert 'Test Memory' in d['content'], f'unexpected response: {d}'
print('capture -> upload -> read: OK')
"

# Drive the recall hook. It must run and exit 0, emitting either nothing or a
# SessionStart additionalContext JSON.
sleep 3
RECALL_OUT=$(python3 "$PLUGIN_DIR/hooks/recall.py")
if [ -n "$RECALL_OUT" ]; then
  echo "$RECALL_OUT" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d['hookSpecificOutput']['hookEventName'] == 'SessionStart', d
print('recall -> additionalContext: OK')
"
else
  echo 'recall produced no summary yet (acceptable); adapter ran cleanly.'
fi

echo "Integration test passed."
