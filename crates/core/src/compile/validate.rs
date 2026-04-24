//! IR-level validator: `NodeId` resolution, DAG check, phase-state-machine
//! walk, per-Fetch-kind edge presence.
//!
//! Feature-availability rejection (build was compiled without `h3`, etc.)
//! happens later, in `vane-engine::FlowGraph::link`.
//!
//! See `spec/architecture/02-flow.md` § _validate_.
