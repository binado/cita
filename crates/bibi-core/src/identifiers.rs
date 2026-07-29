//! Normalized, operational identifiers.
//!
//! Identifiers are not description. A wrong title prints a poor listing; a
//! wrong identifier deduplicates against the wrong record, resolves a selector
//! to the wrong record, or fetches the wrong document. They are also immutable
//! in a way description is not: an arXiv identifier never changes, and a DOI may
//! be *added* on publication but never changes value.

use crate::error::Error;
use std::{fmt, str::FromStr};

/// A normalized DOI.
///
/// DOIs are case-insensitive by specification, so the canonical form is trimmed
/// and ASCII-lowercased and equality is exact over that form.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Doi(String);

impl Doi {
    /// Validate and normalize a DOI.
    pub fn new(value: impl AsRef<str>) -> Result<Self, Error> {
        let value = value.as_ref().trim();
        let valid = value.starts_with("10.")
            && !value.chars().any(char::is_whitespace)
            && value.split_once('/').is_some_and(|(registrant, suffix)| {
                // The registrant code may be subdivided with periods, as in
                // `10.1000.1/abc`; each subdivision is a nonempty digit run.
                registrant.len() > 3
                    && registrant[3..].split('.').all(|part| {
                        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
                    })
                    && !suffix.is_empty()
            });
        if valid {
            Ok(Self(value.to_ascii_lowercase()))
        } else {
            Err(Error::InvalidDoi {
                value: value.to_owned(),
            })
        }
    }

    /// Borrow the normalized value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Doi {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for Doi {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

/// A normalized, versionless arXiv identifier.
///
/// Both the modern `2401.00001` and legacy `hep-th/9901001` forms are accepted.
/// A trailing version is stripped, because a record names a work rather than one
/// revision of it, and the value is lowercased so that equality is exact.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ArxivId(String);

impl ArxivId {
    /// Validate and normalize an arXiv identifier.
    pub fn new(value: impl AsRef<str>) -> Result<Self, Error> {
        let raw = value.as_ref();
        let normalized = strip_version(raw).to_ascii_lowercase();
        if is_modern(&normalized) || is_legacy(&normalized) {
            Ok(Self(normalized))
        } else {
            Err(Error::InvalidArxivId {
                value: raw.trim().to_owned(),
            })
        }
    }

    /// Borrow the normalized value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ArxivId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for ArxivId {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

/// Strip a trailing lowercase `v` followed by digits.
///
/// A bare trailing `v`, an uppercase `V`, or a non-digit tail is not a version
/// and is left alone.
fn strip_version(value: &str) -> &str {
    let value = value.trim();
    match value.rfind('v') {
        Some(at)
            if !value[at + 1..].is_empty()
                && value[at + 1..].bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            &value[..at]
        }
        _ => value,
    }
}

/// The `MM` of a `YYMM` or `YYMMNNN` field must name a real month.
fn valid_month(digits: &str) -> bool {
    digits[2..4]
        .parse::<u8>()
        .is_ok_and(|month| (1..=12).contains(&month))
}

/// `YYMM.NNNN` or `YYMM.NNNNN`.
fn is_modern(value: &str) -> bool {
    let mut parts = value.split('.');
    matches!((parts.next(), parts.next(), parts.next()), (Some(yymm), Some(number), None)
        if yymm.len() == 4
            && yymm.bytes().all(|byte| byte.is_ascii_digit())
            && valid_month(yymm)
            && (number.len() == 4 || number.len() == 5)
            && number.bytes().all(|byte| byte.is_ascii_digit()))
}

/// `archive/YYMMNNN`, optionally with a subject class such as `math.CO`.
fn is_legacy(value: &str) -> bool {
    value.split_once('/').is_some_and(|(archive, number)| {
        // At least one letter: `.` and `..` are legal in the grammar above but
        // would make an identifier read as a relative path, and an identifier is
        // joined into URLs and filenames.
        !archive.is_empty()
            && archive
                .bytes()
                .all(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'.' | b'-'))
            && archive.bytes().any(|byte| byte.is_ascii_alphabetic())
            && number.len() == 7
            && number.bytes().all(|byte| byte.is_ascii_digit())
            && valid_month(number)
    })
}

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

    #[test]
    fn dois_are_trimmed_and_case_folded() {
        assert_eq!(Doi::new("  10.1000/ABC  ").unwrap().as_str(), "10.1000/abc");
        assert_eq!(
            Doi::new("10.1016/j.physletb.2012.08.020").unwrap(),
            Doi::new("10.1016/J.PhysLetB.2012.08.020").unwrap()
        );
    }

    #[test]
    fn dois_reject_malformed_values() {
        for value in [
            "",
            "10/abc",
            "10./abc",
            "10.abc/def",
            "10.1000/",
            "10.1000",
            "doi:10.1000/abc",
            "10.1000/a b",
            "10.1000/ab\u{a0}c",
            "10.1000./abc",
            "10..1000/abc",
        ] {
            assert!(Doi::new(value).is_err(), "{value}");
        }
    }

    #[test]
    fn dois_accept_dotted_registrant_codes() {
        assert_eq!(Doi::new("10.1000.1/ABC").unwrap().as_str(), "10.1000.1/abc");
    }

    #[test]
    fn arxiv_ids_strip_versions_and_fold_case() {
        assert_eq!(ArxivId::new("1207.7214v3").unwrap().as_str(), "1207.7214");
        assert_eq!(
            ArxivId::new(" HEP-TH/9901001v12 ").unwrap().as_str(),
            "hep-th/9901001"
        );
        assert_eq!(ArxivId::new("2401.00001").unwrap().as_str(), "2401.00001");
        assert_eq!(
            ArxivId::new("math.CO/0309136").unwrap().as_str(),
            "math.co/0309136"
        );
    }

    #[test]
    fn arxiv_ids_reject_non_identifiers_and_partial_versions() {
        for value in [
            "",
            "nope",
            "1207.721",
            "1207.721456",
            "12007.7214",
            "hep-th/990100",
            // No month 00 or 13 exists.
            "9913.00001",
            "0000.0001",
            "hep-th/0013001",
            // `.` and `..` are path components, not archives.
            "./1234567",
            "../1234567",
        ] {
            assert!(ArxivId::new(value).is_err(), "{value}");
        }
        // A trailing `v` without digits is part of the id, not a version, so
        // stripping it would silently invent a different identifier.
        assert!(ArxivId::new("1207.7214v").is_err());
        assert!(ArxivId::new("1207.7214V2").is_err());
    }

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
