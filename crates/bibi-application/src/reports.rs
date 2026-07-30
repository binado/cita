//! What a use case returns instead of printing.

use bibi_bibtex::CitationKey;

/// The result of an operation over many items.
///
/// **Skips are not failures.** A refused duplicate, or an entry skipped for a
/// key collision, leaves the requested end state already true, so it exits
/// zero: `bibi add <locator> && make` should proceed when the reference was
/// already there, which is what a build script wants. Only `failures` make the
/// process exit nonzero.
#[derive(Clone, Debug, Default)]
pub struct BatchReport<T> {
    /// Items that changed something.
    pub successes: Vec<T>,
    /// Items that were already in the requested state.
    pub skipped: Vec<SkippedItem>,
    /// Items that could not be completed.
    pub failures: Vec<ItemFailure>,
}

impl<T> BatchReport<T> {
    /// An empty report.
    pub fn new() -> Self {
        Self {
            successes: Vec::new(),
            skipped: Vec::new(),
            failures: Vec::new(),
        }
    }

    /// True when any item failed, which is what makes the process exit nonzero.
    pub fn has_failures(&self) -> bool {
        !self.failures.is_empty()
    }

    /// How many items the report accounts for.
    pub fn len(&self) -> usize {
        self.successes.len() + self.skipped.len() + self.failures.len()
    }

    /// True when nothing was attempted.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An item that was already in the requested state.
#[derive(Clone, Debug)]
pub struct SkippedItem {
    /// The item as the user named it.
    pub item: String,
    /// Why it was skipped.
    pub reason: SkipReason,
    /// The local citation key of the stored record when the command emits it.
    pub key: Option<CitationKey>,
}

/// Why an item was skipped.
#[derive(Clone, Debug)]
pub enum SkipReason {
    /// An existing record already represents this work.
    Duplicate {
        /// The local key it is stored under.
        existing: CitationKey,
    },
    /// The citation key belongs to a different record.
    KeyCollision {
        /// The key that is taken.
        existing: CitationKey,
    },
    /// An earlier selector in the same removal already deleted this record.
    AlreadyRemoved {
        /// The local key the record had.
        existing: CitationKey,
    },
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Duplicate { existing } => {
                write!(formatter, "already stored as `{existing}`")
            }
            Self::KeyCollision { existing } => write!(
                formatter,
                "citation key `{existing}` already belongs to another record"
            ),
            Self::AlreadyRemoved { existing } => {
                write!(
                    formatter,
                    "`{existing}` was already removed by an earlier selector"
                )
            }
        }
    }
}

/// An item that could not be completed.
#[derive(Clone, Debug)]
pub struct ItemFailure {
    /// The item as the user named it.
    pub item: String,
    /// What went wrong, phrased for a person.
    pub message: String,
}

impl ItemFailure {
    /// Build a failure for one item.
    pub fn new(item: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            item: item.into(),
            message: message.into(),
        }
    }
}
