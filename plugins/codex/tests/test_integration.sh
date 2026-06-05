#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
PLUGIN_DIR="$REPO_ROOT/plugins/codex"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"; kill "$SERVER_PID" 2>/dev/null || true' EXIT

ROOT_TOKEN="mh_roottoken"
AGENT_ID="00000000-0000-0000-0000-000000000002"

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
  --features _test -- --host 127.0.0.1 --port 19877 --log-level error &
SERVER_PID=$!
for i in $(seq 1 30); do
  curl -sf http://127.0.0.1:19877/v1/health > /dev/null 2>&1 && break
  sleep 1
done

curl -sf -X POST http://127.0.0.1:19877/v1/admin/users \
  -H "Authorization: Bearer $ROOT_TOKEN" -H "Content-Type: application/json" \
  -d '{"username":"testuser","role":"user"}' > /dev/null
USER_TOKEN=$(curl -sf -X POST http://127.0.0.1:19877/v1/admin/users/testuser/tokens \
  -H "Authorization: Bearer $ROOT_TOKEN" -H "Content-Type: application/json" \
  -d '{"name":"integration"}' | python3 -c "import sys, json; print(json.load(sys.stdin)['token'])")

export MEMORYHUB_URL="http://127.0.0.1:19877"
export MEMORYHUB_TOKEN="$USER_TOKEN"
export MEMORYHUB_AGENT_ID="$AGENT_ID"
export CODEX_HOME="$TMP/.codex"

mkdir -p "$CODEX_HOME/memories/rollout_summaries"
echo "# Durable memory" > "$CODEX_HOME/memories/MEMORY.md"
echo "thread summary" > "$CODEX_HOME/memories/rollout_summaries/x.md"

# Drive the capture hook (it enumerates $CODEX_HOME/memories and uploads).
python3 "$PLUGIN_DIR/hooks/capture.py" < /dev/null

# Verify both files via the read API.
for FN in "MEMORY.md" "rollout_summaries/x.md"; do
  RESP=$(curl -sf -X POST http://127.0.0.1:19877/v1/memories/read \
    -H "Authorization: Bearer $USER_TOKEN" -H "Content-Type: application/json" \
    -d "{\"agent_id\":\"$AGENT_ID\",\"filename\":\"$FN\"}")
  echo "$RESP" | python3 -c "import sys, json; d=json.load(sys.stdin); assert d.get('content'), f'empty: {d}'; print('read ok: $FN')"
done

# Drive recall best-effort.
sleep 3
RECALL_OUT=$(python3 "$PLUGIN_DIR/hooks/recall.py")
if [ -n "$RECALL_OUT" ]; then
  echo "$RECALL_OUT" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d['hookSpecificOutput']['hookEventName']=='SessionStart', d; print('recall ok')"
else
  echo "recall produced no summary yet (acceptable)"
fi

echo "Integration test passed."
