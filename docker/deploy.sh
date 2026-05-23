#!/usr/bin/env bash
#
# Build (and optionally push) the MemoryHub Docker image.
#
# Tags built:  ${IMAGE}:<git-short-sha>[-dirty]  and  ${IMAGE}:latest
#
# Usage:
#   docker/deploy.sh [--push]
#
#   --push            also push both tags (run `docker login ghcr.io` first)
#   IMAGE=...         override the image repo
#                     (default: ghcr.io/asymmetry/memoryhub)
set -euo pipefail

# Default to the project's GHCR repo so a future `--push` just works.
IMAGE="${IMAGE:-ghcr.io/asymmetry/memoryhub}"

PUSH=0
for arg in "$@"; do
  case "$arg" in
    --push) PUSH=1 ;;
    -h|--help) sed -n '3,12p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $arg (try --help)" >&2; exit 2 ;;
  esac
done

ROOT="$(git rev-parse --show-toplevel)"
SHA="$(git -C "$ROOT" rev-parse --short HEAD)"

# Mark images built from a dirty tree so they are never confused with a commit.
DIRTY=""
if ! git -C "$ROOT" diff --quiet HEAD 2>/dev/null; then
  DIRTY="-dirty"
fi
TAG="${SHA}${DIRTY}"

echo "Building ${IMAGE}:${TAG} and ${IMAGE}:latest ..."
DOCKER_BUILDKIT=1 docker build \
  -f "$ROOT/docker/Dockerfile" \
  -t "${IMAGE}:${TAG}" \
  -t "${IMAGE}:latest" \
  "$ROOT"

if [ "$PUSH" -eq 1 ]; then
  echo "Pushing ${IMAGE}:${TAG} and ${IMAGE}:latest ..."
  docker push "${IMAGE}:${TAG}"
  docker push "${IMAGE}:latest"
fi

echo "Done: ${IMAGE}:${TAG}"
