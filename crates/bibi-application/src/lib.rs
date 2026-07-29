//! Command use cases over injected services.
//!
//! This is the only crate that coordinates provider results with manifest
//! candidate mutations. Every use case returns a typed report rather than
//! printing; the binary decides presentation.
//!
//! **Bulk operations are partial; writes are not.** A command resolves
//! everything it was asked to, reports each failure, and then commits the
//! successes in a single atomic write. Resolving fifty of two hundred entries
//! and then hitting a network error must not discard the forty-nine that
//! worked, and must not leave half a manifest behind either.
//!
//! **Offline paths stay offline.** Listing, showing, renaming, removing,
//! rendering, and checking never construct a network client or consult the
//! document cache.
#![warn(missing_docs)]

mod add;
mod error;
mod list;
mod records;
mod render;
mod reports;
mod services;
mod sync;
mod target;

/// The domain types a command-line adapter has to name.
///
/// The binary depends on this crate, the provider contract, and the concrete
/// providers it registers — not on the domain and persistence crates beneath
/// them. Re-exporting the handful of types an argument parser must construct
/// keeps that boundary honest: whatever is not here is not the CLI's business.
pub mod domain {
    pub use bibi_bibtex::CitationKey;
    pub use bibi_core::{ProviderName, Record, RecordFilter, Selector};
    pub use bibi_manifest::ManifestStore;
}

pub use add::{
    AddFileRequest, AddKind, AddReport, AddRequest, AddedRecord, InputSource, add_file,
    add_locators,
};
pub use error::Error;
pub use list::{ListJsonRecord, ListRequest, list, to_json, to_keys};
pub use records::{RemoveReport, RemovedRecord, init, remove, rename, show};
pub use render::{RenderOptions, render_manifest, render_records};
pub use reports::{BatchReport, ItemFailure, SkipReason, SkippedItem};
pub use services::Services;
pub use sync::{IdentifierAddition, RefreshedRecord, SyncReport, SyncRequest, sync};
pub use target::{
    GLOBAL_MANIFEST_ENV, MANIFEST_NAME, PlatformPaths, TargetResolver, TargetSelection, output,
};
