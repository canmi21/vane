# In-Flight Set

A shareable wrapper around `tokio::task::JoinSet` for daemons that
spawn one short-lived task per accepted connection / session and need
deterministic drain behaviour at shutdown.

`tokio::task::JoinSet` is the right primitive for the job but is not
`Sync`, so daemons typically wrap it in `Arc<std::sync::Mutex<...>>`
and re-implement the same three operations: spawn, soft drain (wait
for natural completion), forced drain (`abort_all` then await). This
crate is that wrapper, factored out so the spawn + drain choreography
is auditable in one place.

## Example

```rust,no_run
use std::sync::Arc;
use std::time::Duration;
use in_flight_set::InFlightSet;

# async fn run() {
let set: Arc<InFlightSet> = Arc::new(InFlightSet::new());

// Accept loop: spawn one task per connection.
set.spawn(async { handle_one().await });

// Shutdown: soft drain with a deadline, then escalate.
if tokio::time::timeout(Duration::from_secs(30), set.drain()).await.is_err() {
    set.drain_with_abort().await;
}
# }
# async fn handle_one() {}
```

## Operations

- `spawn(f)` — synchronous; pushes `f` into the underlying `JoinSet`
  under a brief sync-mutex critical section. The accept path never
  yields here.
- `drain()` — async; takes the `JoinSet` out under a brief sync
  critical section, releases the lock, then `join_next`-s every task
  off-lock so the mutex is never held across `.await`.
- `drain_with_abort()` — same as `drain` but calls `abort_all()` on
  the taken set before draining. Caller chooses when to escalate.
- `len()` / `is_empty()` — operator visibility into the set's
  current cardinality.

## Why not `tokio::sync::Mutex`?

`spawn` runs from the accept path which is hot and entirely
synchronous — `JoinSet::spawn` itself is sync. Wrapping it in
`tokio::sync::Mutex` would force every accept site through an
`async fn` for no reason. The sync `std::sync::Mutex` is held only
across the spawn (no await) and across the `mem::replace` that
takes the set out of the lock for drain.
