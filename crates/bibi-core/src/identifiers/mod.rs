//! Normalized, operational identifiers.
//!
//! Identifiers are not description. A wrong title prints a poor listing; a
//! wrong identifier deduplicates against the wrong record, resolves a selector
//! to the wrong record, or fetches the wrong document. They are also immutable
//! in a way description is not: an arXiv identifier never changes, and a DOI may
//! be *added* on publication but never changes value.

mod arxiv;
mod doi;

pub use arxiv::ArxivId;
pub use doi::Doi;

/// How an identifier changed during a refresh.
///
/// The distinction is the whole reason identifiers are a group of their own. An
/// addition is the ordinary preprint-becomes-published event and is accepted; a
/// *replacement* contradicts the claim that these values never change once set,
/// so one of the stored value, the new value, or the provider is wrong, and none
/// of the three should be committed by a routine refresh.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdentifierChange<T> {
    /// The value is unchanged, including both sides being absent.
    Unchanged,
    /// A value appeared where there was none.
    Added(T),
    /// A different value replaced an existing one.
    Replaced {
        /// The stored value.
        old: T,
        /// The value the provider now reports.
        new: T,
    },
}

impl<T: Clone + PartialEq> IdentifierChange<T> {
    /// Classify a stored value against a freshly mapped one.
    ///
    /// A provider that stops reporting a value it previously reported is
    /// `Unchanged`: refresh never removes an identifier, since absence from one
    /// response is far more likely to be a narrowed field set or a transient
    /// gap than a retraction.
    pub fn classify(stored: Option<&T>, incoming: Option<&T>) -> Self {
        match (stored, incoming) {
            (_, None) => Self::Unchanged,
            (None, Some(new)) => Self::Added(new.clone()),
            (Some(old), Some(new)) if old == new => Self::Unchanged,
            (Some(old), Some(new)) => Self::Replaced {
                old: old.clone(),
                new: new.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doi::Doi;

    #[test]
    fn identifier_changes_distinguish_addition_from_replacement() {
        let old = Doi::new("10.1/a").unwrap();
        let new = Doi::new("10.1/b").unwrap();
        assert_eq!(
            IdentifierChange::classify(None, Some(&new)),
            IdentifierChange::Added(new.clone())
        );
        assert_eq!(
            IdentifierChange::classify(Some(&old), Some(&old)),
            IdentifierChange::Unchanged
        );
        assert_eq!(
            IdentifierChange::classify(Some(&old), Some(&new)),
            IdentifierChange::Replaced {
                old: old.clone(),
                new
            }
        );
        assert_eq!(
            IdentifierChange::classify(Some(&old), None),
            IdentifierChange::Unchanged
        );
        assert_eq!(
            IdentifierChange::<Doi>::classify(None, None),
            IdentifierChange::Unchanged
        );
    }
}
