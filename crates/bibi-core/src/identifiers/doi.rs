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
}
