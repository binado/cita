//! INSPIRE JSON metadata provider with authoritative BibTeX snapshots.

mod client;
mod snapshot;
mod wire;

pub use client::{Client, ClientBuilder, Error, RetryEvent};
pub use snapshot::{
    ArxivEprint, Author, Collaboration, Doi, InspireSnapshot, PublicationInfo, Title, UrlValue,
};
