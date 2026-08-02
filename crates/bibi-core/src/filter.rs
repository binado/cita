//! Advisory filtering over records.

use crate::{ProviderName, Record};

/// Conjunctive listing filters.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecordFilter {
    /// Keep managed records owned by this provider.
    pub provider: Option<ProviderName>,
    /// Keep local records.
    pub local: bool,
    /// Case-insensitive author/collaboration substring.
    pub author: Option<String>,
    /// Case-insensitive title substring.
    pub title: Option<String>,
    /// Exact year.
    pub year: Option<i32>,
}

impl RecordFilter {
    /// True when every supplied criterion matches.
    pub fn matches(&self, record: &Record) -> bool {
        let state = record.state();
        self.provider
            .is_none_or(|provider| state.source().provider() == Some(provider))
            && (!self.local || state.source().is_local())
            && self.author.as_ref().is_none_or(|needle| {
                state
                    .description()
                    .authors()
                    .iter()
                    .chain(state.description().collaborations())
                    .any(|value| contains_ignore_case(value, needle))
            })
            && self
                .title
                .as_ref()
                .is_none_or(|needle| contains_ignore_case(state.description().title(), needle))
            && self
                .year
                .is_none_or(|year| state.description().year() == Some(year))
    }

    /// True when no criterion is present.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}
