//! The loaded bibliography and its mutations.

use crate::{BIBLIOGRAPHY_FILE, Entry, Error, FIELD_PREFIX, PATH_ENV};
use bibi_bibliography::{scan_entries, strip_fields_with_prefix, validate_key};
use bibi_core::{Locator, Reference, normalize_arxiv, normalize_doi};
use bibi_inspire_client::InspireRecord;
use std::{
    collections::HashMap,
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

/// A local citation key paired with its projected semantic reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedReference {
    /// Local citation key.
    pub key: String,
    /// Source-neutral projected reference.
    pub reference: Reference,
}

/// Controls whether an add must use a key or may resolve an identity collision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeyRequest {
    /// Require this exact local citation key.
    Exact(String),
    /// Use this key unless its INSPIRE record id already exists under another key.
    Suggested(String),
}

impl KeyRequest {
    /// The requested key, whether exact or suggested.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Exact(key) | Self::Suggested(key) => key,
        }
    }

    fn into_string(self) -> String {
        match self {
            Self::Exact(key) | Self::Suggested(key) => key,
        }
    }
}

/// A validated entry waiting to be added under a requested key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingReference {
    /// Exact or suggested local key request.
    pub key: KeyRequest,
    /// The entry to store.
    pub entry: Entry,
}

/// Controls how [`Bibfile::add_batch`] resolves a collision.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ConflictPolicy {
    /// Leave the existing reference in place and report the incoming one as skipped.
    #[default]
    Skip,
    /// Replace the colliding existing reference(s) with the incoming one.
    Overwrite,
}

/// Result of adding one pending reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AddOutcome {
    /// A new reference was stored under this actual local key.
    Added(String),
    /// The same identity was already stored under this actual local key.
    Existing(String),
    /// An incoming reference collided with an existing one and was left out.
    Skipped {
        /// Local key the incoming reference requested.
        key: String,
        /// Existing local key it collides with (equal to `key` for a same-key clash).
        conflicting: String,
    },
    /// An incoming reference replaced a colliding existing reference.
    Overwritten {
        /// Local key the incoming reference now occupies.
        key: String,
        /// Every local key removed to make room for the incoming reference.
        replaced: Vec<String>,
    },
}

/// One problem found by [`Bibfile::check`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    /// Local citation key the problem belongs to, when it belongs to one.
    pub key: Option<String>,
    /// Human-readable description.
    pub message: String,
}

/// A loaded bibliography, held as the bytes between entries plus the entries.
///
/// Splitting the file this way is what makes byte-preservation structural
/// rather than careful: rendering concatenates the pieces back, so anything
/// bibi does not model — comments, `@string` directives, the author's spacing —
/// survives a rewrite because no code path ever looks at it.
#[derive(Clone, Debug)]
pub struct Bibfile {
    path: PathBuf,
    /// Text preceding each entry; `leading[i]` sits in front of `entries[i]`.
    leading: Vec<String>,
    entries: Vec<Entry>,
    /// Everything after the last entry.
    tail: String,
}

impl Bibfile {
    /// Resolve which bibliography to operate on.
    ///
    /// Explicit path first, then `BIBI_BIB`, then `references.bib` in the
    /// current directory. There is deliberately no walk up the tree: a `.bib`
    /// is not a project marker, and silently adopting a parent directory's
    /// bibliography is worse than asking for a path. A directory resolves to
    /// the default file name inside it.
    pub fn resolve(explicit: Option<&Path>, cwd: &Path) -> PathBuf {
        let candidate = match explicit {
            Some(path) => path.to_path_buf(),
            None => match env::var_os(PATH_ENV).filter(|value| !value.is_empty()) {
                Some(value) => PathBuf::from(value),
                None => cwd.join(BIBLIOGRAPHY_FILE),
            },
        };
        let candidate = if candidate.is_absolute() {
            candidate
        } else {
            cwd.join(candidate)
        };
        if candidate.is_dir() {
            return candidate.join(BIBLIOGRAPHY_FILE);
        }
        candidate
    }

    /// Read and scan a bibliography.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref().to_path_buf();
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::Missing(path));
            }
            Err(source) => return Err(Error::Read { path, source }),
        };
        Self::from_source(path, &source)
    }

    fn from_source(path: PathBuf, source: &str) -> Result<Self, Error> {
        let mut leading = Vec::new();
        let mut entries = Vec::new();
        let mut cursor = 0;
        for span in scan_entries(source)? {
            leading.push(source[cursor..span.span.start].to_owned());
            entries.push(Entry::new(span.key, source[span.span.clone()].to_owned()));
            cursor = span.span.end;
        }
        Ok(Self {
            path,
            leading,
            entries,
            tail: source[cursor..].to_owned(),
        })
    }

    /// Path this bibliography was loaded from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every entry in file order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Local citation keys in file order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.key.as_str())
    }

    /// The complete raw entry stored under `key`.
    pub fn raw(&self, key: &str) -> Option<&str> {
        self.index_of(key)
            .map(|at| self.entries[at].bibtex.as_str())
    }

    fn index_of(&self, key: &str) -> Option<usize> {
        self.entries.iter().position(|entry| entry.key == key)
    }

    /// Render the file exactly as it should be written.
    ///
    /// With no mutations this reproduces the loaded bytes exactly.
    pub fn render(&self) -> String {
        let mut output = String::with_capacity(self.tail.len() + 128 * self.entries.len());
        for (gap, entry) in self.leading.iter().zip(&self.entries) {
            output.push_str(gap);
            output.push_str(&entry.bibtex);
        }
        output.push_str(&self.tail);
        output
    }

    /// Render a bare copy with every tool-owned field removed.
    pub fn render_bare(&self) -> Result<String, Error> {
        Ok(strip_fields_with_prefix(&self.render(), FIELD_PREFIX)?)
    }

    /// Write the bibliography back to its own path.
    pub fn write(&self) -> Result<(), Error> {
        atomic_write(&self.path, self.render().as_bytes())
    }

    /// Project every entry, in file order.
    pub fn projected(&self) -> Result<Vec<ProjectedReference>, Error> {
        self.entries
            .iter()
            .map(|entry| {
                Ok(ProjectedReference {
                    key: entry.key.clone(),
                    reference: entry.project()?,
                })
            })
            .collect()
    }

    /// Find by exact local key, then provider id, normalized DOI, or arXiv id.
    pub fn find(&self, selector: &str) -> Result<Option<ProjectedReference>, Error> {
        if let Some(at) = self.index_of(selector) {
            return Ok(Some(ProjectedReference {
                key: selector.to_owned(),
                reference: self.entries[at].project()?,
            }));
        }
        let Ok(locator) = selector.parse::<Locator>() else {
            return Ok(None);
        };
        for entry in &self.entries {
            let reference = entry.project()?;
            let matches = match &locator {
                Locator::Inspire(id) => reference
                    .identifiers
                    .providers
                    .get("inspire")
                    .is_some_and(|ids| ids.iter().any(|value| value == &id.to_string())),
                Locator::Doi(id) => reference.identifiers.dois.contains(&normalize_doi(id)),
                Locator::Arxiv(id) => reference.identifiers.arxiv.contains(&normalize_arxiv(id)),
            };
            if matches {
                return Ok(Some(ProjectedReference {
                    key: entry.key.clone(),
                    reference,
                }));
            }
        }
        Ok(None)
    }

    /// Validate the whole file: keys, projections, and identity uniqueness.
    ///
    /// Returns every problem rather than the first, because the file is
    /// hand-edited and a single report should be actionable in one pass.
    pub fn check(&self) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut seen: HashMap<String, String> = HashMap::new();
        for entry in &self.entries {
            if let Err(error) = validate_key(&entry.key) {
                diagnostics.push(Diagnostic {
                    key: Some(entry.key.clone()),
                    message: error.to_string(),
                });
            }
            let reference = match entry.project() {
                Ok(reference) => reference,
                Err(error) => {
                    diagnostics.push(Diagnostic {
                        key: Some(entry.key.clone()),
                        message: error.to_string(),
                    });
                    continue;
                }
            };
            for identity in identities(&reference) {
                if let Some(other) = seen.get(&identity) {
                    diagnostics.push(Diagnostic {
                        key: Some(entry.key.clone()),
                        message: format!("shares the identity {identity} with `{other}`"),
                    });
                } else {
                    seen.insert(identity, entry.key.clone());
                }
            }
        }
        diagnostics
    }

    /// Entries bibi refreshes from INSPIRE, as `(key, record id)`.
    ///
    /// Frozen entries are excluded: the marker means the user owns them.
    pub fn managed(&self) -> Vec<(String, u64)> {
        self.entries
            .iter()
            .filter(|entry| !entry.is_frozen())
            .filter_map(|entry| Some((entry.key.clone(), entry.inspire_record_id()?)))
            .collect()
    }

    /// Entries with no record id but an identity INSPIRE could resolve.
    pub fn unmanaged(&self) -> Result<Vec<(String, Locator)>, Error> {
        let mut pending = Vec::new();
        for entry in &self.entries {
            if entry.is_frozen() || entry.is_managed() {
                continue;
            }
            let reference = entry.project()?;
            let locator = reference
                .identifiers
                .arxiv
                .first()
                .map(|id| Locator::Arxiv(id.clone()))
                .or_else(|| {
                    reference
                        .identifiers
                        .dois
                        .first()
                        .map(|id| Locator::Doi(id.clone()))
                });
            if let Some(locator) = locator {
                pending.push((entry.key.clone(), locator));
            }
        }
        Ok(pending)
    }

    /// Validate and add a complete batch, appending new entries in order.
    ///
    /// Collisions never abort the batch: under [`ConflictPolicy::Skip`] a
    /// colliding incoming entry is reported and left out, and under
    /// [`ConflictPolicy::Overwrite`] it replaces what it collides with. Only
    /// malformed input aborts, leaving the file untouched.
    pub fn add_batch(
        &mut self,
        pending: Vec<PendingReference>,
        policy: ConflictPolicy,
    ) -> Result<Vec<AddOutcome>, Error> {
        let mut candidate = self.clone();
        let mut outcomes = Vec::with_capacity(pending.len());
        for item in pending {
            validate_key(item.key.as_str())?;
            let forced = match candidate.same_record(&item, policy)? {
                Some(Forced::AlreadyStored(key)) => {
                    outcomes.push(AddOutcome::Existing(key));
                    continue;
                }
                Some(Forced::Replaces(key)) => Some(key),
                None => None,
            };
            let key = item.key.into_string();
            let entry = item.entry;
            if candidate
                .raw(&key)
                .is_some_and(|existing| existing == entry.bibtex)
            {
                outcomes.push(AddOutcome::Existing(key));
                continue;
            }
            let mut colliding = Vec::new();
            if candidate.index_of(&key).is_some() {
                colliding.push(key.clone());
            }
            if let Some(existing) = forced
                && existing != key
                && !colliding.contains(&existing)
            {
                colliding.push(existing);
            }
            let incoming = identities(&entry.project()?);
            for other in &candidate.entries {
                if other.key == key || colliding.contains(&other.key) {
                    continue;
                }
                if identities(&other.project()?)
                    .iter()
                    .any(|identity| incoming.contains(identity))
                {
                    colliding.push(other.key.clone());
                }
            }
            match (colliding.is_empty(), policy) {
                (true, _) => {
                    candidate.append(key.clone(), entry)?;
                    outcomes.push(AddOutcome::Added(key));
                }
                (false, ConflictPolicy::Skip) => {
                    let conflicting = colliding.into_iter().next().expect("collision present");
                    outcomes.push(AddOutcome::Skipped { key, conflicting });
                }
                (false, ConflictPolicy::Overwrite) => {
                    for removed in &colliding {
                        candidate.drop_key(removed);
                    }
                    candidate.append(key.clone(), entry)?;
                    outcomes.push(AddOutcome::Overwritten {
                        key,
                        replaced: colliding,
                    });
                }
            }
        }
        candidate.ensure_valid()?;
        *self = candidate;
        Ok(outcomes)
    }

    /// What an incoming record's already-stored twin, if any, forces.
    ///
    /// A record id is the strongest identity there is, so finding the same one
    /// under another key is decided here rather than left to the generic
    /// identity sweep: a suggested key defers to the stored entry, and an exact
    /// key demands an explicit overwrite before it may move a stored record.
    fn same_record(
        &self,
        item: &PendingReference,
        policy: ConflictPolicy,
    ) -> Result<Option<Forced>, Error> {
        let Some(record_id) = item.entry.inspire_record_id() else {
            return Ok(None);
        };
        let Some(existing) = self
            .entries
            .iter()
            .find(|entry| entry.inspire_record_id() == Some(record_id))
            .map(|entry| entry.key.clone())
        else {
            return Ok(None);
        };
        match &item.key {
            KeyRequest::Suggested(_) => Ok(Some(Forced::AlreadyStored(existing))),
            KeyRequest::Exact(requested) if requested == &existing => {
                Ok(Some(Forced::AlreadyStored(existing)))
            }
            KeyRequest::Exact(requested) => match policy {
                ConflictPolicy::Skip => Err(Error::CannotRename {
                    existing,
                    requested: requested.clone(),
                }),
                ConflictPolicy::Overwrite => Ok(Some(Forced::Replaces(existing))),
            },
        }
    }

    /// Resolve and remove all supplied selectors together.
    pub fn remove_batch(&mut self, selectors: &[String]) -> Result<Vec<ProjectedReference>, Error> {
        let mut removed = Vec::new();
        for selector in selectors {
            let found = self
                .find(selector)?
                .ok_or_else(|| Error::NoMatch(selector.clone()))?;
            if !removed
                .iter()
                .any(|item: &ProjectedReference| item.key == found.key)
            {
                removed.push(found);
            }
        }
        for item in &removed {
            self.drop_key(&item.key);
        }
        Ok(removed)
    }

    /// Change one entry's local citation key.
    pub fn rekey(&mut self, selector: &str, new_key: &str) -> Result<String, Error> {
        validate_key(new_key)?;
        let found = self
            .find(selector)?
            .ok_or_else(|| Error::NoMatch(selector.to_owned()))?;
        if found.key == new_key {
            return Ok(found.key);
        }
        if self.index_of(new_key).is_some() {
            return Err(Error::KeyInUse(new_key.to_owned()));
        }
        let at = self
            .index_of(&found.key)
            .expect("find returned a stored key");
        self.entries[at].rekey(new_key)?;
        Ok(found.key)
    }

    /// Apply refreshed INSPIRE records to the entries that requested them.
    ///
    /// Returns the local keys whose content actually changed. An entry whose
    /// stored timestamp matches the returned one is left completely alone, so a
    /// sync that learns nothing new touches no bytes and produces no diff.
    ///
    /// Every managed entry must be explained by exactly one record and every
    /// record by exactly one entry: a record id is the strongest identity bibi
    /// has, so an unexplained one on either side means the file and the
    /// provider disagree about what is stored, which is not something to paper
    /// over by writing something plausible.
    pub fn apply_records(&mut self, records: &[InspireRecord]) -> Result<Vec<String>, Error> {
        let mut by_id: HashMap<u64, &InspireRecord> = HashMap::new();
        for record in records {
            if by_id.insert(record.record_id, record).is_some() {
                return Err(Error::UnexpectedRecord(record.record_id));
            }
        }
        let mut claimed: HashMap<u64, String> = HashMap::new();
        let mut changed = Vec::new();
        for at in 0..self.entries.len() {
            if self.entries[at].is_frozen() {
                continue;
            }
            let Some(record_id) = self.entries[at].inspire_record_id() else {
                continue;
            };
            let key = self.entries[at].key.clone();
            if let Some(other) = claimed.insert(record_id, key.clone()) {
                return Err(Error::DuplicateRecord {
                    key,
                    other,
                    record_id,
                });
            }
            let record = by_id.remove(&record_id).ok_or(Error::MissingRecord {
                key: key.clone(),
                record_id,
            })?;
            if self.entries[at].inspire_updated().as_deref() == Some(record.updated.as_str()) {
                continue;
            }
            self.entries[at].refresh(record)?;
            changed.push(key);
        }
        if let Some(record_id) = by_id.keys().next() {
            return Err(Error::UnexpectedRecord(*record_id));
        }
        Ok(changed)
    }

    /// Attach a resolved INSPIRE record to an entry bibi did not manage.
    ///
    /// Refuses when another entry already claims the record, because two local
    /// keys tracking one upstream paper is a duplicate the user has to settle:
    /// `import` dedupes on DOI and arXiv id, so it cannot catch a pair that
    /// only turns out to be the same paper once INSPIRE resolves both.
    pub fn adopt(&mut self, key: &str, record: &InspireRecord) -> Result<(), Error> {
        if let Some(other) = self
            .entries
            .iter()
            .find(|entry| entry.key != key && entry.inspire_record_id() == Some(record.record_id))
        {
            return Err(Error::DuplicateRecord {
                key: key.to_owned(),
                other: other.key.clone(),
                record_id: record.record_id,
            });
        }
        let at = self
            .index_of(key)
            .ok_or_else(|| Error::NoMatch(key.into()))?;
        self.entries[at].adopt(record)
    }

    /// Append an entry at the end of the file, under `key`.
    fn append(&mut self, key: String, mut entry: Entry) -> Result<(), Error> {
        if entry.key != key {
            entry.rekey(&key)?;
        }
        // New entries go last: the user owns the ordering, so an add should
        // read as one appended block in the diff rather than a resort.
        self.leading.push(if self.entries.is_empty() {
            String::new()
        } else {
            "\n\n".to_owned()
        });
        self.entries.push(entry);
        if !self.tail.ends_with('\n') {
            self.tail.push('\n');
        }
        Ok(())
    }

    fn drop_key(&mut self, key: &str) {
        let Some(at) = self.index_of(key) else {
            return;
        };
        self.entries.remove(at);
        // Take the gap that separated this entry from its neighbour. For the
        // first entry that is the gap *after* it, so a leading file comment is
        // not dropped along with the entry it happened to precede.
        self.leading.remove(if at == 0 && self.leading.len() > 1 {
            1
        } else {
            at
        });
    }

    fn ensure_valid(&self) -> Result<(), Error> {
        let mut seen: HashMap<String, String> = HashMap::new();
        for entry in &self.entries {
            validate_key(&entry.key)?;
            for identity in identities(&entry.project()?) {
                if let Some(other) = seen.insert(identity.clone(), entry.key.clone()) {
                    return Err(Error::DuplicateIdentity {
                        key: entry.key.clone(),
                        other,
                        identity,
                    });
                }
            }
        }
        Ok(())
    }
}

/// What an incoming record's already-stored twin forces.
enum Forced {
    /// The record is already here under this key; the add is a no-op.
    AlreadyStored(String),
    /// The record must move out of this key to make room for the incoming one.
    Replaces(String),
}

/// Normalized identity strings a reference claims.
fn identities(reference: &Reference) -> Vec<String> {
    let mut values = Vec::new();
    for doi in &reference.identifiers.dois {
        values.push(format!("doi:{}", normalize_doi(doi)));
    }
    for arxiv in &reference.identifiers.arxiv {
        values.push(format!("arxiv:{}", normalize_arxiv(arxiv)));
    }
    for (provider, ids) in &reference.identifiers.providers {
        for id in ids {
            values.push(format!("{provider}:{id}"));
        }
    }
    values
}

/// Replace a file's contents in one step, or leave the old contents in place.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let directory = path.parent().unwrap_or(Path::new("."));
    let write = |source| Error::Write {
        path: path.to_path_buf(),
        source,
    };
    let mut file = NamedTempFile::new_in(directory).map_err(write)?;
    file.write_all(bytes).map_err(write)?;
    file.as_file().sync_all().map_err(write)?;
    file.persist(path).map_err(|error| write(error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(source: &str) -> Bibfile {
        Bibfile::from_source(PathBuf::from("references.bib"), source).unwrap()
    }

    fn entry(key: &str, title: &str, extra: &str) -> String {
        format!("@article{{{key},\n  title = {{{title}}},\n{extra}}}")
    }

    /// Every shape a hand-maintained bibliography can plausibly take.
    const CORPUS: &[&str] = &[
        "",
        "\n",
        "   \n\n",
        "@article{A, title = {One line}}",
        "@article{A,\n  title = {T},\n}\n",
        "@article{A,\n  title = {T}\n}",
        "@article{A,\n  title = {T},\n}\n\n\n\n@article{B,\n  title = {U},\n}\n",
        "% a leading note\n@article{A,\n  title = {T},\n}\n",
        "@article{A,\n  title = {T},\n}\n% between\n@article{B,\n  title = {U},\n}\n",
        "@string{j = {Journal}}\n\n@article{A,\n  title = {T},\n}\n",
        "@article{A,\n  title = {T},\n}\n\n% a trailing note\n",
        "@article{A,\r\n  title = {T},\r\n}\r\n",
        "@article{A,\n\ttitle = {Tabs},\n}\n",
        "@article{Uni,\n  title = {Ünïcødé ✓},\n}\n",
    ];

    /// The governing invariant: loading and writing back without mutating must
    /// reproduce the file exactly, whatever bibi does or does not model in it.
    #[test]
    fn round_trip_is_byte_identical() {
        for source in CORPUS {
            assert_eq!(
                load(source).render(),
                *source,
                "round trip changed {source:?}"
            );
        }
    }

    #[test]
    fn appending_leaves_every_other_byte_alone() {
        let source = "% keep\n@article{A,\n  title = {T},\n}\n";
        let mut file = load(source);
        file.append("B".into(), Entry::new("B".into(), entry("B", "U", "")))
            .unwrap();
        assert_eq!(
            file.render(),
            "% keep\n@article{A,\n  title = {T},\n}\n\n@article{B,\n  title = {U},\n}\n"
        );
    }

    #[test]
    fn appending_to_an_empty_file_adds_one_trailing_newline() {
        let mut file = load("");
        file.append("A".into(), Entry::new("A".into(), entry("A", "T", "")))
            .unwrap();
        assert_eq!(file.render(), "@article{A,\n  title = {T},\n}\n");
    }

    /// Removing the first entry must not take a file header with it.
    #[test]
    fn removing_preserves_surrounding_content() {
        let source = "% header\n@article{A,\n  title = {T},\n}\n\n@article{B,\n  title = {U},\n}\n";
        let mut file = load(source);
        file.drop_key("A");
        assert_eq!(file.render(), "% header\n@article{B,\n  title = {U},\n}\n");

        let mut file = load(source);
        file.drop_key("B");
        assert_eq!(file.render(), "% header\n@article{A,\n  title = {T},\n}\n");
    }

    #[test]
    fn rekey_changes_only_the_key_token() {
        let mut file = load("@article{Old,\n  title = {T},\n}\n");
        assert_eq!(file.rekey("Old", "New").unwrap(), "Old");
        assert_eq!(file.render(), "@article{New,\n  title = {T},\n}\n");
        assert!(matches!(file.rekey("New", "New"), Ok(key) if key == "New"));
        assert!(matches!(file.rekey("missing", "X"), Err(Error::NoMatch(_))));
    }

    #[test]
    fn rekey_refuses_a_key_already_in_use() {
        let mut file = load(&format!(
            "{}\n\n{}\n",
            entry("A", "T", ""),
            entry("B", "U", "")
        ));
        assert!(matches!(file.rekey("A", "B"), Err(Error::KeyInUse(_))));
    }

    #[test]
    fn tool_fields_drive_managed_frozen_and_identity() {
        let source = format!(
            "{}\n\n{}\n\n{}\n",
            entry("Managed", "T", "  x-bibi-inspire-id = {1229104},\n"),
            entry(
                "Frozen",
                "U",
                "  x-bibi-inspire-id = {51188},\n  x-bibi-frozen = {true},\n"
            ),
            entry("Plain", "V", "  doi = {10.1/PLAIN},\n"),
        );
        let file = load(&source);
        assert_eq!(file.managed(), vec![("Managed".to_owned(), 1229104)]);
        let unmanaged = file.unmanaged().unwrap();
        assert_eq!(unmanaged.len(), 1);
        assert_eq!(unmanaged[0].0, "Plain");

        // The curated id reaches the projection as a provider identity.
        let found = file.find("inspire:1229104").unwrap().unwrap();
        assert_eq!(found.key, "Managed");
    }

    #[test]
    fn a_malformed_record_id_leaves_the_entry_unmanaged() {
        let file = load(&entry("A", "T", "  x-bibi-inspire-id = {oops},\n"));
        assert!(file.managed().is_empty());
        assert!(!file.entries()[0].is_managed());
        // Still readable rather than fatal.
        assert_eq!(file.projected().unwrap().len(), 1);
    }

    #[test]
    fn find_prefers_an_exact_key_then_falls_back_to_identity() {
        let source = format!(
            "{}\n\n{}\n",
            entry("First", "T", "  doi = {10.1/ALPHA},\n"),
            entry("Second", "U", "  eprint = {2001.00001},\n"),
        );
        let file = load(&source);
        assert_eq!(file.find("First").unwrap().unwrap().key, "First");
        assert_eq!(file.find("doi:10.1/alpha").unwrap().unwrap().key, "First");
        assert_eq!(file.find("2001.00001").unwrap().unwrap().key, "Second");
        assert!(file.find("nonsense").unwrap().is_none());
    }

    #[test]
    fn adding_skips_or_overwrites_a_colliding_identity() {
        let source = format!("{}\n", entry("Existing", "T", "  doi = {10.1/SAME},\n"));
        let incoming = || PendingReference {
            key: KeyRequest::Suggested("Incoming".into()),
            entry: Entry::new(
                "Incoming".into(),
                entry("Incoming", "Different", "  doi = {10.1/same},\n"),
            ),
        };

        let mut file = load(&source);
        let outcomes = file
            .add_batch(vec![incoming()], ConflictPolicy::Skip)
            .unwrap();
        assert_eq!(
            outcomes,
            vec![AddOutcome::Skipped {
                key: "Incoming".into(),
                conflicting: "Existing".into()
            }]
        );
        assert_eq!(file.render(), source);

        let mut file = load(&source);
        let outcomes = file
            .add_batch(vec![incoming()], ConflictPolicy::Overwrite)
            .unwrap();
        assert_eq!(
            outcomes,
            vec![AddOutcome::Overwritten {
                key: "Incoming".into(),
                replaced: vec!["Existing".into()]
            }]
        );
        assert_eq!(file.keys().collect::<Vec<_>>(), ["Incoming"]);
    }

    #[test]
    fn adding_the_same_record_under_a_suggested_key_is_a_no_op() {
        let source = format!(
            "{}\n",
            entry("Stored", "T", "  x-bibi-inspire-id = {1229104},\n")
        );
        let mut file = load(&source);
        let outcomes = file
            .add_batch(
                vec![PendingReference {
                    key: KeyRequest::Suggested("Other".into()),
                    entry: Entry::new(
                        "Other".into(),
                        entry("Other", "T", "  x-bibi-inspire-id = {1229104},\n"),
                    ),
                }],
                ConflictPolicy::Skip,
            )
            .unwrap();
        assert_eq!(outcomes, vec![AddOutcome::Existing("Stored".into())]);
        assert_eq!(file.render(), source);
    }

    #[test]
    fn an_exact_key_cannot_move_a_stored_record_without_overwrite() {
        let mut file = load(&format!(
            "{}\n",
            entry("Stored", "T", "  x-bibi-inspire-id = {1229104},\n")
        ));
        let pending = PendingReference {
            key: KeyRequest::Exact("Renamed".into()),
            entry: Entry::new(
                "Renamed".into(),
                entry("Renamed", "T", "  x-bibi-inspire-id = {1229104},\n"),
            ),
        };
        assert!(matches!(
            file.add_batch(vec![pending], ConflictPolicy::Skip),
            Err(Error::CannotRename { .. })
        ));
    }

    #[test]
    fn check_reports_every_duplicate_identity_at_once() {
        let file = load(&format!(
            "{}\n\n{}\n\n{}\n",
            entry("A", "T", "  doi = {10.1/X},\n"),
            entry("B", "U", "  doi = {10.1/x},\n"),
            entry("C", "V", "  eprint = {2001.00001},\n"),
        ));
        let diagnostics = file.check();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].key.as_deref(), Some("B"));
        assert!(diagnostics[0].message.contains("doi:10.1/x"));
    }

    #[test]
    fn bare_rendering_drops_only_tool_fields() {
        let source = format!(
            "% note\n{}\n",
            entry(
                "A",
                "T",
                "  doi = {10.1/X},\n  x-bibi-inspire-id = {1229104},\n  x-bibi-doi = {10.1/x},\n"
            )
        );
        let bare = load(&source).render_bare().unwrap();
        assert!(!bare.contains("x-bibi-"), "{bare}");
        assert!(bare.contains("% note"), "{bare}");
        assert!(bare.contains("doi = {10.1/X}"), "{bare}");
    }

    /// An explicit path short-circuits before the environment is consulted, so
    /// these cases are independent of process-global state. The `BIBI_BIB` and
    /// bare-default branches are covered by the CLI suite, which controls the
    /// environment per subprocess instead of racing other tests for it.
    #[test]
    fn an_explicit_path_resolves_against_the_caller() {
        let cwd = Path::new("/work");
        assert_eq!(
            Bibfile::resolve(Some(Path::new("other.bib")), cwd),
            PathBuf::from("/work/other.bib")
        );
        assert_eq!(
            Bibfile::resolve(Some(Path::new("/abs/other.bib")), cwd),
            PathBuf::from("/abs/other.bib")
        );
    }

    #[test]
    fn a_directory_resolves_to_the_default_file_inside_it() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            Bibfile::resolve(Some(directory.path()), Path::new("/work")),
            directory.path().join(BIBLIOGRAPHY_FILE)
        );
    }

    #[test]
    fn a_missing_file_is_reported_rather_than_created() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("references.bib");
        assert!(matches!(Bibfile::load(&path), Err(Error::Missing(_))));
        assert!(!path.exists());
    }

    #[test]
    fn writing_round_trips_through_the_filesystem() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("references.bib");
        let source = "% note\n@article{A,\n  title = {T},\n}\n";
        fs::write(&path, source).unwrap();
        let file = Bibfile::load(&path).unwrap();
        file.write().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), source);
    }
}
