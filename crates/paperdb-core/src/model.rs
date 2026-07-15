use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Publication {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
}

impl Publication {
    pub fn is_journal(&self) -> bool {
        self.journal.is_some()
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Paper {
    pub key: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collaborations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub document_types: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspire_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arxiv_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dois: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_category: Option<String>,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_updated: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preprint_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<Publication>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolvedPaper {
    pub suggested_key: Option<String>,
    pub title: String,
    pub authors: Vec<String>,
    pub collaborations: Vec<String>,
    pub year: Option<i32>,
    pub document_types: Vec<String>,
    pub url: Option<String>,
    pub inspire_id: Option<u64>,
    pub arxiv_ids: Vec<String>,
    pub dois: Vec<String>,
    pub primary_category: Option<String>,
    pub source: String,
    pub source_updated: Option<String>,
    pub preprint_date: Option<String>,
    pub publication: Option<Publication>,
}

impl ResolvedPaper {
    pub fn into_paper(self, key: String) -> Paper {
        Paper {
            key,
            title: self.title,
            authors: self.authors,
            collaborations: self.collaborations,
            year: self.year,
            document_types: self.document_types,
            url: self.url,
            inspire_id: self.inspire_id,
            arxiv_ids: self.arxiv_ids,
            dois: self.dois,
            primary_category: self.primary_category,
            source: self.source,
            source_updated: self.source_updated,
            preprint_date: self.preprint_date,
            publication: self.publication,
        }
    }
}
