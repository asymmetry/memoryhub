# Manager Actor — Component Design

## Overview

The `Manager` actor is ClawChorus's top-level supervisor. It owns the three sub-system actors — `LlmService`, `MemoryManager`, and `HttpServer` — supervises them, and tears the whole system down cleanly when any of them fails. It runs no business logic and receives no external traffic.

This iteration is **log-only**: there is no restart logic. Any child death stops the process; an operator restarts it.

## Lifecycle

`Manager::new` just holds the config. When the Manager actor starts, it spawns the three children in dependency order — `LlmService`, then `MemoryManager` (needs the LLM), then `HttpServer` (needs the Memory Manager). Each child is spawned with the Manager already registered as its supervisor, so no child can fail before supervision is in place. If any child fails to spawn, the Manager fails to start.

## Supervision

The Manager handles supervision events from each child:

- **Warning** — logged; the child continues.
- **Terminated or panicked** — logged; the Manager stops itself, which triggers shutdown.
- Other lifecycle events are ignored.

## Shutdown

Shutdown has two triggers, both converging on the same teardown:

- **Operator (ctrl-c)** — `main` signals the Manager to stop.
- **Child failure** — the Manager stops itself on a terminated/panicked event.

Either way, the Manager terminates its three children in reverse startup order (`HttpServer` → `MemoryManager` → `LlmService`), waiting for each to stop. A child that already died is handled harmlessly. `main` waits for the Manager to finish, bounded by a timeout so a stuck child cannot block process exit.

## Error Type

`ManagerError` wraps the child sub-system errors (`LlmError`, `MemoryError`, `HttpServerError`) plus an actor-messaging variant. Child spawn failures surface through it and abort startup.

## Testing

Integration tests in `tests/manager.rs` cover the two paths:

- **Clean shutdown** — start the Manager, signal it to stop, expect it to stop within a timeout.
- **Fault shutdown** — make a child fail, expect the Manager to detect it and stop itself within a timeout.

Tests run against an in-memory mock LLM provider, gated behind a hidden Cargo feature so it never ships in release builds.

## Out of Scope (Future Work)

- Restart-on-crash, backoff, restart-attempt caps
- Selective shutdown (e.g. restarting only HTTP)
- Health reporting of child status
- Graceful drain of in-flight requests before shutdown
