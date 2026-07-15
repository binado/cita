use paperdb_core::{
    Paper, ResolvedPaper, fallback_key, normalize_arxiv, normalize_doi, validate_key,
};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;
use thiserror::Error;
use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, Value};

#[derive(Debug)]
pub struct Manifest {
    path: PathBuf,
    document: DocumentMut,
    papers: Vec<Paper>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddOutcome {
    Added(String),
    Existing(String),
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("manifest already exists at {0}")]
    AlreadyExists(PathBuf),
    #[error("could not read manifest {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid manifest {path}: {message}")]
    Invalid { path: PathBuf, message: String },
    #[error("could not write manifest {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("citation key conflict: `{0}` already exists; pass --key with a unique key")]
    KeyConflict(String),
    #[error("identifier conflict: {0}")]
    IdentifierConflict(String),
    #[error("paper `{0}` was not found")]
    PaperNotFound(String),
    #[error("invalid citation key: {0}")]
    InvalidKey(String),
}

#[derive(Deserialize)]
struct ManifestData {
    schema: u32,
    #[serde(default)]
    papers: Vec<Paper>,
}

impl Manifest {
    pub fn create(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref().to_path_buf();
        if path.exists() {
            return Err(Error::AlreadyExists(path));
        }
        let document = "schema = 1\n\n"
            .parse::<DocumentMut>()
            .expect("static TOML is valid");
        let manifest = Self {
            path,
            document,
            papers: Vec::new(),
        };
        manifest.save()?;
        Ok(manifest)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref().to_path_buf();
        let source = fs::read_to_string(&path).map_err(|source| Error::Read {
            path: path.clone(),
            source,
        })?;
        let document = source
            .parse::<DocumentMut>()
            .map_err(|error| Error::Invalid {
                path: path.clone(),
                message: error.to_string(),
            })?;
        let data: ManifestData = toml::from_str(&source).map_err(|error| Error::Invalid {
            path: path.clone(),
            message: error.to_string(),
        })?;
        if data.schema != 1 {
            return Err(Error::Invalid {
                path,
                message: format!("unsupported schema {}", data.schema),
            });
        }
        validate_papers(&data.papers).map_err(|message| Error::Invalid {
            path: path.clone(),
            message,
        })?;
        Ok(Self {
            path,
            document,
            papers: data.papers,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn papers(&self) -> &[Paper] {
        &self.papers
    }

    pub fn add(&mut self, paper: ResolvedPaper, key: Option<&str>) -> Result<AddOutcome, Error> {
        if let Some(key) = key {
            validate_key(key).map_err(Error::InvalidKey)?;
        }
        let mut outcomes = self.insert_papers(vec![(paper, key.map(str::to_owned))])?;
        Ok(outcomes.pop().expect("one paper yields one outcome"))
    }

    pub fn add_batch(&mut self, papers: Vec<ResolvedPaper>) -> Result<Vec<AddOutcome>, Error> {
        self.insert_papers(papers.into_iter().map(|paper| (paper, None)).collect())
    }

    fn insert_papers(
        &mut self,
        resolved: Vec<(ResolvedPaper, Option<String>)>,
    ) -> Result<Vec<AddOutcome>, Error> {
        let original = self.papers.clone();
        let mut additions = Vec::new();
        let mut outcomes = Vec::new();
        for (item, explicit_key) in resolved {
            let matches = matching_indices(&self.papers, &item);
            if matches.len() > 1 {
                self.papers = original;
                return Err(Error::IdentifierConflict(
                    "the resolved identifiers belong to multiple existing papers".into(),
                ));
            }
            if let Some(index) = matches.first() {
                let existing = &self.papers[*index];
                if let (Some(existing_id), Some(candidate_id)) =
                    (existing.inspire_id, item.inspire_id)
                    && existing_id != candidate_id
                {
                    let message = format!(
                        "{} resolves to INSPIRE {}, but its identifier overlaps `{}` (INSPIRE {})",
                        item.title, candidate_id, existing.key, existing_id
                    );
                    self.papers = original;
                    return Err(Error::IdentifierConflict(message));
                }
                outcomes.push(AddOutcome::Existing(self.papers[*index].key.clone()));
                continue;
            }

            let key = explicit_key
                .or_else(|| item.suggested_key.clone())
                .unwrap_or_else(|| fallback_key(&item));
            validate_key(&key).map_err(Error::InvalidKey)?;
            if self.papers.iter().any(|paper| paper.key == key) {
                self.papers = original;
                return Err(Error::KeyConflict(key));
            }
            let paper = item.into_paper(key.clone());
            self.papers.push(paper.clone());
            additions.push(paper);
            outcomes.push(AddOutcome::Added(key));
        }
        if let Err(message) = validate_papers(&self.papers) {
            self.papers = original;
            return Err(Error::IdentifierConflict(message));
        }
        for paper in &additions {
            append_paper(&mut self.document, paper);
        }
        if !additions.is_empty() {
            self.save()?;
        }
        Ok(outcomes)
    }

    pub fn remove_batch(&mut self, selectors: &[String]) -> Result<Vec<Paper>, Error> {
        let mut indices = HashSet::new();
        for selector in selectors {
            let index = find_selector(&self.papers, selector)
                .ok_or_else(|| Error::PaperNotFound(selector.clone()))?;
            indices.insert(index);
        }
        let removed = self
            .papers
            .iter()
            .enumerate()
            .filter(|(index, _)| indices.contains(index))
            .map(|(_, paper)| paper.clone())
            .collect::<Vec<_>>();
        self.papers = self
            .papers
            .iter()
            .enumerate()
            .filter(|(index, _)| !indices.contains(index))
            .map(|(_, paper)| paper.clone())
            .collect();
        if let Some(array) = self
            .document
            .get_mut("papers")
            .and_then(Item::as_array_of_tables_mut)
        {
            let retained = array
                .iter()
                .enumerate()
                .filter(|(index, _)| !indices.contains(index))
                .map(|(_, table)| table.clone())
                .collect::<Vec<_>>();
            array.clear();
            for table in retained {
                array.push(table);
            }
        }
        self.save()?;
        Ok(removed)
    }

    pub fn save(&self) -> Result<(), Error> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let mut temporary = NamedTempFile::new_in(parent).map_err(|source| Error::Write {
            path: self.path.clone(),
            source,
        })?;
        temporary
            .write_all(self.document.to_string().as_bytes())
            .map_err(|source| Error::Write {
                path: self.path.clone(),
                source,
            })?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|source| Error::Write {
                path: self.path.clone(),
                source,
            })?;
        temporary
            .persist(&self.path)
            .map_err(|error| Error::Write {
                path: self.path.clone(),
                source: error.error,
            })?;
        Ok(())
    }
}

fn append_paper(document: &mut DocumentMut, paper: &Paper) {
    if !document.contains_key("papers") {
        document["papers"] = Item::ArrayOfTables(ArrayOfTables::new());
    }
    document["papers"]
        .as_array_of_tables_mut()
        .expect("validated papers is an array of tables")
        .push(paper_table(paper));
}

fn paper_table(paper: &Paper) -> Table {
    let mut table = Table::new();
    insert(&mut table, "key", paper.key.clone());
    insert(&mut table, "title", paper.title.clone());
    insert_array(&mut table, "authors", &paper.authors);
    insert_array(&mut table, "collaborations", &paper.collaborations);
    if let Some(year) = paper.year {
        insert(&mut table, "year", i64::from(year));
    }
    insert_array(&mut table, "document_types", &paper.document_types);
    insert_opt(&mut table, "url", paper.url.as_deref());
    if let Some(id) = paper.inspire_id {
        let id = i64::try_from(id).unwrap_or(i64::MAX);
        insert(&mut table, "inspire_id", id);
    }
    insert_array(&mut table, "arxiv_ids", &paper.arxiv_ids);
    insert_array(&mut table, "dois", &paper.dois);
    insert_opt(
        &mut table,
        "primary_category",
        paper.primary_category.as_deref(),
    );
    insert(&mut table, "source", paper.source.clone());
    insert_opt(
        &mut table,
        "source_updated",
        paper.source_updated.as_deref(),
    );
    insert_opt(&mut table, "preprint_date", paper.preprint_date.as_deref());
    if let Some(publication) = &paper.publication {
        let mut child = Table::new();
        child.set_implicit(false);
        insert_opt(&mut child, "journal", publication.journal.as_deref());
        insert_opt(&mut child, "volume", publication.volume.as_deref());
        insert_opt(&mut child, "issue", publication.issue.as_deref());
        insert_opt(&mut child, "pages", publication.pages.as_deref());
        if let Some(year) = publication.year {
            insert(&mut child, "year", i64::from(year));
        }
        table.insert("publication", Item::Table(child));
    }
    table
}

fn insert(table: &mut Table, key: &str, value: impl Into<Value>) {
    table.insert(key, Item::Value(value.into()));
}

fn insert_opt(table: &mut Table, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        insert(table, key, value);
    }
}

fn insert_array(table: &mut Table, key: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    let mut array = Array::new();
    for value in values {
        array.push(value.as_str());
    }
    table.insert(key, Item::Value(Value::Array(array)));
}

fn matching_indices(papers: &[Paper], candidate: &ResolvedPaper) -> Vec<usize> {
    papers
        .iter()
        .enumerate()
        .filter(|(_, paper)| identifiers_overlap(paper, candidate))
        .map(|(index, _)| index)
        .collect()
}

fn identifiers_overlap(paper: &Paper, candidate: &ResolvedPaper) -> bool {
    paper.inspire_id.is_some() && paper.inspire_id == candidate.inspire_id
        || overlaps_normalized(&paper.arxiv_ids, &candidate.arxiv_ids, normalize_arxiv)
        || overlaps_normalized(&paper.dois, &candidate.dois, normalize_doi)
}

fn overlaps_normalized(left: &[String], right: &[String], normalize: fn(&str) -> String) -> bool {
    left.iter().any(|a| {
        let a = normalize(a);
        right.iter().any(|b| a == normalize(b))
    })
}

fn find_selector(papers: &[Paper], selector: &str) -> Option<usize> {
    if let Some(index) = papers.iter().position(|paper| paper.key == selector) {
        return Some(index);
    }
    let locator = selector.parse::<paperdb_core::Locator>().ok()?;
    papers.iter().position(|paper| match &locator {
        paperdb_core::Locator::Inspire(id) => paper.inspire_id == Some(*id),
        paperdb_core::Locator::Arxiv(id) => paper
            .arxiv_ids
            .iter()
            .any(|value| normalize_arxiv(value) == normalize_arxiv(id)),
        paperdb_core::Locator::Doi(doi) => paper
            .dois
            .iter()
            .any(|value| normalize_doi(value) == normalize_doi(doi)),
    })
}

fn validate_papers(papers: &[Paper]) -> Result<(), String> {
    let mut keys = HashSet::new();
    let mut inspire = HashMap::new();
    let mut arxiv = HashMap::new();
    let mut dois = HashMap::new();
    for paper in papers {
        validate_key(&paper.key)?;
        if !keys.insert(paper.key.clone()) {
            return Err(format!("duplicate citation key `{}`", paper.key));
        }
        if let Some(id) = paper.inspire_id
            && let Some(other) = inspire.insert(id, &paper.key)
        {
            return Err(format!(
                "INSPIRE id {id} is shared by `{other}` and `{}`",
                paper.key
            ));
        }
        for id in &paper.arxiv_ids {
            let id = normalize_arxiv(id);
            if let Some(other) = arxiv.insert(id.clone(), &paper.key)
                && other != &paper.key
            {
                return Err(format!(
                    "arXiv id {id} is shared by `{other}` and `{}`",
                    paper.key
                ));
            }
        }
        for doi in &paper.dois {
            let doi = normalize_doi(doi);
            if let Some(other) = dois.insert(doi.clone(), &paper.key)
                && other != &paper.key
            {
                return Err(format!(
                    "DOI {doi} is shared by `{other}` and `{}`",
                    paper.key
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(key: &str, id: u64) -> ResolvedPaper {
        ResolvedPaper {
            suggested_key: Some(key.into()),
            title: format!("Paper {id}"),
            inspire_id: Some(id),
            source: "inspire".into(),
            ..ResolvedPaper::default()
        }
    }

    #[test]
    fn preserves_comments_unknown_fields_and_is_atomic_on_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("paperdb.toml");
        fs::write(&path, "schema = 1 # keep\ncustom = 'yes'\n\n[[papers]]\nkey = 'One'\ntitle = 'First'\nsource = 'inspire'\ninspire_id = 1\nunknown = 42 # also keep\n").unwrap();
        let mut manifest = Manifest::load(&path).unwrap();
        manifest.add(resolved("Two", 2), None).unwrap();
        let after_add = fs::read_to_string(&path).unwrap();
        assert!(after_add.contains("# keep"));
        assert!(after_add.contains("unknown = 42 # also keep"));
        let error = manifest.add_batch(vec![resolved("Three", 3), resolved("Two", 4)]);
        assert!(matches!(error, Err(Error::KeyConflict(_))));
        assert_eq!(fs::read_to_string(&path).unwrap(), after_add);
    }

    #[test]
    fn removals_are_all_or_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("paperdb.toml");
        let mut manifest = Manifest::create(&path).unwrap();
        manifest
            .add_batch(vec![resolved("One", 1), resolved("Two", 2)])
            .unwrap();
        let before = fs::read_to_string(&path).unwrap();
        assert!(
            manifest
                .remove_batch(&["One".into(), "missing".into()])
                .is_err()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn treats_arxiv_versions_and_doi_case_as_the_same_identifiers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("paperdb.toml");
        let mut manifest = Manifest::create(&path).unwrap();
        let mut first = resolved("One", 1);
        first.arxiv_ids = vec!["2401.00001v2".into()];
        first.dois = vec!["10.1000/ABC".into()];
        manifest.add(first, None).unwrap();

        let mut same_arxiv = resolved("Other", 1);
        same_arxiv.arxiv_ids = vec!["2401.00001".into()];
        assert_eq!(
            manifest.add(same_arxiv, None).unwrap(),
            AddOutcome::Existing("One".into())
        );
        assert_eq!(
            manifest.remove_batch(&["doi:10.1000/abc".into()]).unwrap()[0].key,
            "One"
        );
    }

    #[test]
    fn treats_arxiv_case_as_the_same_identifier() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("paperdb.toml");
        let mut manifest = Manifest::create(&path).unwrap();
        let mut first = resolved("One", 1);
        first.arxiv_ids = vec!["HEP-TH/9901001v2".into()];
        manifest.add(first, None).unwrap();

        let mut same_arxiv = resolved("Other", 1);
        same_arxiv.arxiv_ids = vec!["hep-th/9901001".into()];
        assert_eq!(
            manifest.add(same_arxiv, None).unwrap(),
            AddOutcome::Existing("One".into())
        );
    }

    #[test]
    fn rejects_overlapping_identifiers_with_different_inspire_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("paperdb.toml");
        let mut manifest = Manifest::create(&path).unwrap();
        let mut first = resolved("One", 1);
        first.arxiv_ids = vec!["2401.00001".into()];
        manifest.add(first, None).unwrap();
        let before = fs::read_to_string(&path).unwrap();

        let mut conflicting = resolved("Two", 2);
        conflicting.arxiv_ids = vec!["2401.00001v3".into()];
        assert!(matches!(
            manifest.add(conflicting, None),
            Err(Error::IdentifierConflict(_))
        ));
        assert_eq!(fs::read_to_string(path).unwrap(), before);
    }
}
