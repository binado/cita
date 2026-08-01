//! Provider-neutral bibliography domain model.
#![warn(missing_docs)]

mod bibliography;
mod error;
mod filter;
mod identifiers;
mod locator;
mod record;
mod record_id;
mod source;

pub mod remote;

pub use bibliography::{Admission, AdmissionKind, Bibliography, CollisionPolicy, Replacement};
pub use error::Error;
pub use filter::RecordFilter;
pub use identifiers::{ArxivId, Doi};
pub use locator::Locator;
pub use record::{Description, Identifiers, Record, RecordState};
pub use record_id::RecordId;
pub use source::{ProviderId, ProviderName, Source};
