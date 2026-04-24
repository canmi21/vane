//! `L4ForwardFetch` — TCP via `tokio::io::copy_bidirectional`; UDP session
//! forwarder keyed by 5-tuple (UDP lands S2-11).
//!
//! See `spec/architecture/06-l4.md` § _`l4_forward`_. Feature: S1-18.
