//! Scripted facade construction for application tests.

use crate::Providers;
use bibi_bibtex::BibtexEntry;
use bibi_core::{Description, Identifiers, ProviderId, ProviderName, RecordState, Source};
use std::sync::Arc;

pub use bibi_core::remote::testing::{FakeProvider, ProviderCall, verify_contract};

/// Construct a facade whose INSPIRE slot is scripted.
pub fn providers(remote: Arc<FakeProvider>) -> Providers {
    Providers::scripted(remote)
}

/// Construct a complete managed test state.
pub fn provider_record(provider_id: &str, texkey: &str, title: &str) -> RecordState {
    RecordState::new(
        Source::managed(
            ProviderName::Inspire,
            ProviderId::new(provider_id).expect("provider id"),
        ),
        Identifiers::default(),
        Description::new(title, Vec::new(), Vec::new(), None),
        BibtexEntry::parse_one(format!("@misc{{{texkey},title={{{title}}}}}"))
            .expect("test BibTeX"),
    )
    .expect("test state")
}
