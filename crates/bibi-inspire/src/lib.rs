//! The INSPIRE provider.
//!
//! Retrieval and mapping are separate halves: `transport` owns HTTP, pacing,
//! and retry and returns raw responses; `mapping` is a pure function from a raw
//! response to provider-neutral core values. `provider` composes them.
#![warn(missing_docs)]
