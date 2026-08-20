//! INSPIRE JSON metadata provider with authoritative BibTeX snapshots.
#![warn(missing_docs)]

mod client;
mod snapshot;
mod wire;

pub use client::{Client, ClientBuilder, Error, RetryEvent};
pub use snapshot::InspireSnapshot;
pub use wire::{
    ApiArxivEprint, ApiAuthor, ApiCollaboration, ApiDoi, ApiImprint, ApiLiteratureMetadata,
    ApiLiteratureRecord, ApiPublicationInfo, ApiTitle,
};
