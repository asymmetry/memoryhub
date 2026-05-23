# syntax=docker/dockerfile:1

FROM rust:1.95-bookworm AS builder

WORKDIR /app

# The whole workspace is copied in (host target/ and VCS are excluded via
# .dockerignore). The cargo registry and target dir are kept in BuildKit cache
# mounts, so dependencies — including the bundled SQLite C build — are not
# recompiled unless they actually change. The binary is copied out of the
# cache-mounted target dir within the same step so it survives into the image.
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked -p memoryhub \
    && cp target/release/memoryhub /usr/local/bin/memoryhub

FROM debian:bookworm-slim AS runtime

# ca-certificates is required for outbound HTTPS to the LLM / embedding APIs.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --uid 10001 memoryhub \
    && mkdir -p /data \
    && chown memoryhub:memoryhub /data

COPY --from=builder /usr/local/bin/memoryhub /usr/local/bin/memoryhub

# All on-disk state (SQLite db, memory files, prompts) lives under MEMORYHUB_HOME.
ENV MEMORYHUB_HOME=/data
EXPOSE 8000
USER memoryhub
WORKDIR /data

ENTRYPOINT ["memoryhub"]
