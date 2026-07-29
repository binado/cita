//! Parsing what the user typed into something a provider can resolve.
//!
//! Parsing generic syntax belongs here. Deciding whether a provider *supports* a
//! locator belongs to the provider layer, which is why an unqualified value that
//! is neither an arXiv id nor a DOI becomes a bare provider id for the registry
//! to place, rather than an error or a guess.

use crate::{
    error::Error,
    identifiers::{ArxivId, Doi},
    provider_name::ProviderName,
};
use percent_encoding::percent_decode_str;
use std::{borrow::Cow, fmt, str::FromStr};
use url::Url;

/// Something a provider can be asked to resolve.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Locator {
    /// A normalized arXiv identifier. Provider-neutral.
    Arxiv(ArxivId),
    /// A normalized DOI. Provider-neutral.
    Doi(Doi),
    /// A provider's own record id, exactly as written.
    ProviderId(String),
}

impl fmt::Display for Locator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Arxiv(id) => write!(formatter, "arxiv:{id}"),
            Self::Doi(doi) => write!(formatter, "doi:{doi}"),
            Self::ProviderId(id) => formatter.write_str(id),
        }
    }
}

/// A locator, with the provider the user named for it, if any.
///
/// A qualifier and a `--provider` flag that disagree are a usage error caught
/// before any request, because a wrong provider permanently determines the
/// BibTeX and texkey a record is stored with.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QualifiedLocator {
    /// The provider named by a `<provider>:` qualifier.
    pub provider: Option<ProviderName>,
    /// What to resolve.
    pub locator: Locator,
}

impl QualifiedLocator {
    /// True when this locator names no provider and no identifier kind, so the
    /// registry must find the single provider that recognizes its syntax.
    pub fn needs_provider_recognition(&self) -> bool {
        self.provider.is_none() && matches!(self.locator, Locator::ProviderId(_))
    }
}

impl fmt::Display for QualifiedLocator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.provider {
            Some(provider) => write!(formatter, "{provider}:{}", self.locator),
            None => write!(formatter, "{}", self.locator),
        }
    }
}

impl FromStr for QualifiedLocator {
    type Err = Error;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let value = input.trim();
        if value.is_empty() {
            return Err(invalid(input));
        }
        if let Some(rest) = strip_prefix_ci(value, "arxiv:") {
            return Ok(Self {
                provider: None,
                locator: Locator::Arxiv(ArxivId::new(rest)?),
            });
        }
        if let Some(rest) = strip_prefix_ci(value, "doi:") {
            return Ok(Self {
                provider: None,
                locator: Locator::Doi(Doi::new(rest)?),
            });
        }
        if let Some(locator) = canonical_url(value) {
            return Ok(Self {
                provider: None,
                locator,
            });
        }
        // A `<name>:<rest>` prefix in the provider-name grammar qualifies the
        // rest as that provider's own id. Installed-provider validation is the
        // registry's, so an unknown name parses and fails later, by name.
        // `//` after the colon is a URL authority, not a provider id: without
        // this guard `https://inspirehep.net/...` parses as a provider named
        // `https`, and a URL whose host bibi does not know must instead reach
        // the provider that owns that syntax.
        if let Some((prefix, rest)) = value.split_once(':')
            && !rest.trim().is_empty()
            && !rest.starts_with("//")
            && let Ok(provider) = ProviderName::new(prefix)
        {
            return Ok(Self {
                provider: Some(provider),
                locator: Locator::ProviderId(rest.trim().to_owned()),
            });
        }
        if let Ok(arxiv) = ArxivId::new(value) {
            return Ok(Self {
                provider: None,
                locator: Locator::Arxiv(arxiv),
            });
        }
        if let Ok(doi) = Doi::new(value) {
            return Ok(Self {
                provider: None,
                locator: Locator::Doi(doi),
            });
        }
        Ok(Self {
            provider: None,
            locator: Locator::ProviderId(value.to_owned()),
        })
    }
}

/// Recognize the canonical URLs of the two provider-neutral identifier schemes.
///
/// A provider's own web address is that provider's syntax to recognize, not
/// core's: teaching core about `inspirehep.net` would make every future
/// provider's URL a change to this function.
fn canonical_url(value: &str) -> Option<Locator> {
    let url = Url::parse(value).ok()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let decoded = decoded_path(&url)?;
    let path = decoded.strip_suffix('/').unwrap_or(&decoded);
    match url.host_str()? {
        "arxiv.org" => {
            let id = match path.strip_prefix("/abs/") {
                Some(id) => id,
                None => path
                    .strip_prefix("/pdf/")
                    .map(|id| id.strip_suffix(".pdf").unwrap_or(id))?,
            };
            ArxivId::new(id).ok().map(Locator::Arxiv)
        }
        "doi.org" => Doi::new(path.strip_prefix('/')?).ok().map(Locator::Doi),
        _ => None,
    }
}

fn decoded_path(url: &Url) -> Option<Cow<'_, str>> {
    percent_decode_str(url.path()).decode_utf8().ok()
}

fn strip_prefix_ci<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .filter(|candidate| candidate.eq_ignore_ascii_case(prefix))
        .map(|_| value[prefix.len()..].trim())
}

fn invalid(value: &str) -> Error {
    Error::InvalidLocator {
        value: value.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(value: &str) -> QualifiedLocator {
        value.parse().unwrap()
    }

    #[test]
    fn explicit_prefixes_normalize_their_identifiers() {
        assert_eq!(
            parse("arxiv:1207.7214v3").locator,
            Locator::Arxiv(ArxivId::new("1207.7214").unwrap())
        );
        assert_eq!(
            parse("DOI:10.1000/ABC").locator,
            Locator::Doi(Doi::new("10.1000/abc").unwrap())
        );
        assert!(parse("arxiv:1207.7214").provider.is_none());
    }

    #[test]
    fn bare_identifiers_are_recognized_without_a_prefix() {
        assert_eq!(
            parse("hep-th/9901001").locator,
            Locator::Arxiv(ArxivId::new("hep-th/9901001").unwrap())
        );
        assert_eq!(
            parse("10.1016/j.physletb.2012.08.020").locator,
            Locator::Doi(Doi::new("10.1016/j.physletb.2012.08.020").unwrap())
        );
    }

    #[test]
    fn a_provider_qualifier_carries_the_rest_verbatim() {
        let qualified = parse("inspire:1124337");
        assert_eq!(
            qualified.provider,
            Some(ProviderName::new("inspire").unwrap())
        );
        assert_eq!(qualified.locator, Locator::ProviderId("1124337".into()));
        assert!(!qualified.needs_provider_recognition());
        // An uninstalled provider still parses; naming it is the registry's job.
        assert_eq!(
            parse("ads:2024ApJ...900..1X").provider,
            Some(ProviderName::new("ads").unwrap())
        );
    }

    #[test]
    fn an_unrecognized_bare_value_defers_to_the_registry() {
        let bare = parse("1124337");
        assert!(bare.provider.is_none());
        assert_eq!(bare.locator, Locator::ProviderId("1124337".into()));
        assert!(bare.needs_provider_recognition());
        // A provider's own URL is that provider's syntax, so it arrives intact.
        let url = parse("https://inspirehep.net/literature/1124337");
        assert!(url.needs_provider_recognition());
    }

    #[test]
    fn canonical_arxiv_and_doi_urls_resolve_to_identifiers() {
        for value in [
            "https://arxiv.org/abs/2506.14764v2",
            "https://arxiv.org/pdf/2506.14764v3.pdf",
            "https://arxiv.org/pdf/2506.14764?download=1#page=2",
        ] {
            assert_eq!(
                parse(value).locator,
                Locator::Arxiv(ArxivId::new("2506.14764").unwrap()),
                "{value}"
            );
        }
        assert_eq!(
            parse("  https://arxiv.org/abs/hep-th%2F9901001/  ").locator,
            Locator::Arxiv(ArxivId::new("hep-th/9901001").unwrap())
        );
        assert_eq!(
            parse("https://doi.org/10.1000%2FABC%2FDef?utm_source=x").locator,
            Locator::Doi(Doi::new("10.1000/abc/def").unwrap())
        );
    }

    #[test]
    fn non_canonical_urls_are_not_treated_as_identifiers() {
        for value in [
            "http://arxiv.org/abs/2506.14764",
            "https://www.arxiv.org/abs/2506.14764",
            "https://user@arxiv.org/abs/2506.14764",
            "https://arxiv.org:8443/abs/2506.14764",
            "https://arxiv.org/abs/2506.14764/extra",
            "https://dx.doi.org/10.1000/example",
        ] {
            assert!(
                matches!(parse(value).locator, Locator::ProviderId(_)),
                "{value} was read as an identifier"
            );
        }
    }

    #[test]
    fn a_malformed_explicit_identifier_is_an_error_not_a_provider_id() {
        // The prefix is a claim about the kind, so a bad value must not fall
        // through and become some provider's opaque id.
        assert!("arxiv:nope".parse::<QualifiedLocator>().is_err());
        assert!("doi:nope".parse::<QualifiedLocator>().is_err());
        assert!("".parse::<QualifiedLocator>().is_err());
        assert!("   ".parse::<QualifiedLocator>().is_err());
    }
}
