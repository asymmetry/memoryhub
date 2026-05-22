# Supervisor Actor — Component Design

## Overview

The `MemoryHub` actor (named after the crate) is the top-level supervisor. It owns the three sub-system actors — `LlmService`, `MemoryManager`, `HttpServer` — supervises them, and tears the system down cleanly when any fails. It runs no business logic and receives no external traffic.

This iteration is **log-only**: there is no restart logic. Any child death stops the process; an operator restarts it. Restart/backoff policies are deliberately deferred until the failure modes are better understood.

## Lifecycle

Children are spawned in dependency order — `LlmService`, then `MemoryManager` (needs the LLM), then `HttpServer` (needs the Memory Manager). Each is spawned with the `MemoryHub` actor already registered as its supervisor, so no child can fail before supervision is in place. If any child fails to spawn, `MemoryHub` fails to start.

## Supervision & Shutdown

Child supervision events: a warning is logged and the child continues; a terminated/panicked event is logged and `MemoryHub` stops itself. Stopping converges with the operator path (ctrl-c, signalled from `main`) on the same teardown: children are terminated in reverse startup order (`HttpServer` → `MemoryManager` → `LlmService`), each awaited. A child that already died is handled harmlessly. `main` waits for `MemoryHub` to finish, bounded by a timeout so a stuck child cannot block process exit.

Reverse-order teardown ensures the front door closes before its dependencies: `HttpServer` stops accepting requests before `MemoryManager` goes away, which stops before `LlmService`.

## Errors

`MemoryHubError` (in `src/error.rs`) wraps the three child errors — `Llm(LlmError)`, `Memory(MemoryError)`, `Http(HttpServerError)` — via `From`. Child spawn failures surface through these and abort startup.

## Out of Scope (Future Work)

- Restart-on-crash, backoff, restart-attempt caps
- Selective shutdown (e.g. restarting only HTTP)
- Health reporting of child status
- Graceful drain of in-flight requests before shutdown
