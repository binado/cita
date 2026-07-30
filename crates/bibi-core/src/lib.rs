//! Provider-neutral domain values and pure record operations.
//!
//! `bibi-core` owns record identity, normalized identifiers, locator parsing,
//! selector resolution, and listing filters. It performs no I/O and models no
//! external API's wire shape.
//!
//! Every string-like value is a newtype whose constructor normalizes or
//! validates once, so that downstream code compares canonical forms rather than
//! re-deciding what counts as the same DOI. The six field groups of a
//! [`Record`] are named for the role they play, because the roles differ in
//! what a wrong value costs: an identifier reaches a deliverable, a description
//! reaches a listing.
#![warn(missing_docs)]

mod error;
mod filter;
mod id;
mod identifiers;
mod locator;
mod provider_name;
mod record;
mod selector;

pub mod provider;

pub use error::Error;
pub use filter::RecordFilter;
pub use id::BibiId;
pub use identifiers::{ArxivId, Doi, IdentifierChange};
pub use locator::{Locator, QualifiedLocator};
pub use provider_name::{ProviderId, ProviderName, Revision};
pub use record::{Description, Identifiers, Provenance, ProviderOwned, Record};
pub use selector::{Selector, SelectorForm};
