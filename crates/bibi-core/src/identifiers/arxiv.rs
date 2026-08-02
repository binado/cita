use crate::error::Error;
use std::{fmt, str::FromStr};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
