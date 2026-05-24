# docker

Docker build and deployment assets for MemoryHub.

## Build

The working tree must be clean. Run from anywhere in the repo:

```sh
docker/deploy.sh
```

This builds `memoryhub:<version>` and `memoryhub:latest`, where `<version>` comes from `memoryhub/Cargo.toml`. Override the name with `IMAGE=name deploy.sh`.

The build needs BuildKit (default on modern Docker) for its cache mounts.
