# docker

Docker build and deployment assets for MemoryHub.

- `Dockerfile` — multi-stage build: compiles the release binary in a Rust builder, ships it in a `debian:bookworm-slim` runtime as a non-root user.
- `deploy.sh` — builds and tags the image locally. Pushing is handled by GitHub Actions on release tags only.

## Build

The working tree must be clean. Run from anywhere in the repo (the script resolves the repo root itself):

```sh
docker/deploy.sh
```

This builds `memoryhub:<version>` and `memoryhub:latest`, where `<version>` comes from `memoryhub/Cargo.toml`. Override the name with `IMAGE=myname docker/deploy.sh`.

The build needs BuildKit (default on modern Docker) for its cache mounts.
