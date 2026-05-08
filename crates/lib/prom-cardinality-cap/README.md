# Prom Cardinality Cap

Track unique `(metric_name, label_set)` combinations per "tenant"
namespace and silently drop new ones once a cap is reached, emitting
a `tracing::warn!` exactly once on the first drop per namespace.

Whenever untrusted input contributes to metric labels — multi-tenant
gateways, per-plugin metric facades, anything where a misbehaving
caller can grow your Prometheus index forever — this is the guardrail
that gets ad-hoc reinvented as `HashSet + AtomicBool` in every project.
The label set is hashed in a sort-stable order, so callers do not have
to sort beforehand.

## Example

```rust
use std::sync::Arc;
use prom_cardinality_cap::CardinalityRegistry;

let registry = CardinalityRegistry::with_cap(1000);
let tenant: Arc<str> = Arc::from("plugin-a.wasm");

if registry.try_admit(&tenant, "requests_total", &[
    ("method".into(), "GET".into()),
    ("status".into(), "200".into()),
]) {
    // emit to your prometheus registry / metrics crate
} else {
    // dropped — over cap; warn-once already fired for this namespace
}
```

## Resetting

The registry holds state in a single `Arc<Mutex<HashMap<…>>>`; drop
the `CardinalityRegistry` to reset every namespace. There is no
per-namespace eviction by design — if the caller wants to reset on
plugin reload, the natural unit is "drop the old registry and build a
new one" (cheap; the cap is read once at construction).

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
