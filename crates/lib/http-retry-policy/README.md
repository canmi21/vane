# HTTP Retry Policy

A serializable retry policy for HTTP clients: `Backoff::{None | Fixed | Exponential { base, max, jitter }}`,
a method allow-list (defaulting to RFC 9110 idempotent verbs), and a
`BufferingPolicy` knob that names the trade-off between "retries are
safe" and "request body memory is unbounded".

Sits in the gap between `tower-retry` (no backoff, no method gating)
and `backon` (backoff but no HTTP-aware policy types). Includes a
tiny `parse_duration("100ms" / "5s" / "2m")` helper for JSON / TOML
config schemas that drive policy from operator-facing files.

## Example

```rust
use std::time::Duration;
use http::Method;
use http_retry_policy::{Backoff, BufferingPolicy, RetryPolicy};

let policy = RetryPolicy {
    max_attempts: 3,
    methods: RetryPolicy::idempotent_methods(),
    backoff: Backoff::Exponential {
        base: Duration::from_millis(100),
        max: Duration::from_secs(5),
        jitter: true,
    },
    buffering: BufferingPolicy::Opportunistic,
};

# fn classify_retryable(_: &()) -> bool { true }
# async fn send_once<E>() -> Result<(), E> { Ok(()) }
# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let method = Method::GET;
for attempt in 1..=policy.max_attempts {
    let pre_sleep = policy.backoff.delay_for_attempt(attempt);
    if !pre_sleep.is_zero() {
        tokio::time::sleep(pre_sleep).await;
    }
    match send_once::<Box<dyn std::error::Error>>().await {
        Ok(()) => return Ok(()),
        Err(e) if !classify_retryable(&()) || !policy.methods.contains(&method) => {
            return Err(e);
        }
        Err(_) if attempt == policy.max_attempts => break,
        Err(_) => continue,
    }
}
# Ok(())
# }
```

## Buffering

`BufferingPolicy` exists to make the "is this request body
replayable?" trade-off explicit:

- `Opportunistic` — retry only when the body is already buffered. A
  streaming request body collapses retry to a single attempt.
- `Force` — buffer the body up-front so retries are always safe.
  Predictable retry, deterministic memory cost.

The policy struct just carries the choice; how the caller fulfils it
(buffer eagerly, switch transports, etc.) is application logic.

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
