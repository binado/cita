use crate::ProviderError;
use std::{fmt, str::FromStr};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Locator {
    Inspire(u64),
    Arxiv(String),
    Doi(String),
}

impl Locator {
    pub fn normalized(&self) -> String {
        match self {
            Self::Inspire(id) => format!("inspire:{id}"),
            Self::Arxiv(id) => format!("arxiv:{}", normalize_arxiv(id)),
            Self::Doi(doi) => format!("doi:{}", doi.to_ascii_lowercase()),
        }
    }
}

impl fmt::Display for Locator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.normalized())
    }
}

impl FromStr for Locator {
    type Err = ProviderError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if let Some(value) = strip_prefix_ci(input, "inspire:") {
            let id = value.parse::<u64>().map_err(|_| invalid(input))?;
            return (id > 0)
                .then_some(Self::Inspire(id))
                .ok_or_else(|| invalid(input));
        }
        if let Some(value) = strip_prefix_ci(input, "arxiv:") {
            return parse_arxiv(value)
                .map(Self::Arxiv)
                .ok_or_else(|| invalid(input));
        }
        if let Some(value) = strip_prefix_ci(input, "doi:") {
            return parse_doi(value)
                .map(Self::Doi)
                .ok_or_else(|| invalid(input));
        }
        parse_arxiv(input)
            .map(Self::Arxiv)
            .ok_or_else(|| invalid(input))
    }
}

fn invalid(input: &str) -> ProviderError {
    ProviderError::InvalidLocator(format!(
        "`{input}` (use an arXiv id or an explicit arxiv:, doi:, or inspire: locator)"
    ))
}

fn strip_prefix_ci<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .filter(|candidate| candidate.eq_ignore_ascii_case(prefix))
        .map(|_| &value[prefix.len()..])
}

fn parse_arxiv(value: &str) -> Option<String> {
    let normalized = normalize_arxiv(value);
    let modern = {
        let mut parts = normalized.split('.');
        matches!((parts.next(), parts.next(), parts.next()), (Some(a), Some(b), None)
            if a.len() == 4 && a.bytes().all(|c| c.is_ascii_digit())
            && (b.len() == 4 || b.len() == 5) && b.bytes().all(|c| c.is_ascii_digit()))
    };
    let legacy = normalized.split_once('/').is_some_and(|(archive, number)| {
        !archive.is_empty()
            && archive
                .bytes()
                .all(|c| c.is_ascii_alphabetic() || matches!(c, b'.' | b'-'))
            && number.len() == 7
            && number.bytes().all(|c| c.is_ascii_digit())
    });
    (modern || legacy).then_some(normalized)
}

/// Strip a trailing `v<digits>` version suffix from an arXiv id, preserving case.
///
/// Case is preserved so the result is still suitable for storage; use
/// [`normalize_arxiv`] when comparing ids for equality.
pub fn strip_arxiv_version(id: &str) -> &str {
    let id = id.trim();
    if let Some(index) = id.rfind('v')
        && !id[index + 1..].is_empty()
        && id[index + 1..].bytes().all(|c| c.is_ascii_digit())
    {
        return &id[..index];
    }
    id
}

pub(crate) fn normalize_arxiv(value: &str) -> String {
    strip_arxiv_version(value).to_ascii_lowercase()
}

fn parse_doi(value: &str) -> Option<String> {
    let value = value.trim();
    let valid = value.starts_with("10.")
        && value.split_once('/').is_some_and(|(registrant, suffix)| {
            registrant.len() > 3
                && registrant[3..].bytes().all(|c| c.is_ascii_digit())
                && !suffix.is_empty()
                && !value.bytes().any(|c| c.is_ascii_whitespace())
        });
    valid.then(|| value.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_and_normalizes_supported_locators() {
        assert_eq!(
            "1207.7214v3".parse(),
            Ok(Locator::Arxiv("1207.7214".into()))
        );
        assert_eq!(
            "hep-th/9901001".parse(),
            Ok(Locator::Arxiv("hep-th/9901001".into()))
        );
        assert_eq!(
            "DOI:10.1000/ABC".parse(),
            Ok(Locator::Doi("10.1000/abc".into()))
        );
        assert_eq!("inspire:42".parse(), Ok(Locator::Inspire(42)));
    }

    #[test]
    fn rejects_ambiguous_and_invalid_values() {
        for value in ["10.1000/x", "arxiv:nope", "inspire:0", ""] {
            assert!(value.parse::<Locator>().is_err(), "{value}");
        }
    }
}
