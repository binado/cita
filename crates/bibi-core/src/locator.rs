//! Parsing one unambiguous record locator.

use crate::{ArxivId, Doi, Error, ProviderId, ProviderName};
use percent_encoding::percent_decode_str;
use std::{borrow::Cow, fmt, str::FromStr};
use url::Url;

/// A texkey, remote identity, portable identifier, or provider-private value.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Locator {
    /// An exact derived texkey, explicitly prefixed with `k:`.
    Texkey(String),
    /// A qualified provider identity.
    ProviderIdentity(ProviderName, ProviderId),
    /// A normalized DOI.
    Doi(Doi),
    /// A normalized arXiv identifier.
    Arxiv(ArxivId),
    /// Provider-private syntax.
    Opaque(String),
}

impl Locator {
    /// Parse a locator.
    pub fn parse(input: &str) -> Result<Self, Error> {
        input.parse()
    }
}

impl fmt::Display for Locator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Texkey(key) => write!(formatter, "k:{key}"),
            Self::ProviderIdentity(provider, id) => write!(formatter, "{provider}:{id}"),
            Self::Doi(doi) => write!(formatter, "doi:{doi}"),
            Self::Arxiv(id) => write!(formatter, "arxiv:{id}"),
            Self::Opaque(value) => formatter.write_str(value),
        }
    }
}

impl FromStr for Locator {
    type Err = Error;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let value = input.trim();
        if value.is_empty() {
            return Err(invalid(input));
        }
        if let Some(rest) = strip_prefix_ci(value, "k:") {
            if rest.is_empty() {
                return Err(invalid(input));
            }
            return Ok(Self::Texkey(rest.to_owned()));
        }
        if let Some(rest) = strip_prefix_ci(value, "arxiv:") {
            return Ok(Self::Arxiv(ArxivId::new(rest)?));
        }
        if let Some(rest) = strip_prefix_ci(value, "doi:") {
            return Ok(Self::Doi(Doi::new(rest)?));
        }
        if let Some(locator) = canonical_url(value) {
            return Ok(locator);
        }
        if let Some((prefix, rest)) = value.split_once(':')
            && !rest.trim().is_empty()
            && !rest.starts_with("//")
            && let Ok(provider) = ProviderName::from_str(prefix)
        {
            return Ok(Self::ProviderIdentity(
                provider,
                ProviderId::new(rest.trim()).expect("checked non-empty"),
            ));
        }
        if let Ok(arxiv) = ArxivId::new(value) {
            return Ok(Self::Arxiv(arxiv));
        }
        if let Ok(doi) = Doi::new(value) {
            return Ok(Self::Doi(doi));
        }
        Ok(Self::Opaque(value.to_owned()))
    }
}

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

    #[test]
    fn texkeys_are_explicit_and_accept_the_provider_grammar() {
        assert_eq!(
            Locator::parse("k:a/b").unwrap(),
            Locator::Texkey("a/b".into())
        );
        assert!(Locator::parse("k:").is_err());
        assert!(matches!(
            Locator::parse("2401.00001").unwrap(),
            Locator::Arxiv(_)
        ));
    }

    #[test]
    fn installed_provider_qualifiers_are_structural() {
        assert_eq!(
            Locator::parse("inspire:1124337").unwrap(),
            Locator::ProviderIdentity(ProviderName::Inspire, ProviderId::new("1124337").unwrap())
        );
        assert_eq!(
            Locator::parse("ads:whatever").unwrap(),
            Locator::Opaque("ads:whatever".into())
        );
    }
}
