use percent_encoding::percent_decode_str;
use std::{borrow::Cow, fmt, str::FromStr};
use thiserror::Error as ThisError;
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq, ThisError)]
/// Error returned when a reference locator cannot be parsed.
pub enum Error {
    /// The supplied string is not a supported locator.
    #[error("invalid reference locator: {0}")]
    InvalidLocator(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
/// A normalized identifier accepted by a metadata provider.
pub enum Locator {
    /// A numeric INSPIRE literature record identifier.
    Inspire(u64),
    /// A versionless arXiv identifier.
    Arxiv(String),
    /// A normalized DOI.
    Doi(String),
}

impl Locator {
    /// Return the canonical, explicitly prefixed locator string.
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
    type Err = Error;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if let Some(value) = strip_prefix_ci(input, "inspire:") {
            return parse_inspire(value)
                .map(Self::Inspire)
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
        if let Some(locator) = parse_url(input) {
            return Ok(locator);
        }
        parse_arxiv(input)
            .map(Self::Arxiv)
            .ok_or_else(|| invalid(input))
    }
}

fn parse_url(value: &str) -> Option<Locator> {
    let url = Url::parse(value).ok()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    match url.host_str()? {
        "arxiv.org" => {
            let decoded = decoded_path(&url)?;
            let path = decoded.strip_suffix('/').unwrap_or(&decoded);
            let (id, pdf) = if let Some(id) = path.strip_prefix("/abs/") {
                (id, false)
            } else {
                (path.strip_prefix("/pdf/")?, true)
            };
            let id = if pdf {
                id.strip_suffix(".pdf").unwrap_or(id)
            } else {
                id
            };
            parse_arxiv(id).map(Locator::Arxiv)
        }
        "inspirehep.net" => {
            let decoded = decoded_path(&url)?;
            let path = decoded.strip_suffix('/').unwrap_or(&decoded);
            path.strip_prefix("/literature/")
                .and_then(parse_inspire)
                .map(Locator::Inspire)
        }
        "doi.org" => {
            let decoded = decoded_path(&url)?;
            let doi = decoded.strip_prefix('/')?;
            if doi.trim() != doi {
                return None;
            }
            parse_doi(doi).map(Locator::Doi)
        }
        _ => None,
    }
}

fn decoded_path(url: &Url) -> Option<Cow<'_, str>> {
    percent_decode_str(url.path()).decode_utf8().ok()
}

fn invalid(input: &str) -> Error {
    Error::InvalidLocator(format!(
        "`{input}` (use an arXiv id, an explicit arxiv:, doi:, or inspire: locator, or a canonical URL)"
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

fn parse_inspire(value: &str) -> Option<u64> {
    let id = value.parse::<u64>().ok()?;
    (id > 0).then_some(id)
}

/// Strip a trailing lowercase `v` + non-empty digit run from an arXiv id.
///
/// Leading and trailing whitespace is trimmed. Only a lowercase `v` followed by
/// at least one ASCII digit counts as a version (`v2`, `v12`); a bare trailing
/// `v`, an uppercase `V`, or a non-digit tail is left unchanged. Case of the id
/// body is preserved so the result is suitable for storage.
///
/// This does not validate arXiv id shape. For equality comparisons, ASCII-lowercase
/// the stripped result.
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

/// Normalize an arXiv id for identity comparison: strip a trailing version
/// and ASCII-lowercase the result.
pub fn normalize_arxiv(value: &str) -> String {
    strip_arxiv_version(value).to_ascii_lowercase()
}

/// Normalize a DOI for identity comparison: trim whitespace and
/// ASCII-lowercase (DOIs are case-insensitive by specification).
pub fn normalize_doi(doi: &str) -> String {
    doi.trim().to_ascii_lowercase()
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
    fn accepts_arxiv_urls_as_locators() {
        assert_eq!(
            "https://arxiv.org/abs/2506.14764v2".parse(),
            Ok(Locator::Arxiv("2506.14764".into()))
        );
    }

    #[test]
    fn accepts_arxiv_pdf_urls_as_locators() {
        assert_eq!(
            "https://arxiv.org/pdf/2506.14764v3.pdf".parse(),
            Ok(Locator::Arxiv("2506.14764".into()))
        );
    }

    #[test]
    fn accepts_canonical_arxiv_url_variants() {
        assert_eq!(
            "https://arxiv.org/pdf/2506.14764?download=1#page=2".parse(),
            Ok(Locator::Arxiv("2506.14764".into()))
        );
        assert_eq!(
            "  https://arxiv.org/abs/hep-th%2F9901001/  ".parse(),
            Ok(Locator::Arxiv("hep-th/9901001".into()))
        );
    }

    #[test]
    fn accepts_inspire_literature_urls_as_locators() {
        assert_eq!(
            "https://inspirehep.net/literature/1234567/?utm_source=bibi#metadata".parse(),
            Ok(Locator::Inspire(1_234_567))
        );
    }

    #[test]
    fn accepts_doi_urls_as_locators() {
        assert_eq!(
            "https://doi.org/10.1000%2FABC%2FDef%28ghi%29?utm_source=bibi#details".parse(),
            Ok(Locator::Doi("10.1000/abc/def(ghi)".into()))
        );
    }

    #[test]
    fn rejects_ambiguous_and_invalid_values() {
        for value in ["10.1000/x", "arxiv:nope", "inspire:0", ""] {
            assert!(value.parse::<Locator>().is_err(), "{value}");
        }
    }

    #[test]
    fn rejects_noncanonical_and_malformed_urls() {
        for value in [
            "http://arxiv.org/abs/2506.14764",
            "https://www.arxiv.org/abs/2506.14764",
            "https://arxiv.org.example/abs/2506.14764",
            "https://user@arxiv.org/abs/2506.14764",
            "https://arxiv.org:8443/abs/2506.14764",
            "https://arxiv.org/export/2506.14764",
            "https://arxiv.org/abs/2506.14764/extra",
            "https://arxiv.org/abs/2506.14764.pdf",
            "https://inspirehep.net/literature/0",
            "https://inspirehep.net/literature/nope",
            "https://inspirehep.net/literature/42/extra",
            "https://dx.doi.org/10.1000/example",
            "https://doi.org/not-a-doi",
            "https://doi.org/10.1000/example%20",
        ] {
            assert!(value.parse::<Locator>().is_err(), "{value}");
        }
    }

    #[test]
    fn strip_arxiv_version_preserves_case_and_normalize_lowercases() {
        assert_eq!(strip_arxiv_version("HEP-TH/9901001v1"), "HEP-TH/9901001");
        assert_eq!(normalize_arxiv("HEP-TH/9901001v1"), "hep-th/9901001");
        assert_eq!(strip_arxiv_version("  1207.7214v2  "), "1207.7214");
        assert_eq!(strip_arxiv_version("1207.7214v"), "1207.7214v");
        assert_eq!(strip_arxiv_version("1207.7214v2a"), "1207.7214v2a");
        assert_eq!(strip_arxiv_version("1207.7214"), "1207.7214");
        assert_eq!(strip_arxiv_version("1207.7214V2"), "1207.7214V2");
    }
}
