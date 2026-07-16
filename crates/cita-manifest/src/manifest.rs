use cita_core::{
    INSPIRE_SOURCE, Locator, PaperRecord, Publication, ResolvedPaper, fallback_key,
    normalize_arxiv, normalize_doi, validate_key,
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
    Updated(String),
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
    #[error("paper already stored as `{existing}`; omit --key or pass --key {existing}")]
    CannotRename { existing: String },
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

    /// Find a stored paper by exact citation key, then by locator.
    pub fn find_paper(&self, selector: &str) -> Option<(&str, &PaperRecord)> {
        let key = find_selector(&self.papers, selector)?;
        let (key, record) = self
            .papers
            .get_key_value(key)
            .expect("selector resolved to a present key");
        Some((key, record))
    }

    /// Find a stored paper using the same citation-key or locator syntax as
    /// [`Self::remove_batch`], returning an error when it is absent.
    pub fn paper(&self, selector: &str) -> Result<(&str, &PaperRecord), Error> {
        self.find_paper(selector)
            .ok_or_else(|| Error::PaperNotFound(selector.to_owned()))
    }

    pub fn add(
        &mut self,
        paper: ResolvedPaper,
        key: Option<&str>,
        force: bool,
    ) -> Result<AddOutcome, Error> {
        if let Some(key) = key {
            validate_key(key).map_err(Error::InvalidKey)?;
        }
        let mut outcomes = self.insert_papers(vec![(paper, key.map(str::to_owned))], force)?;
        Ok(outcomes.pop().expect("one paper yields one outcome"))
    }

    pub fn add_batch(
        &mut self,
        papers: Vec<ResolvedPaper>,
        force: bool,
    ) -> Result<Vec<AddOutcome>, Error> {
        self.insert_papers(
            papers.into_iter().map(|paper| (paper, None)).collect(),
            force,
        )
    }

    fn insert_papers(
        &mut self,
        resolved: Vec<(ResolvedPaper, Option<String>)>,
        force: bool,
    ) -> Result<Vec<AddOutcome>, Error> {
        let original = self.papers.clone();
        let mut additions = Vec::new();
        let mut updates = Vec::new();
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
                if let Some(requested) = &explicit_key
                    && requested != existing_key
                {
                    self.papers = original;
                    return Err(Error::CannotRename {
                        existing: existing_key.clone(),
                    });
                }
                if force && self.papers[existing_key] != item.record {
                    let key = existing_key.clone();
                    self.papers.insert(key.clone(), item.record.clone());
                    updates.push((key.clone(), item.record));
                    outcomes.push(AddOutcome::Updated(key));
                } else {
                    outcomes.push(AddOutcome::Existing(existing_key.clone()));
                }
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
        for (key, record) in &updates {
            update_paper(&mut self.document, key, record);
        }
        if !additions.is_empty() || !updates.is_empty() {
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
            keys.insert(key.to_owned());
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

/// Update known fields on an existing `[papers.<key>]` table in place, leaving
/// comments, unknown keys, and table decor intact.
fn update_paper(document: &mut DocumentMut, key: &str, record: &PaperRecord) {
    let table = document["papers"]
        .as_table_mut()
        .expect("validated papers is a table")
        .get_mut(key)
        .and_then(Item::as_table_mut)
        .expect("updated key must already exist in the document");
    apply_record(table, record);
}

fn apply_record(table: &mut Table, record: &PaperRecord) {
    insert(table, "title", record.title.clone());
    set_array(table, "authors", &record.authors);
    set_array(table, "collaborations", &record.collaborations);
    set_i32(table, "year", record.year);
    set_array(table, "document_types", &record.document_types);
    set_opt(table, "url", record.url.as_deref());
    insert(table, "source", record.source.clone());
    set_opt(table, "source_id", record.source_id.as_deref());
    set_array(table, "arxiv_ids", &record.arxiv_ids);
    set_array(table, "dois", &record.dois);
    set_opt(
        table,
        "primary_category",
        record.primary_category.as_deref(),
    );
    set_opt(table, "source_updated", record.source_updated.as_deref());
    set_opt(table, "preprint_date", record.preprint_date.as_deref());
    match &record.publication {
        Some(publication) => {
            if !table.contains_key("publication") {
                let mut child = Table::new();
                child.set_implicit(false);
                table.insert("publication", Item::Table(child));
            }
            let child = table
                .get_mut("publication")
                .and_then(Item::as_table_mut)
                .expect("publication was just ensured");
            apply_publication(child, publication);
        }
        None => {
            remove_publication(table);
        }
    }
}

fn apply_publication(table: &mut Table, publication: &Publication) {
    set_opt(table, "journal", publication.journal.as_deref());
    set_opt(table, "volume", publication.volume.as_deref());
    set_opt(table, "issue", publication.issue.as_deref());
    set_opt(table, "pages", publication.pages.as_deref());
    set_i32(table, "year", publication.year);
}

fn remove_publication(table: &mut Table) {
    let remove_table = match table.get_mut("publication").and_then(Item::as_table_mut) {
        Some(publication) => {
            for key in ["journal", "volume", "issue", "pages", "year"] {
                publication.remove(key);
            }
            publication.is_empty()
        }
        None => false,
    };
    if remove_table {
        table.remove("publication");
    }
}

fn paper_table(record: &PaperRecord) -> Table {
    let mut table = Table::new();
    apply_record(&mut table, record);
    table
}

fn insert(table: &mut Table, key: &str, value: impl Into<Value>) {
    let mut value = value.into();
    if let Some(existing) = table.get(key).and_then(Item::as_value) {
        *value.decor_mut() = existing.decor().clone();
    }
    table.insert(key, Item::Value(value));
}

fn set_opt(table: &mut Table, key: &str, value: Option<&str>) {
    match value {
        Some(value) => insert(table, key, value),
        None => {
            table.remove(key);
        }
    }
}

fn set_i32(table: &mut Table, key: &str, value: Option<i32>) {
    match value {
        Some(value) => insert(table, key, i64::from(value)),
        None => {
            table.remove(key);
        }
    }
}

fn set_array(table: &mut Table, key: &str, values: &[String]) {
    if values.is_empty() {
        table.remove(key);
        return;
    }
    let mut array = Array::new();
    for value in values {
        array.push(value.as_str());
    }
    insert(table, key, Value::Array(array));
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

fn find_selector<'a>(papers: &'a BTreeMap<String, PaperRecord>, selector: &str) -> Option<&'a str> {
    if let Some((key, _)) = papers.get_key_value(selector) {
        return Some(key);
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
        .map(|(key, _)| key.as_str())
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
        manifest.add(resolved("Two", 2), None, false).unwrap();
        let after_add = fs::read_to_string(&path).unwrap();
        assert!(after_add.contains("# keep"));
        assert!(after_add.contains("unknown = 42 # also keep"));
        let error = manifest.add_batch(vec![resolved("Three", 3), resolved("Two", 4)], false);
        assert!(matches!(error, Err(Error::KeyConflict(_))));
        assert_eq!(fs::read_to_string(&path).unwrap(), after_add);
    }

    #[test]
    fn key_collision_with_different_identity_is_a_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cita.toml");
        let mut manifest = Manifest::create(&path).unwrap();
        manifest.add(resolved("One", 1), None, false).unwrap();
        let before = fs::read_to_string(&path).unwrap();
        let error = manifest.add(resolved("One", 2), None, false);
        assert!(matches!(error, Err(Error::KeyConflict(key)) if key == "One"));
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn removals_are_all_or_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cita.toml");
        let mut manifest = Manifest::create(&path).unwrap();
        manifest
            .add_batch(vec![resolved("One", 1), resolved("Two", 2)], false)
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
        manifest.add(first, None, false).unwrap();

        let mut same_arxiv = resolved("Other", 1);
        same_arxiv.record.arxiv_ids = vec!["2401.00001".into()];
        assert_eq!(
            manifest.add(same_arxiv, None, false).unwrap(),
            AddOutcome::Existing("One".into())
        );
        assert_eq!(
            manifest.remove_batch(&["doi:10.1000/abc".into()]).unwrap()[0].0,
            "One"
        );
    }

    #[test]
    fn finds_papers_by_key_and_normalized_locators() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cita.toml");
        let mut manifest = Manifest::create(&path).unwrap();
        let mut item = resolved("One", 42);
        item.record.arxiv_ids = vec!["HEP-TH/9901001v2".into()];
        item.record.dois = vec!["10.1000/ABC".into()];
        manifest.add(item, None, false).unwrap();

        for selector in ["One", "hep-th/9901001", "doi:10.1000/abc", "inspire:42"] {
            let (key, record) = manifest.find_paper(selector).unwrap();
            assert_eq!(key, "One");
            assert_eq!(record.title, "Paper 42");
        }
        assert!(manifest.find_paper("missing").is_none());
        assert!(matches!(
            manifest.paper("missing"),
            Err(Error::PaperNotFound(selector)) if selector == "missing"
        ));
    }

    #[test]
    fn exact_key_takes_precedence_over_locator_matching() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cita.toml");
        let mut manifest = Manifest::create(&path).unwrap();
        manifest.add(resolved("1207.7214", 1), None, false).unwrap();
        let mut locator_match = resolved("Other", 2);
        locator_match.record.arxiv_ids = vec!["1207.7214".into()];
        manifest.add(locator_match, None, false).unwrap();

        let (key, record) = manifest.find_paper("1207.7214").unwrap();
        assert_eq!(key, "1207.7214");
        assert_eq!(record.source_id.as_deref(), Some("1"));
    }

    #[test]
    fn treats_arxiv_case_as_the_same_identifier() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cita.toml");
        let mut manifest = Manifest::create(&path).unwrap();
        let mut first = resolved("One", 1);
        first.record.arxiv_ids = vec!["HEP-TH/9901001v2".into()];
        manifest.add(first, None, false).unwrap();

        let mut same_arxiv = resolved("Other", 1);
        same_arxiv.record.arxiv_ids = vec!["hep-th/9901001".into()];
        assert_eq!(
            manifest.add(same_arxiv, None, false).unwrap(),
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
        manifest.add(first, None, false).unwrap();
        let before = fs::read_to_string(&path).unwrap();

        let mut conflicting = resolved("Two", 2);
        conflicting.record.arxiv_ids = vec!["2401.00001v3".into()];
        assert!(matches!(
            manifest.add(conflicting, None, false),
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
        manifest
            .add(resolved("Aad:2012tfa", 1), None, false)
            .unwrap();
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
        manifest.add(resolved("Mid", 5), None, false).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let alpha = text.find("[papers.Alpha]").unwrap();
        let mid = text.find("[papers.Mid]").unwrap();
        let zed = text.find("[papers.Zed]").unwrap();
        assert!(alpha < mid && mid < zed, "{text}");
        assert!(text.contains("# alpha comment"), "{text}");
    }

    #[test]
    fn force_updates_metadata_in_place_preserving_comments_and_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cita.toml");
        fs::write(
            &path,
            "schema = 1 # keep\n\n[papers.One]\ntitle = 'Old' # keep title comment\nauthors = ['Old Author'] # keep authors comment\nsource = 'inspire'\nsource_id = '1'\narxiv_ids = ['1207.7214'] # keep ids comment\nunknown = 42 # also keep\n\n[papers.One.publication]\njournal = 'Old Journal'\nunknown_nested = 'yes' # keep nested unknown\n",
        )
        .unwrap();
        let mut manifest = Manifest::load(&path).unwrap();

        let mut refreshed = resolved("Suggested", 1);
        refreshed.record.title = "New".into();
        refreshed.record.authors = vec!["New Author".into()];
        refreshed.record.arxiv_ids = vec!["1207.7214".into()];
        refreshed.record.dois = vec!["10.1000/new".into()];

        assert_eq!(
            manifest.add(refreshed.clone(), None, false).unwrap(),
            AddOutcome::Existing("One".into())
        );
        assert_eq!(manifest.papers()["One"].title, "Old");

        assert_eq!(
            manifest.add(refreshed.clone(), None, true).unwrap(),
            AddOutcome::Updated("One".into())
        );
        assert_eq!(manifest.papers()["One"].title, "New");
        assert_eq!(
            manifest.papers()["One"].dois,
            vec!["10.1000/new".to_string()]
        );
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# keep"), "{text}");
        assert!(text.contains("# keep title comment"), "{text}");
        assert!(text.contains("# keep authors comment"), "{text}");
        assert!(text.contains("# keep ids comment"), "{text}");
        assert!(text.contains("unknown = 42 # also keep"), "{text}");
        assert!(
            text.contains("unknown_nested = 'yes' # keep nested unknown"),
            "{text}"
        );
        assert!(!text.contains("journal ="), "{text}");
        assert!(
            text.contains("title = \"New\"") || text.contains("title = 'New'"),
            "{text}"
        );
        assert!(text.contains("10.1000/new"), "{text}");
        let reloaded = Manifest::load(&path).unwrap();
        assert_eq!(reloaded.papers()["One"], refreshed.record);
    }

    #[test]
    fn force_equal_metadata_is_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cita.toml");
        let mut manifest = Manifest::create(&path).unwrap();
        let paper = resolved("One", 1);
        manifest.add(paper.clone(), None, false).unwrap();
        let before = fs::read_to_string(&path).unwrap();
        assert_eq!(
            manifest.add(paper, None, true).unwrap(),
            AddOutcome::Existing("One".into())
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn explicit_key_rejects_renaming_an_existing_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cita.toml");
        let mut manifest = Manifest::create(&path).unwrap();
        manifest.add(resolved("One", 1), None, false).unwrap();
        let mut refreshed = resolved("Other", 1);
        refreshed.record.title = "Changed".into();
        for force in [false, true] {
            assert!(matches!(
                manifest.add(refreshed.clone(), Some("Other"), force),
                Err(Error::CannotRename { existing }) if existing == "One"
            ));
        }
        assert_eq!(
            manifest.add(refreshed, Some("One"), false).unwrap(),
            AddOutcome::Existing("One".into())
        );
        assert_eq!(manifest.papers()["One"].title, "Paper 1");
    }

    #[test]
    fn force_creates_and_removes_publication_tables() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cita.toml");
        let mut manifest = Manifest::create(&path).unwrap();
        manifest.add(resolved("One", 1), None, false).unwrap();

        let mut with_publication = resolved("One", 1);
        with_publication.record.publication = Some(Publication {
            journal: Some("Journal".into()),
            year: Some(2025),
            ..Publication::default()
        });
        assert_eq!(
            manifest.add(with_publication.clone(), None, true).unwrap(),
            AddOutcome::Updated("One".into())
        );
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("[papers.One.publication]"), "{text}");
        assert!(text.contains("journal = \"Journal\""), "{text}");
        assert_eq!(
            Manifest::load(&path).unwrap().papers()["One"],
            with_publication.record
        );

        let without_publication = resolved("One", 1);
        assert_eq!(
            manifest
                .add(without_publication.clone(), None, true)
                .unwrap(),
            AddOutcome::Updated("One".into())
        );
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("[papers.One.publication]"), "{text}");
        assert_eq!(
            Manifest::load(&path).unwrap().papers()["One"],
            without_publication.record
        );
    }

    #[test]
    fn force_removes_empty_arrays_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cita.toml");
        let mut initial = resolved("One", 1);
        initial.record.authors = vec!["Author".into()];
        initial.record.collaborations = vec!["Collaboration".into()];
        let mut manifest = Manifest::create(&path).unwrap();
        manifest.add(initial, None, false).unwrap();

        let refreshed = resolved("One", 1);
        assert_eq!(
            manifest.add(refreshed.clone(), None, true).unwrap(),
            AddOutcome::Updated("One".into())
        );
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("authors ="), "{text}");
        assert!(!text.contains("collaborations ="), "{text}");
        assert_eq!(
            Manifest::load(&path).unwrap().papers()["One"],
            refreshed.record
        );
    }

    #[test]
    fn successful_force_update_batches_update_every_paper() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cita.toml");
        let mut manifest = Manifest::create(&path).unwrap();
        manifest
            .add_batch(vec![resolved("One", 1), resolved("Two", 2)], false)
            .unwrap();

        let mut one = resolved("One", 1);
        one.record.title = "Updated One".into();
        let mut two = resolved("Two", 2);
        two.record.title = "Updated Two".into();
        assert_eq!(
            manifest
                .add_batch(vec![two.clone(), one.clone()], true)
                .unwrap(),
            vec![
                AddOutcome::Updated("Two".into()),
                AddOutcome::Updated("One".into())
            ]
        );
        let reloaded = Manifest::load(&path).unwrap();
        assert_eq!(reloaded.papers()["One"], one.record);
        assert_eq!(reloaded.papers()["Two"], two.record);
    }

    #[test]
    fn force_updates_are_all_or_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cita.toml");
        let mut manifest = Manifest::create(&path).unwrap();
        manifest
            .add_batch(vec![resolved("One", 1), resolved("Two", 2)], false)
            .unwrap();
        let before = fs::read_to_string(&path).unwrap();

        let mut update_one = resolved("One", 1);
        update_one.record.title = "Updated One".into();
        // New identity trying to take an occupied key → KeyConflict; prior update
        // in the batch must roll back.
        let mut steal_key = resolved("Steal", 3);
        steal_key.suggested_key = Some("Two".into());

        let error = manifest.add_batch(vec![update_one, steal_key], true);
        assert!(matches!(error, Err(Error::KeyConflict(_))));
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
        assert_eq!(manifest.papers()["One"].title, "Paper 1");
    }
}
