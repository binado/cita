use crate::{ProviderName, RecordId};
use thiserror::Error as ThisError;

/// Domain and bibliography failures.
#[derive(Clone, Debug, Eq, PartialEq, ThisError)]
#[allow(missing_docs)]
pub enum Error {
    /// A record id is not a canonical UUID.
    #[error("invalid record id `{value}`; expected a canonical UUID")]
    InvalidRecordId { value: String },
    /// A provider name is not installed.
    #[error(
        "unknown provider `{value}`; installed providers: {}",
        ProviderName::installed_list()
    )]
    UnknownProvider { value: String },
    /// A provider handle is empty.
    #[error("a provider id must not be empty")]
    EmptyProviderId,
    /// A DOI is invalid.
    #[error("invalid DOI `{value}`; expected `10.<registrant>/<suffix>`")]
    InvalidDoi { value: String },
    /// An arXiv identifier is invalid.
    #[error("invalid arXiv identifier `{value}`; expected `2401.00001` or `hep-th/9901001`")]
    InvalidArxivId { value: String },
    /// A locator is empty or explicitly malformed.
    #[error("invalid locator `{value}`")]
    InvalidLocator { value: String },
    /// Complete state requires a display title.
    #[error("record `{texkey}` has no title")]
    MissingTitle { texkey: String },
    /// A stable identity already belongs to a record.
    #[error("record already exists as `{id}` (matched by {matched_by})")]
    ExistingRecord { id: RecordId, matched_by: String },
    /// Stable claims point to several resident records.
    #[error("record identities diverge across resident records: {ids}")]
    DivergentIdentity { ids: String },
    /// The final bibliography contains a repeated texkey.
    #[error("texkey `{texkey}` belongs to both `{first}` and `{second}`")]
    TexkeyInUse {
        texkey: String,
        first: RecordId,
        second: RecordId,
    },
    /// A final identity index is repeated.
    #[error("{kind} `{value}` belongs to both `{first}` and `{second}`")]
    DuplicateIdentity {
        kind: &'static str,
        value: String,
        first: RecordId,
        second: RecordId,
    },
    /// A record id is repeated.
    #[error("record id `{id}` appears more than once")]
    DuplicateRecordId { id: RecordId },
    /// A requested resident record does not exist.
    #[error("record `{id}` does not exist")]
    UnknownRecord { id: RecordId },
    /// One batch attempts several changes to one record.
    #[error("record `{id}` is targeted more than once in one mutation")]
    ConflictingChanges { id: RecordId },
    /// Two selectors in one removal resolve to one record.
    #[error("record `{id}` is selected more than once")]
    DuplicateRemoval { id: RecordId },
    /// An opaque locator cannot search a bibliography.
    #[error("`{value}` is meaningful only to a provider")]
    UnrecognizedLocator { value: String },
    /// A locator matched no resident record.
    #[error("no record matches `{locator}`")]
    NoMatch { locator: String },
}
