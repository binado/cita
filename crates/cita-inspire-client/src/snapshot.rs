use cita_bibliography::parse as parse_bibtex;
use cita_core::{
    Identifiers, ProjectionError, Publication, Reference, ReferenceSource, normalize_arxiv,
    normalize_doi,
};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspireSnapshot {
    pub record_id: u64,
    pub updated: String,
    pub texkeys: Vec<String>,
    pub bibtex: String,
    pub titles: Vec<Title>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<Author>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collaborations: Vec<Collaboration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub publication_info: Vec<PublicationInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arxiv_eprints: Vec<ArxivEprint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dois: Vec<Doi>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<UrlValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub document_types: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preprint_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub earliest_date: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Title {
    pub title: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Author {
    pub full_name: String,
    #[serde(
        default,
        deserialize_with = "one_or_many",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub role: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Collaboration {
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal_volume: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal_issue: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub curated_relation: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArxivEprint {
    pub value: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Doi {
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UrlValue {
    pub value: String,
}

fn is_false(value: &bool) -> bool {
    !*value
}

pub(crate) fn one_or_many<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Value {
        One(String),
        Many(Vec<String>),
    }
    Option::<Value>::deserialize(deserializer).map(|value| match value {
        None => Vec::new(),
        Some(Value::One(value)) => vec![value],
        Some(Value::Many(values)) => values,
    })
}

impl InspireSnapshot {
    pub fn validate(&self) -> Result<(), ProjectionError> {
        if self.record_id == 0 {
            return Err(ProjectionError::Invalid("INSPIRE record id is zero".into()));
        }
        let reference = self.project()?;
        if self.texkeys.is_empty() {
            return Err(ProjectionError::Invalid(
                "INSPIRE snapshot has no texkeys".into(),
            ));
        }
        let entries = parse_bibtex(&self.bibtex)
            .map_err(|error| ProjectionError::Invalid(error.to_string()))?;
        if entries.len() != 1 {
            return Err(ProjectionError::Invalid(format!(
                "INSPIRE snapshot has {} BibTeX entries",
                entries.len()
            )));
        }
        let (key, bibtex) = entries.iter().next().expect("length checked");
        if !self.texkeys.contains(key) {
            return Err(ProjectionError::Invalid(format!(
                "BibTeX key `{key}` is not one of the INSPIRE texkeys"
            )));
        }
        let bib_reference = bibtex.project()?;
        if !identifiers_match(&reference, &bib_reference) {
            return Err(ProjectionError::Invalid(
                "INSPIRE JSON and BibTeX do not identify the same record".into(),
            ));
        }
        Ok(())
    }
}

fn identifiers_match(json: &Reference, bib: &Reference) -> bool {
    let json_has_identifiers =
        !json.identifiers.dois.is_empty() || !json.identifiers.arxiv.is_empty();
    !json_has_identifiers
        || json
            .identifiers
            .dois
            .iter()
            .any(|id| bib.identifiers.dois.contains(id))
        || json
            .identifiers
            .arxiv
            .iter()
            .any(|id| bib.identifiers.arxiv.contains(id))
}

impl ReferenceSource for InspireSnapshot {
    fn project(&self) -> Result<Reference, ProjectionError> {
        let title = self
            .titles
            .first()
            .map(|item| item.title.trim().to_owned())
            .filter(|value| !value.is_empty())
            .ok_or(ProjectionError::MissingTitle)?;
        let publication_info = select_publication(self);
        let publication = publication_info.map(to_publication);
        let arxiv = unique(
            self.arxiv_eprints
                .iter()
                .map(|item| normalize_arxiv(&item.value)),
        );
        let dois = unique(self.dois.iter().map(|item| normalize_doi(&item.value)));
        let year = publication
            .as_ref()
            .and_then(|item| item.year)
            .or_else(|| date_year(self.preprint_date.as_deref()))
            .or_else(|| date_year(self.earliest_date.as_deref()))
            .or_else(|| arxiv.first().and_then(|id| arxiv_year(id)));
        let primary_category = self
            .arxiv_eprints
            .first()
            .and_then(|item| item.categories.first())
            .cloned();
        let mut providers = BTreeMap::new();
        providers.insert("inspire".into(), vec![self.record_id.to_string()]);
        Ok(Reference {
            title,
            authors: self
                .authors
                .iter()
                .filter(|author| {
                    author.role.is_empty()
                        || author
                            .role
                            .iter()
                            .any(|role| role.eq_ignore_ascii_case("author"))
                })
                .map(|author| author.full_name.clone())
                .collect(),
            collaborations: self
                .collaborations
                .iter()
                .map(|item| item.value.clone())
                .collect(),
            year,
            publication,
            url: self.urls.first().map(|url| url.value.clone()).or_else(|| {
                Some(format!(
                    "https://inspirehep.net/literature/{}",
                    self.record_id
                ))
            }),
            primary_category,
            identifiers: Identifiers {
                dois,
                arxiv,
                providers,
            },
        })
    }
}

fn unique(values: impl IntoIterator<Item = String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn select_publication(snapshot: &InspireSnapshot) -> Option<&PublicationInfo> {
    let visible = |item: &&PublicationInfo| !item.hidden && item.journal_title.is_some();
    snapshot
        .publication_info
        .iter()
        .filter(visible)
        .find(|item| item.curated_relation && (item.page_start.is_some() || item.artid.is_some()))
        .or_else(|| {
            snapshot
                .publication_info
                .iter()
                .filter(visible)
                .find(|item| {
                    item.year.is_some()
                        && (item.journal_volume.is_some()
                            || item.page_start.is_some()
                            || item.artid.is_some())
                })
        })
}

fn to_publication(item: &PublicationInfo) -> Publication {
    let pages = match (&item.page_start, &item.page_end, &item.artid) {
        (Some(start), Some(end), _) if start != end => Some(format!("{start}-{end}")),
        (Some(start), _, _) => Some(start.clone()),
        (_, _, Some(article)) => Some(article.clone()),
        _ => None,
    };
    Publication {
        journal: item.journal_title.clone(),
        volume: item.journal_volume.clone(),
        issue: item.journal_issue.clone(),
        pages,
        year: item.year,
    }
}

fn date_year(date: Option<&str>) -> Option<i32> {
    date?.get(..4)?.parse().ok()
}

fn arxiv_year(id: &str) -> Option<i32> {
    if id.as_bytes().get(4) == Some(&b'.') {
        return Some(2000 + id.get(..2)?.parse::<i32>().ok()?);
    }
    let year = id.split_once('/')?.1.get(..2)?.parse::<i32>().ok()?;
    Some(if year >= 91 { 1900 + year } else { 2000 + year })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(bibtex: &str, doi: &[&str], arxiv: &[&str]) -> InspireSnapshot {
        InspireSnapshot {
            record_id: 1,
            updated: "2026-01-01".into(),
            texkeys: vec!["Key".into()],
            bibtex: bibtex.into(),
            titles: vec![Title {
                title: "JSON title".into(),
            }],
            authors: Vec::new(),
            collaborations: Vec::new(),
            publication_info: Vec::new(),
            arxiv_eprints: arxiv
                .iter()
                .map(|value| ArxivEprint {
                    value: (*value).into(),
                    categories: Vec::new(),
                })
                .collect(),
            dois: doi
                .iter()
                .map(|value| Doi {
                    value: (*value).into(),
                })
                .collect(),
            urls: Vec::new(),
            document_types: Vec::new(),
            preprint_date: None,
            earliest_date: None,
        }
    }

    #[test]
    fn validates_matching_doi_or_arxiv_identity() {
        snapshot(
            "@misc{Key,title={Different},doi={10.1/x}}",
            &["10.1/X"],
            &[],
        )
        .validate()
        .unwrap();
        snapshot(
            "@misc{Key,title={Different},eprint={2401.00001}}",
            &["10.1/no"],
            &["2401.00001v2"],
        )
        .validate()
        .unwrap();
    }

    #[test]
    fn rejects_mismatching_supplied_identities() {
        assert!(
            snapshot("@misc{Key,title={Same}}", &["10.1/x"], &[])
                .validate()
                .is_err()
        );
    }

    #[test]
    fn identifierless_record_relies_on_validated_texkey() {
        snapshot("@misc{Key,title={Unrelated title}}", &[], &[])
            .validate()
            .unwrap();
    }

    #[test]
    fn durable_snapshot_rejects_unknown_fields() {
        let json = r#"{
            "record_id": 1, "updated": "now", "texkeys": ["Key"],
            "bibtex": "@misc{Key,title={Title}}", "titles": [{"title":"Title"}],
            "unknown": true
        }"#;
        assert!(serde_json::from_str::<InspireSnapshot>(json).is_err());
    }
}
