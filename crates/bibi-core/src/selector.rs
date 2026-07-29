//! Resolving what the user typed to exactly one stored record.
//!
//! Selection is a correctness operation: a selector that binds the wrong record
//! corrupts data, which is why it is exact, ordered, and never fuzzy. Filtering
//! is the opposite kind of thing — a filter that misses returns a poor listing —
//! and the two never share a code path.

use crate::{
    error::Error,
    identifiers::{ArxivId, Doi},
    provider_name::{ProviderId, ProviderName},
};
use bibi_bibtex::CitationKey;
use std::fmt;

/// One way a selector string could denote a record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SelectorForm {
    /// An exact local citation key.
    Key(CitationKey),
    /// A qualified provider identity.
    ProviderIdentity(ProviderName, ProviderId),
    /// A normalized DOI.
    Doi(Doi),
    /// A normalized arXiv identifier.
    Arxiv(ArxivId),
}

/// A user-supplied record selector.
///
/// One string can be several things at once — `2401.00001` is a valid citation
/// key *and* a valid arXiv id — so a selector keeps every form it could take,
/// in the documented resolution order: exact local key, provider identity,
/// DOI, arXiv id. The first form that matches wins, which makes the outcome
/// depend on the manifest rather than on the order the user happened to type.
///
/// The bibi id is deliberately not a form. It is internal identity, not a
/// name the user is expected to know or type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Selector {
    input: String,
    forms: Vec<SelectorForm>,
}

impl Selector {
    /// Parse a selector, retaining every form it could denote.
    pub fn parse(input: &str) -> Result<Self, Error> {
        let value = input.trim();
        let mut forms = Vec::new();
        if let Ok(key) = CitationKey::new(value) {
            forms.push(SelectorForm::Key(key));
        }
        if let Some((prefix, rest)) = value.split_once(':')
            && let (Ok(provider), Ok(id)) =
                (ProviderName::new(prefix), ProviderId::new(rest.trim()))
        {
            forms.push(SelectorForm::ProviderIdentity(provider, id));
        }
        if let Ok(doi) = Doi::new(value) {
            forms.push(SelectorForm::Doi(doi));
        }
        if let Ok(arxiv) = ArxivId::new(value) {
            forms.push(SelectorForm::Arxiv(arxiv));
        }
        if forms.is_empty() {
            return Err(Error::InvalidSelector {
                value: input.to_owned(),
            });
        }
        Ok(Self {
            input: value.to_owned(),
            forms,
        })
    }

    /// The forms to try, in resolution order.
    pub fn forms(&self) -> &[SelectorForm] {
        &self.forms
    }

    /// The selector as the user wrote it, for diagnostics.
    pub fn as_str(&self) -> &str {
        &self.input
    }
}

impl fmt::Display for Selector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forms(input: &str) -> Vec<SelectorForm> {
        Selector::parse(input).unwrap().forms().to_vec()
    }

    #[test]
    fn a_plain_key_denotes_only_a_key() {
        assert_eq!(
            forms("Aad:2012tfa"),
            [
                SelectorForm::Key(CitationKey::new("Aad:2012tfa").unwrap()),
                // `Aad` is not a provider name, so no identity form is offered.
            ]
        );
    }

    #[test]
    fn an_arxiv_id_is_tried_as_a_key_first() {
        // Resolution order is what disambiguates: a record literally keyed
        // `2401.00001` wins over a record carrying that arXiv id.
        assert_eq!(
            forms("2401.00001"),
            [
                SelectorForm::Key(CitationKey::new("2401.00001").unwrap()),
                SelectorForm::Arxiv(ArxivId::new("2401.00001").unwrap()),
            ]
        );
    }

    #[test]
    fn a_qualified_provider_id_offers_the_identity_form() {
        assert_eq!(
            forms("inspire:1124337"),
            [
                SelectorForm::Key(CitationKey::new("inspire:1124337").unwrap()),
                SelectorForm::ProviderIdentity(
                    ProviderName::new("inspire").unwrap(),
                    ProviderId::new("1124337").unwrap()
                ),
            ]
        );
    }

    #[test]
    fn a_doi_offers_the_doi_form_but_is_not_a_key() {
        assert_eq!(
            forms("10.1000/abc"),
            [SelectorForm::Doi(Doi::new("10.1000/abc").unwrap())]
        );
    }

    #[test]
    fn a_legacy_arxiv_id_offers_the_arxiv_form() {
        assert_eq!(
            forms("hep-th/9901001"),
            [SelectorForm::Arxiv(ArxivId::new("hep-th/9901001").unwrap())]
        );
    }

    #[test]
    fn a_selector_that_denotes_nothing_is_rejected() {
        for value in ["", "   ", "what is this", "a/b"] {
            assert!(Selector::parse(value).is_err(), "{value}");
        }
    }
}
