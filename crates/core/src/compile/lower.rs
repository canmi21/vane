//! Lower analyzed rules to `SymbolicFlowGraph`: group by listener, sort
//! by `(level desc, specificity desc, name asc)`, build decision tree,
//! flatten to indexed `Vec<Node>`, hash-cons predicates and stateless
//! middleware.
//!
//! See `spec/architecture/02-flow.md` § _lower_ and § _Hash-consing_.
