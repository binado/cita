use thiserror::Error as ThisError;

/// Every failure this crate can produce.
///
/// Byte offsets are relative to the source that was handed to the scanner, and
/// are reported wherever the scanner knows them. Parser-internal types are
/// never exposed.
#[derive(Clone, Debug, Eq, PartialEq, ThisError)]
pub enum Error {
    /// An entry's structure is broken at a known offset.
    #[error("malformed BibTeX entry at byte {offset}: {reason}")]
    MalformedEntry {
        /// Byte offset where scanning stopped.
        offset: usize,
        /// What the scanner expected instead.
        reason: String,
    },
    /// `biblatex`'s raw parser rejected this entry, or the slice did not
    /// resolve to exactly one entry (for example an `@string`, `@preamble`,
    /// or `@comment` directive, which `biblatex` parses as something else).
    #[error("invalid BibTeX at byte {offset}: {message}")]
    InvalidGrammar {
        /// Byte offset `biblatex` reported the problem at.
        offset: usize,
        /// `biblatex`'s description of the problem.
        message: String,
    },
    /// One source contains the same citation key more than once.
    #[error("citation key conflict: `{key}` appears more than once")]
    DuplicateKey {
        /// The repeated key.
        key: String,
    },
    /// A standalone entry was required but the source holds a different count.
    #[error("expected exactly one standalone entry, found {found}")]
    NotStandalone {
        /// How many entries the scanner found.
        found: usize,
    },
    /// Local metadata projection found no usable title.
    #[error("entry `{key}` has no title")]
    MissingTitle {
        /// The entry's source key.
        key: String,
    },
    /// Semantic parsing of a local entry failed.
    #[error("could not read local metadata: {message}")]
    SemanticParse {
        /// The underlying parser's description of the problem.
        message: String,
    },
}
