use crate::{
    Client, Error as ClientError, LiteratureId, LiteratureMetadata, LiteratureRecord,
    PublicationInfo,
};
use async_trait::async_trait;
use cita_core::{
    INSPIRE_SOURCE, Locator, MetadataProvider, PaperRecord, ProviderError, Publication,
    ResolvedPaper, strip_arxiv_version,
};

pub struct InspireProvider {
    client: Client,
}

impl InspireProvider {
    pub fn new() -> Result<Self, ClientError> {
        Ok(Self {
            client: Client::new()?,
        })
    }
}

#[async_trait]
impl MetadataProvider for InspireProvider {
    async fn resolve(&self, locator: &Locator) -> Result<ResolvedPaper, ProviderError> {
        let id = match locator {
            Locator::Inspire(id) => LiteratureId::record(*id),
            Locator::Arxiv(id) => LiteratureId::arxiv(id.clone()),
            Locator::Doi(doi) => LiteratureId::doi(doi.clone()),
        }
        .map_err(|error| ProviderError::InvalidLocator(error.to_string()))?;
        let record = self
            .client
            .literature(id)
            .await
            .map_err(|error| match error {
                ClientError::NotFound(_) => ProviderError::NotFound(locator.to_string()),
                ClientError::MalformedResponse(error) => {
                    ProviderError::Malformed(error.to_string())
                }
                error => ProviderError::Request(error.to_string()),
            })?;
        map_record(record)
    }
}

fn map_record(record: LiteratureRecord) -> Result<ResolvedPaper, ProviderError> {
    let LiteratureRecord {
        id,
        metadata,
        extra,
    } = record;
    let title = metadata
        .titles
        .first()
        .map(|value| value.title.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProviderError::Malformed("record has no title".into()))?;
    let inspire_id: Option<u64> = id.as_deref().and_then(|id| id.parse().ok());
    let publication_info = select_publication(&metadata);
    let publication = publication_info.map(to_publication);
    let arxiv_ids = metadata
        .arxiv_eprints
        .iter()
        .map(|item| strip_arxiv_version(&item.value).to_owned())
        .collect::<Vec<_>>();
    let year = publication
        .as_ref()
        .and_then(|item| item.year)
        .or_else(|| date_year(metadata.preprint_date.as_deref()))
        .or_else(|| arxiv_ids.first().and_then(|id| arxiv_year(id)));
    let primary_category = metadata
        .arxiv_eprints
        .first()
        .and_then(|item| item.categories.first())
        .cloned();
    let url = inspire_id
        .map(|id| format!("https://inspirehep.net/literature/{id}"))
        .or_else(|| metadata.urls.first().map(|url| url.value.clone()));
    let source_updated = extra
        .get("updated")
        .and_then(|value| value.as_str())
        .map(str::to_owned)
        .or_else(|| {
            metadata
                .extra
                .get("date")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
        });

    Ok(ResolvedPaper {
        suggested_key: metadata.texkeys.first().cloned(),
        record: PaperRecord {
            title,
            authors: metadata
                .authors
                .iter()
                .filter(|author| {
                    author.role.is_empty()
                        || author
                            .role
                            .iter()
                            .any(|role| role.to_ascii_lowercase().contains("author"))
                })
                .map(|author| author.full_name.clone())
                .collect(),
            collaborations: metadata
                .collaborations
                .iter()
                .map(|item| item.value.clone())
                .collect(),
            year,
            document_types: metadata.document_types,
            url,
            source: INSPIRE_SOURCE.into(),
            source_id: inspire_id.map(|id| id.to_string()),
            arxiv_ids,
            dois: metadata
                .dois
                .into_iter()
                .map(|item| item.value.to_ascii_lowercase())
                .collect(),
            primary_category,
            source_updated,
            preprint_date: metadata.preprint_date,
            publication,
        },
    })
}

fn select_publication(metadata: &LiteratureMetadata) -> Option<&PublicationInfo> {
    let visible_journal = |item: &&PublicationInfo| !item.hidden && item.journal_title.is_some();
    metadata
        .publication_info
        .iter()
        .filter(visible_journal)
        .find(|item| item.curated_relation && (item.page_start.is_some() || item.artid.is_some()))
        .or_else(|| {
            metadata
                .publication_info
                .iter()
                .filter(visible_journal)
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
        let year: i32 = id.get(..2)?.parse().ok()?;
        return Some(2000 + year);
    }
    let number = id.split_once('/')?.1;
    let year: i32 = number.get(..2)?.parse().ok()?;
    Some(if year >= 91 { 1900 + year } else { 2000 + year })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_applies_selection_rules() {
        let record: LiteratureRecord = serde_json::from_value(serde_json::json!({
            "id": "1124337",
            "updated": "2025-01-01T00:00:00+00:00",
            "metadata": {
                "titles": [{"title": "First title"}, {"title": "Second title"}],
                "authors": [
                    {"full_name": "Aad, Georges"},
                    {"full_name": "Editor, Eve", "role": "editor"},
                    {"full_name": "Writer, Will", "role": ["author"]}
                ],
                "collaborations": [{"value": "ATLAS"}],
                "texkeys": ["Aad:2012tfa", "Other"],
                "arxiv_eprints": [{"value": "1207.7214v2", "categories": ["hep-ex"]}],
                "dois": [{"value": "10.1016/TEST"}],
                "document_type": ["article"],
                "publication_info": [
                    {"journal_title": "Hidden", "year": 2011, "page_start": "1", "hidden": true, "curated_relation": true},
                    {"journal_title": "Fallback", "year": 2011, "journal_volume": "1"},
                    {"journal_title": "Phys.Lett.B", "journal_volume": "716", "journal_issue": "1", "page_start": "1", "page_end": "29", "year": 2012, "curated_relation": true}
                ]
            }
        })).unwrap();
        let paper = map_record(record).unwrap();
        assert_eq!(paper.suggested_key.as_deref(), Some("Aad:2012tfa"));
        assert_eq!(paper.record.title, "First title");
        assert_eq!(paper.record.authors, ["Aad, Georges", "Writer, Will"]);
        assert_eq!(paper.record.year, Some(2012));
        assert_eq!(paper.record.source, INSPIRE_SOURCE);
        assert_eq!(paper.record.source_id.as_deref(), Some("1124337"));
        assert_eq!(paper.record.arxiv_ids, ["1207.7214"]);
        assert_eq!(paper.record.primary_category.as_deref(), Some("hep-ex"));
        assert_eq!(
            paper.record.publication.unwrap().pages.as_deref(),
            Some("1-29")
        );
    }

    #[test]
    fn derives_year_from_modern_and_legacy_arxiv_ids() {
        assert_eq!(arxiv_year("2401.00001"), Some(2024));
        assert_eq!(arxiv_year("hep-th/9901001"), Some(1999));
        assert_eq!(arxiv_year("hep-th/0701001"), Some(2007));
    }
}
