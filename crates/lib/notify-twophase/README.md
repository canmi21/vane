# Notify Two-Phase

A two-phase wrapper around [`notify-debouncer-full`](https://crates.io/crates/notify-debouncer-full) that closes the
"startup gap" between _server-bind_ and _fs-watcher-subscribe_.

If you bind a listener first and `notify::Watcher::watch` afterwards,
any file change that lands in the gap is **lost** — at least on macOS
FSEvents, which does not replay events for files already present at
subscription time. The fix is to subscribe **first**, queue events
into an unbounded channel during the bind window, and drain them once
the rest of the daemon is up. This crate does that for you.

## Two phases

1. **Phase 1 — `arm`** runs the synchronous `new_debouncer` +
   `debouncer.watch()` calls and returns a [`Subscription`]. From
   this point on, events are flowing into an unbounded mpsc.
2. **Phase 2 — your async loop** awaits [`Subscription::recv`]. Pair
   it with a `tokio_util::sync::CancellationToken` for graceful
   shutdown. The `Subscription` is dropped at the end of the loop;
   that stops the underlying watcher thread.

## Example

```rust,no_run
use std::time::Duration;
use notify_twophase::Subscription;
use tokio_util::sync::CancellationToken;

# async fn run(config_dir: std::path::PathBuf, cancel: CancellationToken) -> Result<(), Box<dyn std::error::Error>> {
let mut sub = notify_twophase::arm(config_dir.clone())?;
// ... bind listeners here; events that land in this window queue ...

tokio::spawn(async move {
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => return,
            evt = sub.recv() => {
                if evt.is_none() { return; }
                // reload your config, reconcile listeners, etc.
            }
        }
    }
});
# Ok(())
# }
```

## Default reload filter

`arm` uses a built-in batch predicate ([`is_reloadable_batch`]) that
fires on `Create` / `Modify(Data)` / `Modify(Name)` / `Remove` events
under the watched tree, and drops `Access` / `Modify(Metadata)` /
`Other` / `Any`. Use [`arm_with`] for a custom debounce window or a
custom predicate.

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
