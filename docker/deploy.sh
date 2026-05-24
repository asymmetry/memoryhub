#!/usr/bin/env bash
#
# Build the MemoryHub Docker image locally.
#
# Tags built:  ${IMAGE}:<version>  and  ${IMAGE}:latest
#
# Usage:
#   docker/deploy.sh
#
# Environment variables:
#   IMAGE=...         override the image name (default: memoryhub)

set -euo pipefail

IMAGE="${IMAGE:-memoryhub}"

for arg in "$@"; do
  case "$arg" in
    -h|--help) sed -n '3,11p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $arg (try --help)" >&2; exit 2 ;;
  esac
done

ROOT="$(git rev-parse --show-toplevel)"

if ! git -C "$ROOT" diff --quiet HEAD 2>/dev/null; then
  echo "error: working tree is dirty — commit or stash changes before building" >&2
  exit 1
fi

TAG="$(grep -m1 '^version' "$ROOT/memoryhub/Cargo.toml" | sed 's/version = "\(.*\)"/\1/')"

echo "Building ${IMAGE}:${TAG} and ${IMAGE}:latest ..."
DOCKER_BUILDKIT=1 docker build \
  -f "$ROOT/docker/Dockerfile" \
  -t "${IMAGE}:${TAG}" \
  -t "${IMAGE}:latest" \
  "$ROOT"

echo "Done: ${IMAGE}:${TAG}"
