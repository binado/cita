//! Parsing what the user typed into something that names exactly one record.
//!
//! One type serves both jobs a user-supplied string used to do separately:
//! naming something for a provider to resolve (`add`) and naming something
//! already in the manifest (`remove`, `rename`, `fetch`, `show`). A citation
//! key's grammar can look identical to a DOI or a modern arXiv id, which used
//! to force a selector to keep every form a string could denote and try them
//! in a fixed priority order — silently unreachable for whichever record lost
//! the tie. The `k:` sigil removes the ambiguity structurally instead: a bare
//! string is never a citation key, so nothing needs to be tried twice.
//!
//! Deciding whether a provider *supports* a locator belongs to the provider
//! layer, which is why an unqualified value that matches no recognized shape
//! becomes [`Locator::Opaque`] rather than an error or a guess.

use crate::{
    error::Error,
    identifiers::{ArxivId, Doi},
    provenance::{Provider, ProviderId},
};
use bibi_bibtex::CitationKey;
use percent_encoding::percent_decode_str;
use std::{borrow::Cow, fmt, str::FromStr};
use url::Url;

/// Something that names exactly one record: a local citation key, a provider's
/// own identity, a provider-neutral identifier, or an opaque value deferred to
/// whichever provider is selected.
///
/// No provider may ever be named `k`: `k:1124337` would otherwise be ambiguous
/// between a citation key and a qualifier for a provider named `k`. The `k:`
/// rule is checked before every other rule, so this is the only place that
/// ever reasons about it.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Locator {
    /// An exact local citation key, given with a mandatory `k:` sigil.
    Key(CitationKey),
    /// A qualified provider identity: `<provider>:<id>`.
    ProviderIdentity(Provider, ProviderId),
    /// A normalized DOI. Provider-neutral.
    Doi(Doi),
    /// A normalized arXiv identifier. Provider-neutral.
    Arxiv(ArxivId),
    /// No recognized shape. Deferred to whichever provider is selected.
    Opaque(String),
}

impl Locator {
    /// Parse a locator. A thin wrapper so call sites need no turbofish.
    pub fn parse(input: &str) -> Result<Self, Error> {
        input.parse()
    }
}

impl fmt::Display for Locator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Key(key) => write!(formatter, "k:{key}"),
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
            return CitationKey::new(rest)
                .map(Self::Key)
                .map_err(|_| invalid(input));
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
        // A `<name>:<rest>` prefix naming an installed provider exactly
        // qualifies the rest as that provider's own id. A name outside the
        // installed set falls through instead of qualifying, so a texkey like
        // `Aad:2012tfa` is never truncated to id `2012tfa` under a provider
        // named `Aad`.
        // `//` after the colon is a URL authority, not a provider id: without
        // this guard `https://inspirehep.net/...` parses as a provider named
        // `https`, and a URL whose host bibi does not know must instead reach
        // the provider that owns that syntax.
        if let Some((prefix, rest)) = value.split_once(':')
            && !rest.trim().is_empty()
            && !rest.starts_with("//")
            && let Ok(provider) = Provider::from_str(prefix)
        {
            let id = ProviderId::new(rest.trim()).expect("checked non-empty above");
            return Ok(Self::ProviderIdentity(provider, id));
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

    fn parse(value: &str) -> Locator {
        value.parse().unwrap()
    }

    #[test]
    fn a_key_sigil_is_the_only_way_to_denote_a_citation_key() {
        assert_eq!(
            parse("k:Aad:2012tfa"),
            Locator::Key(CitationKey::new("Aad:2012tfa").unwrap())
        );
        // Case-insensitive, matching `arxiv:`/`doi:`.
        assert_eq!(
            parse("K:Aad:2012tfa"),
            Locator::Key(CitationKey::new("Aad:2012tfa").unwrap())
        );
    }

    #[test]
    fn a_string_shaped_like_an_identifier_is_never_ambiguous() {
        // Without the sigil, a key-shaped arXiv id is always read as an arXiv
        // id: the two can no longer both be true of one string.
        assert_eq!(
            parse("2401.00001"),
            Locator::Arxiv(ArxivId::new("2401.00001").unwrap())
        );
        assert_eq!(
            parse("k:2401.00001"),
            Locator::Key(CitationKey::new("2401.00001").unwrap())
        );
    }

    #[test]
    fn explicit_prefixes_normalize_their_identifiers() {
        assert_eq!(
            parse("arxiv:1207.7214v3"),
            Locator::Arxiv(ArxivId::new("1207.7214").unwrap())
        );
        assert_eq!(
            parse("DOI:10.1000/ABC"),
            Locator::Doi(Doi::new("10.1000/abc").unwrap())
        );
    }

    #[test]
    fn bare_identifiers_are_recognized_without_a_prefix() {
        assert_eq!(
            parse("hep-th/9901001"),
            Locator::Arxiv(ArxivId::new("hep-th/9901001").unwrap())
        );
        assert_eq!(
            parse("10.1016/j.physletb.2012.08.020"),
            Locator::Doi(Doi::new("10.1016/j.physletb.2012.08.020").unwrap())
        );
    }

    #[test]
    fn a_provider_qualifier_carries_the_rest_verbatim() {
        assert_eq!(
            parse("inspire:1124337"),
            Locator::ProviderIdentity(Provider::Inspire, ProviderId::new("1124337").unwrap())
        );
        // A name outside the installed set does not qualify: it falls through
        // whole, for the selected provider to interpret in its own syntax.
        assert_eq!(
            parse("ads:2024ApJ...900..1X"),
            Locator::Opaque("ads:2024ApJ...900..1X".into())
        );
    }

    #[test]
    fn a_bare_value_defers_to_the_selected_provider() {
        assert_eq!(parse("1124337"), Locator::Opaque("1124337".into()));
        // A provider's own URL is that provider's syntax, so it arrives intact.
        assert_eq!(
            parse("https://inspirehep.net/literature/1124337"),
            Locator::Opaque("https://inspirehep.net/literature/1124337".into())
        );
    }

    #[test]
    fn canonical_arxiv_and_doi_urls_resolve_to_identifiers() {
        for value in [
            "https://arxiv.org/abs/2506.14764v2",
            "https://arxiv.org/pdf/2506.14764v3.pdf",
            "https://arxiv.org/pdf/2506.14764?download=1#page=2",
        ] {
            assert_eq!(
                parse(value),
                Locator::Arxiv(ArxivId::new("2506.14764").unwrap()),
                "{value}"
            );
        }
        assert_eq!(
            parse("  https://arxiv.org/abs/hep-th%2F9901001/  "),
            Locator::Arxiv(ArxivId::new("hep-th/9901001").unwrap())
        );
        assert_eq!(
            parse("https://doi.org/10.1000%2FABC%2FDef?utm_source=x"),
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
                matches!(parse(value), Locator::Opaque(_)),
                "{value} was read as an identifier"
            );
        }
    }

    #[test]
    fn a_malformed_explicit_identifier_is_an_error_not_opaque() {
        // The prefix is a claim about the kind, so a bad value must not fall
        // through and become an opaque value.
        assert!("arxiv:nope".parse::<Locator>().is_err());
        assert!("doi:nope".parse::<Locator>().is_err());
        assert!("".parse::<Locator>().is_err());
        assert!("   ".parse::<Locator>().is_err());
    }

    #[test]
    fn a_malformed_key_is_an_error_not_opaque() {
        assert!("k:has spaces".parse::<Locator>().is_err());
        assert!("k:".parse::<Locator>().is_err());
    }

    #[test]
    fn display_round_trips_through_parse() {
        for value in [
            "k:Aad:2012tfa",
            "inspire:1124337",
            "doi:10.1000/abc",
            "arxiv:1207.7214",
            "some-opaque-value",
        ] {
            assert_eq!(parse(value).to_string(), value, "{value}");
        }
    }
}
