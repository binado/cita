use cita_core::{
    INSPIRE_SOURCE, Locator, PaperRecord, ResolvedPaper, fallback_key, normalize_arxiv,
    normalize_doi, validate_key,
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;
use thiserror::Error;
use toml_edit::{Array, DocumentMut, Item, Table, Value};

#[derive(Debug)]
pub struct Manifest {
    path: PathBuf,
    document: DocumentMut,
    papers: BTreeMap<String, PaperRecord>,
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
    papers: BTreeMap<String, PaperRecord>,
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
            papers: BTreeMap::new(),
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
        // Papers must live in `[papers.<key>]` tables. An inline `papers = { .. }`
        // map deserializes fine but is invisible to the `toml_edit` edit path,
        // so removals would report success while leaving the file untouched.
        if let Some(item) = document.get("papers")
            && !item.is_table()
        {
            return Err(Error::Invalid {
                path,
                message: "`papers` must be a table of `[papers.<key>]` entries, \
                          not an inline map"
                    .into(),
            });
        }
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
    pub fn papers(&self) -> &BTreeMap<String, PaperRecord> {
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
            let matches = matching_keys(&self.papers, &item.record);
            if matches.len() > 1 {
                self.papers = original;
                return Err(Error::IdentifierConflict(
                    "the resolved identifiers belong to multiple existing papers".into(),
                ));
            }
            if let Some(existing_key) = matches.first() {
                let existing = &self.papers[existing_key];
                if existing.source == item.record.source
                    && let (Some(existing_id), Some(candidate_id)) =
                        (&existing.source_id, &item.record.source_id)
                    && existing_id != candidate_id
                {
                    let message = format!(
                        "{} resolves to {} {}, but its identifier overlaps `{}` ({} {})",
                        item.record.title,
                        item.record.source,
                        candidate_id,
                        existing_key,
                        existing.source,
                        existing_id
                    );
                    self.papers = original;
                    return Err(Error::IdentifierConflict(message));
                }
                outcomes.push(AddOutcome::Existing(existing_key.clone()));
                continue;
            }

            let key = explicit_key
                .or(item.suggested_key)
                .unwrap_or_else(|| fallback_key(&item.record));
            if let Err(message) = validate_key(&key) {
                self.papers = original;
                return Err(Error::InvalidKey(message));
            }
            // The key is taken by a paper with a *different* identity (an
            // identity match would have hit the `Existing` arm above), so
            // inserting would silently replace it — refuse instead.
            if self.papers.contains_key(&key) {
                self.papers = original;
                return Err(Error::KeyConflict(key));
            }
            self.papers.insert(key.clone(), item.record.clone());
            additions.push((key.clone(), item.record));
            outcomes.push(AddOutcome::Added(key));
        }
        if let Err(message) = validate_papers(&self.papers) {
            self.papers = original;
            return Err(Error::IdentifierConflict(message));
        }
        for (key, record) in &additions {
            append_paper(&mut self.document, key, record);
        }
        if !additions.is_empty() {
            self.save()?;
        }
        Ok(outcomes)
    }

    pub fn remove_batch(
        &mut self,
        selectors: &[String],
    ) -> Result<Vec<(String, PaperRecord)>, Error> {
        let mut keys = BTreeSet::new();
        for selector in selectors {
            let key = find_selector(&self.papers, selector)
                .ok_or_else(|| Error::PaperNotFound(selector.clone()))?;
            keys.insert(key);
        }
        let mut removed = Vec::new();
        for key in &keys {
            let record = self
                .papers
                .remove(key)
                .expect("selector resolved to a present key");
            removed.push((key.clone(), record));
        }
        if let Some(table) = self.document.get_mut("papers").and_then(Item::as_table_mut) {
            for key in &keys {
                table.remove(key);
            }
        }
        self.save()?;
        Ok(removed)
    }

    pub fn save(&self) -> Result<(), Error> {
        let mut document = self.document.clone();
        sort_paper_tables(&mut document);
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let mut temporary = NamedTempFile::new_in(parent).map_err(|source| Error::Write {
            path: self.path.clone(),
            source,
        })?;
        temporary
            .write_all(document.to_string().as_bytes())
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

/// Render `[papers.<key>]` tables in key order regardless of insertion or
/// hand-edited order, so saved manifests are deterministic and diff-friendly.
/// Comments travel with their tables.
fn sort_paper_tables(document: &mut DocumentMut) {
    let Some(papers) = document.get_mut("papers").and_then(Item::as_table_mut) else {
        return;
    };
    let mut keys = papers
        .iter()
        .map(|(key, _)| key.to_owned())
        .collect::<Vec<_>>();
    keys.sort_unstable();
    for (position, key) in keys.iter().enumerate() {
        if let Some(table) = papers.get_mut(key).and_then(Item::as_table_mut) {
            table.set_position(position);
        }
    }
}

fn append_paper(document: &mut DocumentMut, key: &str, record: &PaperRecord) {
    if !document.contains_key("papers") {
        let mut papers = Table::new();
        papers.set_implicit(true);
        document.insert("papers", Item::Table(papers));
    }
    document["papers"]
        .as_table_mut()
        .expect("validated papers is a table")
        .insert(key, Item::Table(paper_table(record)));
}

fn paper_table(record: &PaperRecord) -> Table {
    let mut table = Table::new();
    insert(&mut table, "title", record.title.clone());
    insert_array(&mut table, "authors", &record.authors);
    insert_array(&mut table, "collaborations", &record.collaborations);
    if let Some(year) = record.year {
        insert(&mut table, "year", i64::from(year));
    }
    insert_array(&mut table, "document_types", &record.document_types);
    insert_opt(&mut table, "url", record.url.as_deref());
    insert(&mut table, "source", record.source.clone());
    insert_opt(&mut table, "source_id", record.source_id.as_deref());
    insert_array(&mut table, "arxiv_ids", &record.arxiv_ids);
    insert_array(&mut table, "dois", &record.dois);
    insert_opt(
        &mut table,
        "primary_category",
        record.primary_category.as_deref(),
    );
    insert_opt(
        &mut table,
        "source_updated",
        record.source_updated.as_deref(),
    );
    insert_opt(&mut table, "preprint_date", record.preprint_date.as_deref());
    if let Some(publication) = &record.publication {
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

fn matching_keys(papers: &BTreeMap<String, PaperRecord>, candidate: &PaperRecord) -> Vec<String> {
    papers
        .iter()
        .filter(|(_, record)| identifiers_overlap(record, candidate))
        .map(|(key, _)| key.clone())
        .collect()
}

fn identifiers_overlap(record: &PaperRecord, candidate: &PaperRecord) -> bool {
    record.source_id.is_some()
        && record.source == candidate.source
        && record.source_id == candidate.source_id
        || overlaps_normalized(&record.arxiv_ids, &candidate.arxiv_ids, normalize_arxiv)
        || overlaps_normalized(&record.dois, &candidate.dois, normalize_doi)
}

fn overlaps_normalized(left: &[String], right: &[String], normalize: fn(&str) -> String) -> bool {
    left.iter().any(|a| {
        let a = normalize(a);
        right.iter().any(|b| a == normalize(b))
    })
}

fn find_selector(papers: &BTreeMap<String, PaperRecord>, selector: &str) -> Option<String> {
    if papers.contains_key(selector) {
        return Some(selector.to_owned());
    }
    let locator = selector.parse::<Locator>().ok()?;
    papers
        .iter()
        .find(|(_, record)| match &locator {
            Locator::Inspire(id) => {
                record.source == INSPIRE_SOURCE
                    && record.source_id.as_deref() == Some(id.to_string().as_str())
            }
            Locator::Arxiv(id) => record
                .arxiv_ids
                .iter()
                .any(|value| normalize_arxiv(value) == normalize_arxiv(id)),
            Locator::Doi(doi) => record
                .dois
                .iter()
                .any(|value| normalize_doi(value) == normalize_doi(doi)),
        })
        .map(|(key, _)| key.clone())
}

fn validate_papers(papers: &BTreeMap<String, PaperRecord>) -> Result<(), String> {
    let mut sources = HashMap::new();
    let mut arxiv = HashMap::new();
    let mut dois = HashMap::new();
    for (key, record) in papers {
        validate_key(key)?;
        if let Some(id) = &record.source_id
            && let Some(other) = sources.insert((record.source.clone(), id.clone()), key)
        {
            return Err(format!(
                "{} id {id} is shared by `{other}` and `{key}`",
                record.source
            ));
        }
        for id in &record.arxiv_ids {
            let id = normalize_arxiv(id);
            if let Some(other) = arxiv.insert(id.clone(), key)
                && other != key
            {
                return Err(format!("arXiv id {id} is shared by `{other}` and `{key}`"));
            }
        }
        for doi in &record.dois {
            let doi = normalize_doi(doi);
            if let Some(other) = dois.insert(doi.clone(), key)
                && other != key
            {
                return Err(format!("DOI {doi} is shared by `{other}` and `{key}`"));
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
            record: PaperRecord {
                title: format!("Paper {id}"),
                source: INSPIRE_SOURCE.into(),
                source_id: Some(id.to_string()),
                ..PaperRecord::default()
            },
        }
    }

    #[test]
    fn preserves_comments_unknown_fields_and_is_atomic_on_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cita.toml");
        fs::write(&path, "schema = 1 # keep\ncustom = 'yes'\n\n[papers.One]\ntitle = 'First'\nsource = 'inspire'\nsource_id = '1'\nunknown = 42 # also keep\n").unwrap();
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
    fn key_collision_with_different_identity_is_a_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cita.toml");
        let mut manifest = Manifest::create(&path).unwrap();
        manifest.add(resolved("One", 1), None).unwrap();
        let before = fs::read_to_string(&path).unwrap();
        let error = manifest.add(resolved("One", 2), None);
        assert!(matches!(error, Err(Error::KeyConflict(key)) if key == "One"));
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn removals_are_all_or_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cita.toml");
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
        let path = dir.path().join("cita.toml");
        let mut manifest = Manifest::create(&path).unwrap();
        let mut first = resolved("One", 1);
        first.record.arxiv_ids = vec!["2401.00001v2".into()];
        first.record.dois = vec!["10.1000/ABC".into()];
        manifest.add(first, None).unwrap();

        let mut same_arxiv = resolved("Other", 1);
        same_arxiv.record.arxiv_ids = vec!["2401.00001".into()];
        assert_eq!(
            manifest.add(same_arxiv, None).unwrap(),
            AddOutcome::Existing("One".into())
        );
        assert_eq!(
            manifest.remove_batch(&["doi:10.1000/abc".into()]).unwrap()[0].0,
            "One"
        );
    }

    #[test]
    fn treats_arxiv_case_as_the_same_identifier() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cita.toml");
        let mut manifest = Manifest::create(&path).unwrap();
        let mut first = resolved("One", 1);
        first.record.arxiv_ids = vec!["HEP-TH/9901001v2".into()];
        manifest.add(first, None).unwrap();

        let mut same_arxiv = resolved("Other", 1);
        same_arxiv.record.arxiv_ids = vec!["hep-th/9901001".into()];
        assert_eq!(
            manifest.add(same_arxiv, None).unwrap(),
            AddOutcome::Existing("One".into())
        );
    }

    #[test]
    fn rejects_overlapping_identifiers_with_different_inspire_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cita.toml");
        let mut manifest = Manifest::create(&path).unwrap();
        let mut first = resolved("One", 1);
        first.record.arxiv_ids = vec!["2401.00001".into()];
        manifest.add(first, None).unwrap();
        let before = fs::read_to_string(&path).unwrap();

        let mut conflicting = resolved("Two", 2);
        conflicting.record.arxiv_ids = vec!["2401.00001v3".into()];
        assert!(matches!(
            manifest.add(conflicting, None),
            Err(Error::IdentifierConflict(_))
        ));
        assert_eq!(fs::read_to_string(path).unwrap(), before);
    }

    #[test]
    fn rejects_duplicate_paper_tables_in_hand_written_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cita.toml");
        fs::write(
            &path,
            "schema = 1\n\n[papers.One]\ntitle = 'First'\nsource = 'inspire'\n\n[papers.One]\ntitle = 'Again'\nsource = 'inspire'\n",
        )
        .unwrap();
        assert!(matches!(Manifest::load(&path), Err(Error::Invalid { .. })));
    }

    #[test]
    fn rejects_inline_papers_maps_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cita.toml");
        fs::write(
            &path,
            "schema = 1\npapers = { One = { title = 'First', source = 'inspire' } }\n",
        )
        .unwrap();
        assert!(matches!(Manifest::load(&path), Err(Error::Invalid { .. })));
    }

    #[test]
    fn keys_with_colons_round_trip_as_quoted_table_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cita.toml");
        let mut manifest = Manifest::create(&path).unwrap();
        manifest.add(resolved("Aad:2012tfa", 1), None).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains(r#"[papers."Aad:2012tfa"]"#), "{text}");
        let reloaded = Manifest::load(&path).unwrap();
        assert!(reloaded.papers().contains_key("Aad:2012tfa"));
    }

    #[test]
    fn save_renders_paper_tables_key_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cita.toml");
        fs::write(
            &path,
            "schema = 1\n\n[papers.Zed]\ntitle = 'Last'\nsource = 'inspire'\nsource_id = '9'\n\n# alpha comment\n[papers.Alpha]\ntitle = 'First'\nsource = 'inspire'\nsource_id = '1'\n",
        )
        .unwrap();
        let mut manifest = Manifest::load(&path).unwrap();
        manifest.add(resolved("Mid", 5), None).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let alpha = text.find("[papers.Alpha]").unwrap();
        let mid = text.find("[papers.Mid]").unwrap();
        let zed = text.find("[papers.Zed]").unwrap();
        assert!(alpha < mid && mid < zed, "{text}");
        assert!(text.contains("# alpha comment"), "{text}");
    }
}
