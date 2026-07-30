//! Record builders shared by this crate's unit tests.

use bibi_bibtex::{BibtexEntry, CitationKey};
use bibi_core::{
    BibiId, Description, Identifiers, Provenance, Provider, ProviderId, ProviderOwned, Record,
    Revision,
};

pub(crate) fn entry(source: &str) -> BibtexEntry {
    BibtexEntry::parse_one(source.to_owned()).unwrap()
}

/// A managed record whose payload and title are derived from its key.
pub(crate) fn record(key: &str, provider: Provider, provider_id: &str) -> Record {
    Record::new(
        BibiId::new(),
        CitationKey::new(key).unwrap(),
        ProviderOwned {
            provenance: Provenance::managed(
                provider,
                ProviderId::new(provider_id).unwrap(),
                Some(Revision::new("2026-01-01T00:00:00+00:00").unwrap()),
            ),
            identifiers: Identifiers::default(),
            payload: entry(&format!("@misc{{Provider:{provider_id},title={{{key}}}}}")),
            description: Description {
                title: key.to_owned(),
                ..Description::default()
            },
        },
    )
    .unwrap()
}
