//! Provider-neutral values every provider produces.

use crate::error::ProviderError;
use bibi_bibtex::BibtexEntry;
use bibi_core::{
    BibiId, Description, Identifiers, Provenance, ProviderId, ProviderOwned, Revision,
};

/// Everything a provider knows about one record.
///
/// The shape follows from the record's field groups: a provider produces
/// provenance, identifiers, description, and payload. It never produces
/// identity or naming — the application mints the bibi id, and the local key is
/// adopted from the payload's own texkey or supplied by the user.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderRecord {
    /// Which provider owns this record, and its handle and token.
    pub provenance: Provenance,
    /// Canonical normalized identifiers.
    pub identifiers: Identifiers,
    /// Advisory display data, mapped from the structured record.
    pub description: Description,
    /// Verbatim provider BibTeX.
    pub payload: BibtexEntry,
}

impl From<ProviderRecord> for ProviderOwned {
    fn from(record: ProviderRecord) -> Self {
        Self {
            provenance: record.provenance,
            identifiers: record.identifiers,
            payload: record.payload,
            description: record.description,
        }
    }
}

/// One record a refresh should examine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshRequest {
    /// Which record this is, so results can be matched back exactly.
    pub bibi_id: BibiId,
    /// The provider's handle for it.
    pub provider_id: ProviderId,
    /// The token stored at the last refresh, if any.
    pub stored_revision: Option<Revision>,
}

/// The narrowed half of a record: everything but the payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderMetadata {
    /// The provider's handle, as it reports it now.
    pub provider_id: ProviderId,
    /// The provider's current change token.
    pub revision: Option<Revision>,
    /// Canonical normalized identifiers.
    pub identifiers: Identifiers,
    /// Advisory display data.
    pub description: Description,
}

/// What resolving one locator produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Resolution {
    /// A complete mapped record.
    Found(Box<ProviderRecord>),
    /// The provider understands the locator and holds no such record.
    NotFound,
    /// The provider cannot resolve locators of that kind.
    UnsupportedLocator,
}

/// What examining one managed record produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefreshState {
    /// The record's current metadata.
    Metadata(Box<ProviderMetadata>),
    /// The provider no longer holds this record.
    ///
    /// A normal event, not an exception: identifiers change and records get
    /// merged or withdrawn. It is a per-record no-op with a warning, never an
    /// automatic re-binding, which would be a silent rebind inside a bulk
    /// operation.
    Missing,
    /// This provider refreshes nothing.
    Unrefreshable,
}

/// One record's refresh outcome, tied to the record that was asked about.
#[derive(Debug)]
pub struct RefreshItem {
    /// Which record this answers for.
    pub bibi_id: BibiId,
    /// The outcome, or why it could not be produced.
    pub result: Result<RefreshState, ProviderError>,
}

/// One record's BibTeX, or its absence from a batch.
///
/// `None` is absence, which is unambiguous and survivable: no entry claimed
/// that record, so it is reported missing and every other record in the batch
/// proceeds. Ambiguity inside a batch is a [`ProviderError`] over the whole
/// call instead, because then no pairing in the response can be trusted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayloadItem {
    /// Which record this answers for.
    pub provider_id: ProviderId,
    /// The entry, when one arrived.
    pub payload: Option<BibtexEntry>,
}
