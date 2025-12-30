# Task 1.2: Extract Flow Execution Engine

**Status:** Planned (Phase I)

**Description:** Eliminate duplication across transport/flow.rs, carrier/flow.rs, application/flow.rs (~600 lines duplicated)

## Current State

- 3 nearly identical flow.rs files
- Only difference: Plugin trait variant (Middleware vs. L7Middleware vs. Terminator vs. L7Terminator)

## Proposed Design

```rust
// stack/flow/engine.rs
pub trait FlowExecutor {
    type Context;
    type MiddlewareOutput;
    type TerminatorOutput;

    async fn execute_middleware(
        &self,
        plugin: &dyn Plugin,
        inputs: ResolvedInputs,
        context: &mut Self::Context,
    ) -> Result<Self::MiddlewareOutput>;

    async fn execute_terminator(
        &self,
        plugin: &dyn Plugin,
        inputs: ResolvedInputs,
        context: Self::Context,
    ) -> Result<Self::TerminatorOutput>;
}

pub async fn execute_flow<E: FlowExecutor>(
    step: &ProcessingStep,
    executor: &E,
    context: &mut E::Context,
    flow_path: String,
) -> Result<E::TerminatorOutput> {
    // Generic flow execution logic (template resolution, plugin lookup, recursion)
}
```

## Discussion Points

1. FlowExecutor trait 设计是否合理？
2. 是否需要支持 flow-level hooks（before/after plugin execution）？
3. 如何处理 L4/L4+/L7 的差异（ConnectionObject vs. Container）？

## Benefits

- Single source of truth for flow logic
- Bug fixes apply to all layers
- Easier to add flow-level features

## Complexity

Medium (trait design + refactoring)

## Estimated Time

3-4 days
