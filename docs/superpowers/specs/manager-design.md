# Manager Actor — Component Design

## Overview

The `Manager` actor is the top-level supervisor for ClawChorus. It owns the three sub-system actors (`LlmService`, `MemoryManager`, `HttpServer`), subscribes to their supervision events, logs lifecycle activity, and initiates a clean shutdown when any child terminates or panics. It runs no business logic.

This iteration is **log-only**: there is no restart logic. Any child death tears the whole process down. Operators restart the process.

## Architecture

```
        main.rs
           |
        Manager
        /  |  \
   Llm   Memory   Http
   Service Manager Server
```

`main.rs` spawns exactly one actor: the `Manager`. The `Manager` constructs and owns its three children — they are not spawned by `main` and passed in.

### Construction

`Manager::new(config, shutdown_tx)`:

1. Builds the LLM provider from `config.llm` and spawns `LlmService` → captures `Address<LlmService>` + `JoinHandle`.
2. Spawns `MemoryManager` with the LLM address → captures `Address<MemoryManager>` + `JoinHandle`.
3. Spawns `HttpServer` with the `MemoryManager` address and `config.server` → captures `Address<HttpServer>` + `JoinHandle`.
4. Stores the `shutdown_tx: oneshot::Sender<()>` for use during fault-driven shutdown.

The Manager exposes no message handlers beyond the three `Handler<SupervisionEvent<_>>` impls described below. It receives no external traffic.

### Supervisor wiring

In `post_start`, the Manager subscribes itself as supervisor for each child:

```rust
let recipient: Recipient<SupervisionEvent<LlmService>> = ctx.address().clone().into();
llm_addr.send(Supervisor::Set(recipient)).await?;
```

Same pattern for `MemoryManager` and `HttpServer`. Once subscribed, the Manager's mailbox receives `SupervisionEvent<X>` messages for each child.

## Supervision Events

Each of the three `Handler<SupervisionEvent<X>>` impls dispatches to a common helper, distinguished only by the static child-name string used in logs.

| Event variant            | Action                                                                                              |
| ------------------------ | --------------------------------------------------------------------------------------------------- |
| `Warn(_, err)`           | `warn!` with child name and error display. Continue.                                                |
| `State(_, state)`        | `debug!` with state. Continue.                                                                      |
| `Terminated(_, err)`     | `error!` with child name and optional error. Initiate shutdown.                                     |
| `Panicked(_, info)`      | `error!` with child name and panic info. Initiate shutdown.                                         |

### Initiate shutdown

A single `initiate_shutdown(&mut self, reason: &str)` method:

1. If `self.shutting_down` is `true`, log and return (idempotent).
2. Set `self.shutting_down = true`.
3. Send `Signal::Terminate` to each surviving child via `do_send` (fire-and-forget; failures are logged but do not block).
4. Take `self.shutdown_tx` (it is `Option<oneshot::Sender<()>>`); send `()`. Ignore send errors (`main` may have already exited via ctrl-c).
5. Stop the Manager itself by sending `Signal::Terminate` to `ctx.address()`.

`post_stop` joins each child's `JoinHandle` with a per-handle abort fallback (mirrors the existing `MemoryManager::post_stop` pattern). The order is reverse of startup: `HttpServer` → `MemoryManager` → `LlmService`.

## main.rs

```rust
let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
let manager = Manager::new(config, shutdown_tx)?;
let (_manager_addr, manager_handle) = manager.start("manager")?;

tokio::select! {
    _ = tokio::signal::ctrl_c() => info!("ctrl-c received"),
    _ = shutdown_rx => warn!("Manager initiated shutdown after child failure"),
}

// Manager's own post_stop drains children. Cap wait time so a stuck child
// can't block process exit indefinitely.
let _ = tokio::time::timeout(Duration::from_secs(5), manager_handle).await;
```

On ctrl-c, `main` returns from `select!` and drops `_manager_addr`; the runtime delivers `Signal::Terminate` to the Manager, which drains children in `post_stop`. On fault-driven shutdown, the Manager has already started teardown before signalling `shutdown_rx`.

## Error Type

`ManagerError` added to `src/error.rs` alongside `ConfigError`:

```rust
pub enum ManagerError {
    Llm(#[from] LlmError),
    Memory(#[from] MemoryError),
    Http(#[from] HttpServerError),
    Actor(String),
}
```

`Actor::Error = ManagerError`. `Manager::new` returns `Result<Self, ManagerError>` so child spawn failures abort startup.

## Module Layout

```
src/
  manager.rs           // Manager actor (~150 LOC)
  error.rs             // add ManagerError variant
  lib.rs               // add `pub mod manager;`
  main.rs              // simplified: only Manager + shutdown_rx
```

No new submodule directory — `manager.rs` stays a single file. Existing `MemoryManager` keeps its current name (the new actor is unambiguously `Manager` at the crate root).

## Testing

**Unit (in `src/manager.rs`):**

- `dispatch_event_shutdown_is_idempotent` — invoke the shutdown helper twice; assert `shutdown_tx` is taken only once and no panic.

**Integration (`tests/manager_integration.rs`):**

- `manager_starts_and_stops_cleanly` — construct with `MockProvider` (use a free port: `server.port = 0`), start, signal-terminate the Manager, assert `manager_handle.await` completes within a 5s timeout.
- `manager_initiates_shutdown_when_child_dies` — construct, send `Signal::Terminate` to one child via a `pub(crate)` test-only accessor on the Manager, assert `shutdown_rx` fires within a 2s timeout.

The third "end-to-end through HTTP" check is already covered by `tests/http_integration.rs` plus the existing `MemoryManager` tests; adding it at the Manager level would duplicate without raising confidence.

Out of scope: restart logic, exponential backoff, partial degraded modes.

## Out of Scope (Future Work)

- Restart-on-crash for any child
- Backoff or restart-attempt caps
- Selective shutdown (HTTP-only restart)
- Health endpoints reporting child status
- Graceful drain of in-flight requests before shutdown
