//! The INSPIRE provider.
//!
//! Retrieval and mapping are separate halves. [`transport`] owns HTTP, pacing,
//! and retry and returns raw responses; [`mapping`] is a pure function from a
//! raw response to provider-neutral core values; [`provider`] composes them and
//! implements the generic contract.
//!
//! **The seam is the network boundary**, which is what makes mapping a pure
//! function of its input. Mapping is where every per-provider judgment call
//! lives — which year, which title, which identifier is canonical — and where a
//! provider changing its schema does its damage, so it is testable against
//! captured responses with no network and no flakiness. The two failure classes
//! separate along the same seam: retrieval fails in ways retrying may fix, and
//! mapping fails in ways it cannot.
//!
//! Both halves are public so diagnostics and fixture capture can call them
//! alone. Neither's types cross the generic provider contract: a caller has to
//! name this crate to reach them, so the coupling stays visible and contained.
#![warn(missing_docs)]

mod batching;
mod error;
mod wire;

pub mod join;
pub mod mapping;
pub mod provider;
pub mod rate_limit;
pub mod retry;
pub mod testing;
pub mod transport;

pub use batching::{MAX_BATCH_RECORDS, MAX_ENCODED_QUERY};
pub use join::DeclaredKeys;
pub use mapping::MappedRecord;
pub use provider::InspireProvider;
pub use rate_limit::{Clock, RateLimiter, SystemClock};
pub use retry::{RetryEvent, RetryObserver};
pub use transport::{RawBibtex, RawJson, Transport, TransportBuilder};
