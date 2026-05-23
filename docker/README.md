# docker

Docker build and deployment assets for MemoryHub.

- `Dockerfile` — multi-stage build: compiles the release binary in a Rust
  builder, ships it in a `debian:bookworm-slim` runtime as a non-root user.
- `deploy.sh` — builds and tags the image (and can push to GHCR).

## Build

Run from anywhere in the repo (the script resolves the repo root itself):

```sh
docker/deploy.sh
```

This builds `ghcr.io/asymmetry/memoryhub:<git-sha>` and `:latest`. Override the
repo with `IMAGE=ghcr.io/owner/name docker/deploy.sh`. Pushing is wired up for
the future (`docker/deploy.sh --push`, after `docker login ghcr.io`).

The build needs BuildKit (default on modern Docker) for its cache mounts.

## Run

All state lives under `MEMORYHUB_HOME` (set to `/data` in the image); mount a
volume there to persist it. The server listens on `0.0.0.0:8000`. Provider API
keys are read from the environment (`DEEPSEEK_API_KEY` for chat, `OPENAI_API_KEY`
for embeddings, by default).

```sh
docker run --rm \
  -p 8000:8000 \
  -v memoryhub-data:/data \
  -e DEEPSEEK_API_KEY \
  -e OPENAI_API_KEY \
  ghcr.io/asymmetry/memoryhub:latest
```

CLI flags pass through after the image name, e.g. `--log-level memoryhub=debug`
or `--port 9000` (also map the new port with `-p`). Drop a `config.toml` into the
mounted volume to override defaults.
