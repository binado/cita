//! SQLite-backed global bibliography storage.
#![warn(missing_docs)]

mod interchange;
mod library;
mod source;

pub use interchange::Interchange;
pub use library::{
    AddOutcome, ConflictPolicy, Library, LibraryError, ProjectedReference, ShelfEntry, ShelfName,
    SkippedReference, SyncCandidate, SyncUpdate, global_library_root,
};
pub use source::{HepIdentifiers, InspireEntry, KeyRequest, PendingReference, SourceSnapshot};

/// Name of the authoritative SQLite database.
pub const DATABASE_FILE: &str = "library.sqlite3";
/// Fixed default shelf name.
pub const DEFAULT_SHELF: &str = "main";
