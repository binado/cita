//! INSPIRE JSON metadata provider with authoritative BibTeX snapshots.

mod client;
mod snapshot;
mod wire;

pub use client::{Client, ClientBuilder, Error, RetryEvent};
pub use snapshot::{InspireRecord, project_inspire};
