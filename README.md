# MemoryHub

[![Crates.io](https://img.shields.io/crates/v/memoryhub)](https://crates.io/crates/memoryhub)
[![docs.rs](https://img.shields.io/docsrs/memoryhub)](https://docs.rs/memoryhub)
[![CI](https://github.com/asymmetry/memoryhub/actions/workflows/ci.yml/badge.svg)](https://github.com/asymmetry/memoryhub/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/asymmetry/memoryhub/graph/badge.svg?token=MDSTGWUBTX)](https://codecov.io/gh/asymmetry/memoryhub)
[![License: MIT](https://img.shields.io/crates/l/memoryhub)](LICENSE)

A centralized memory service for AI agents.

Personal AI agents (Claude Code, etc.) each build up knowledge as local Markdown
files, in isolation. MemoryHub pools those per-user memories into one shared,
searchable knowledge base: it ingests raw Markdown from each agent, embeds it for
semantic search, and runs a synthesizer that produces per-user and cross-user
summaries.

## What it does

- **Collects memories** from many users and agents over a small JSON API.
- **Searchable instantly** — every memory is indexed the moment it's written, so
  it's immediately available. No batch or reprocess step.
- **Finds the right memory** — search handles both meaning-based and exact
  keyword queries, so results hold up whether you describe an idea or quote a term.
- **Synthesizes** — generates per-user and cross-user summaries from the pooled
  memories as they come in.
- **Human-readable storage** — memories stay as plain Markdown files you can read,
  edit, and version directly.
- **Bring your own models** — chat and embeddings are configured separately, so you
  can mix providers (and swap in a stub for tests).

It's a single self-contained service.

## Quick start

```bash
# Run the server. Pass a bootstrap admin token.
MEMORYHUB_ADMIN_TOKEN=mh_root_secret cargo run -p memoryhub

# Provision a user and mint a token. The bootstrap token is honored only until an
# admin user exists, so use it to create users and their tokens first.
curl -sX POST http://127.0.0.1:8000/v1/admin/users \
  -H 'Authorization: Bearer mh_root_secret' \
  -H 'Content-Type: application/json' \
  -d '{"username":"alice","role":"user"}'

curl -sX POST http://127.0.0.1:8000/v1/admin/users/alice/tokens \
  -H 'Authorization: Bearer mh_root_secret' \
  -H 'Content-Type: application/json' \
  -d '{"name":"laptop"}'
# => {"id":"...","token":"mh_..."}   the token is shown only once; copy it.

# Every /v1 route except /v1/health requires the token.
TOKEN=mh_...   # the token minted above

# Write a memory (agent_id is a UUID)
curl -sX POST http://127.0.0.1:8000/v1/memories/write \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"agent_id":"550e8400-e29b-41d4-a716-446655440000","filename":"role.md","content":"# Role\nBackend engineer"}'

# Search
curl -sX POST http://127.0.0.1:8000/v1/memories/search \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"query":"who works on the backend?"}'
```

Configuration is a `config.toml` under the data directory (chat/embedding
provider, models, chunking, paths). The data directory is chosen by, in order:
`--base-dir` > `$MEMORYHUB_HOME` > `~/.memoryhub`. The bootstrap admin token may
also be set as `admin_token` in the `[auth]` section instead of via
`MEMORYHUB_ADMIN_TOKEN`.

## License

This project is licensed under [MIT](LICENSE).
