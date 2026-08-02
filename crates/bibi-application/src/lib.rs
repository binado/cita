//! Strict command use cases over complete bibliography values.
#![warn(missing_docs)]

mod add;
mod check;
mod error;
mod list;
mod records;
mod render;
mod services;
mod sync;
mod target;

/// Domain types needed by the command-line adapter.
pub mod domain {
    pub use bibi_core::{Bibliography, Locator, ProviderName, Record, RecordFilter, Source};
    pub use bibi_manifest::BibliographyStore;
}

pub use add::{AddRequest, ImportRequest, MutationReport, add, import};
pub use bibi_core::{Admission, AdmissionKind};
pub use check::{CheckOutcome, check};
pub use error::Error;
pub use list::{ListJsonRecord, ListRequest, list, to_json};
pub use records::{RemovedRecord, init, remove, show};
pub use render::{RenderOptions, render_bibliography, render_records};
pub use services::Services;
pub use sync::{SyncReport, SyncRequest, sync};
pub use target::{MANIFEST_NAME, TargetResolver};
