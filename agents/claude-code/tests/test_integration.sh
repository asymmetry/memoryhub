#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
PLUGIN_DIR="$REPO_ROOT/agents/claude-code"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"; kill "$SERVER_PID" 2>/dev/null || true' EXIT

ROOT_TOKEN="mh_roottoken"

mkdir -p "$TMP/.claude"

# Server config: mock LLM provider (no API keys) and an in-memory auth db
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

# Start the server (--features _test enables the mock LLM provider). The root admin token
# bootstraps user/token management.
MEMORYHUB_HOME="$TMP/mhdata" MEMORYHUB_ADMIN_TOKEN="$ROOT_TOKEN" \
  cargo run --manifest-path "$REPO_ROOT/memoryhub/Cargo.toml" \
  --features _test -- --host 127.0.0.1 --port 19876 --log-level error &
SERVER_PID=$!
# Wait for server to be ready (up to 30s)
for i in $(seq 1 30); do
  curl -sf http://127.0.0.1:19876/v1/health > /dev/null 2>&1 && break
  sleep 1
done

# Bootstrap: create a user and mint a token via the admin API using the root token.
curl -sf -X POST http://127.0.0.1:19876/v1/admin/users \
  -H "Authorization: Bearer $ROOT_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"username":"testuser","role":"user"}' > /dev/null

USER_TOKEN=$(curl -sf -X POST http://127.0.0.1:19876/v1/admin/users/testuser/tokens \
  -H "Authorization: Bearer $ROOT_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"name":"integration"}' | python3 -c "import sys, json; print(json.load(sys.stdin)['token'])")

# Plugin config pointing at our test server, with the minted token, placed where
# Path.home() will find it.
cat > "$TMP/.claude/memoryhub.json" <<EOF
{
  "url": "http://127.0.0.1:19876",
  "token": "$USER_TOKEN",
  "agent_id": "00000000-0000-0000-0000-000000000001"
}
EOF

# Create a fake memory file under ~/.claude/projects/<hash>/memory so push-all finds it
MEMORY_DIR="$TMP/.claude/projects/proj-hash/memory"
mkdir -p "$MEMORY_DIR"
echo "# Test Memory" > "$MEMORY_DIR/test.md"

# Test push-all (walks the given project dir under $HOME/.claude/projects)
HOME="$TMP" python3 "$PLUGIN_DIR/memoryhub.py" push-all --project-dir "$TMP/.claude/projects/proj-hash"

# Verify via read API; filename is the path relative to ~/.claude/projects
RESPONSE=$(curl -sf -X POST http://127.0.0.1:19876/v1/memories/read \
  -H "Authorization: Bearer $USER_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"agent_id":"00000000-0000-0000-0000-000000000001","filename":"proj-hash/memory/test.md"}')

echo "$RESPONSE" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert 'Test Memory' in d['content'], f'unexpected response: {d}'
print('Integration test passed.')
"
