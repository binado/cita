//! Command flows over fake providers and temporary manifests.

use bibi_application::{
    AddFileRequest, AddKind, AddRequest, InputSource, ListRequest, Services, add_file,
    add_locators, domain::CitationKey, domain::Manifest, domain::ManifestStore, list, remove,
    rename, show, to_json,
};
use bibi_core::{ArxivId, Doi};
use bibi_provider::{
    Provider,
    testing::{FakeProvider, provider_record, providers},
};
use std::{path::PathBuf, sync::Arc};
use tempfile::TempDir;

/// A project directory with an injected provider roster.
struct Project {
    directory: TempDir,
    services: Services,
}

impl Project {
    fn with(remote: Arc<FakeProvider>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let services = Services::new(Arc::new(providers(remote)));
        Self {
            directory,
            services,
        }
    }

    fn new(network: Arc<FakeProvider>) -> Self {
        Self::with(network)
    }

    fn store(&self) -> ManifestStore {
        ManifestStore::new(self.directory.path().join("bibi.toml"))
    }

    /// The manifest as the commands see it, loaded fresh.
    fn loaded(&self) -> Manifest {
        self.store().load().unwrap().manifest
    }

    fn file(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.directory.path().join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn manifest(&self) -> String {
        std::fs::read_to_string(self.store().path()).unwrap()
    }
}

fn network() -> Arc<FakeProvider> {
    Arc::new(
        FakeProvider::new("inspire")
            .with_record(
                "arxiv:1207.7214",
                provider_record("inspire", "1124337", "Aad:2012tfa", "Observation"),
            )
            .with_record(
                "doi:10.1/other",
                provider_record("inspire", "999", "Other:2020", "Another paper"),
            ),
    )
}

async fn add(
    project: &Project,
    locators: &[&str],
    request: AddRequest,
) -> bibi_application::AddReport {
    let locators = locators
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    add_locators(&project.services, &project.store(), &locators, &request)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_first_add_creates_the_manifest_without_an_init() {
    let project = Project::new(network());
    assert!(!project.store().exists());
    let report = add(&project, &["1207.7214"], AddRequest::default()).await;
    assert!(report.committed);
    assert!(project.store().exists());
    // The provider's own texkey is adopted as the local key.
    assert_eq!(report.items.successes[0].key.as_str(), "Aad:2012tfa");
    assert_eq!(report.items.successes[0].kind, AddKind::Added);
}

#[tokio::test]
async fn a_read_command_reports_a_missing_manifest_rather_than_conjuring_one() {
    let project = Project::new(network());
    // Listing is pure over a loaded manifest, so the refusal to conjure one
    // belongs to the load the command performs first.
    assert!(project.store().load().is_err());
    assert!(show(&project.store(), "Anything").is_err());
    assert!(!project.store().exists());
}

#[tokio::test]
async fn an_explicit_key_overrides_the_adopted_one() {
    let project = Project::new(network());
    let report = add(
        &project,
        &["1207.7214"],
        AddRequest {
            key: Some(CitationKey::new("Higgs").unwrap()),
            ..AddRequest::default()
        },
    )
    .await;
    assert_eq!(report.items.successes[0].key.as_str(), "Higgs");
    assert!(project.manifest().contains("@article{Aad:2012tfa,"));
}

#[tokio::test]
async fn an_explicit_key_accepts_exactly_one_locator() {
    let project = Project::new(network());
    let error = add_locators(
        &project.services,
        &project.store(),
        &["1207.7214".to_owned(), "10.1/other".to_owned()],
        &AddRequest {
            key: Some(CitationKey::new("Higgs").unwrap()),
            ..AddRequest::default()
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("exactly one locator"));
}

#[tokio::test]
async fn a_duplicate_is_skipped_and_still_reports_its_key() {
    let project = Project::new(network());
    add(&project, &["1207.7214"], AddRequest::default()).await;
    let report = add(&project, &["1207.7214"], AddRequest::default()).await;

    assert!(report.items.successes.is_empty());
    assert_eq!(report.items.skipped.len(), 1);
    assert!(!report.items.has_failures(), "a skip is not a failure");
    assert!(!report.committed, "nothing changed, so nothing was written");
    assert_eq!(
        report.items.skipped[0].key.as_ref().unwrap().as_str(),
        "Aad:2012tfa"
    );
}

#[tokio::test]
async fn a_key_collision_fails_an_add_and_names_the_escape_hatch() {
    let project = Project::new(network());
    add(
        &project,
        &["10.1/other"],
        AddRequest {
            key: Some(CitationKey::new("Aad:2012tfa").unwrap()),
            ..AddRequest::default()
        },
    )
    .await;
    // A different work whose adopted texkey is already taken.
    let report = add(&project, &["1207.7214"], AddRequest::default()).await;
    assert_eq!(report.items.failures.len(), 1);
    assert!(report.items.failures[0].message.contains("--key"));
}

#[tokio::test]
async fn overwrite_replaces_provider_data_and_preserves_id_and_key() {
    let project = Project::new(network());
    add(
        &project,
        &["1207.7214"],
        AddRequest {
            key: Some(CitationKey::new("Higgs").unwrap()),
            ..AddRequest::default()
        },
    )
    .await;
    let before = list(&project.loaded(), &ListRequest::default());

    let updated = Arc::new(FakeProvider::new("inspire").with_record(
        "arxiv:1207.7214",
        provider_record("inspire", "1124337", "Aad:2012tfa", "Corrected title"),
    ));
    let project = Project {
        directory: project.directory,
        services: Services::new(Arc::new(providers(updated))),
    };
    let report = add(
        &project,
        &["1207.7214"],
        AddRequest {
            overwrite: true,
            ..AddRequest::default()
        },
    )
    .await;

    assert_eq!(report.items.successes[0].kind, AddKind::Overwritten);
    let after = list(&project.loaded(), &ListRequest::default());
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].id, before[0].id, "the bibi id is immutable");
    assert_eq!(after[0].key, before[0].key, "the local key is the user's");
    assert_eq!(after[0].description.title, "Corrected title");
}

#[tokio::test]
async fn an_overwrite_with_two_possible_targets_is_refused() {
    let project = Project::new(network());
    add(&project, &["1207.7214"], AddRequest::default()).await;
    add(&project, &["10.1/other"], AddRequest::default()).await;
    // Deduplication identifies one record while `--key` names another.
    let report = add(
        &project,
        &["1207.7214"],
        AddRequest {
            key: Some(CitationKey::new("Other:2020").unwrap()),
            overwrite: true,
            ..AddRequest::default()
        },
    )
    .await;
    assert_eq!(report.items.failures.len(), 1);
    assert!(
        report.items.failures[0]
            .message
            .contains("different record")
    );
}

#[tokio::test]
async fn a_partial_batch_commits_its_successes_and_reports_the_rest() {
    let project = Project::new(network());
    let report = add(
        &project,
        &["1207.7214", "10.1/nobody-has-this", "not a locator at all"],
        AddRequest::default(),
    )
    .await;

    assert_eq!(report.items.successes.len(), 1);
    assert_eq!(report.items.failures.len(), 2);
    assert!(report.committed, "the one success is not discarded");
    assert_eq!(list(&project.loaded(), &ListRequest::default()).len(), 1);
}

#[tokio::test]
async fn a_dry_run_reports_without_writing() {
    let project = Project::new(network());
    let report = add(
        &project,
        &["1207.7214"],
        AddRequest {
            dry_run: true,
            ..AddRequest::default()
        },
    )
    .await;
    assert_eq!(report.items.successes.len(), 1);
    assert!(!report.committed);
    assert!(!project.store().exists());
}

#[tokio::test]
async fn importing_resolves_what_it_can_and_fails_entries_without_identifiers() {
    let project = Project::new(network());
    let path = project.file(
        "colleague.bib",
        "@article{TheirKey:2012,\n  title = {Their formatting},\n  eprint = {1207.7214}\n}\n\n@unpublished{Notes:2026,\n  title = {Lecture notes},\n  author = {Roe, Richard}\n}\n",
    );
    let report = add_file(
        &project.services,
        &project.store(),
        &AddFileRequest {
            source: InputSource::Path(path),
            provider: Provider::Inspire,
            overwrite: false,
            dry_run: false,
        },
    )
    .await
    .unwrap();

    assert_eq!(report.items.successes.len(), 1);
    assert_eq!(report.items.failures.len(), 1);
    let records = list(&project.loaded(), &ListRequest::default());
    let resolved = records
        .iter()
        .find(|record| record.key.as_str() == "TheirKey:2012")
        .unwrap();
    // Resolved: the provider's payload, but the colleague's citation key.
    assert_eq!(resolved.provenance.provider.as_str(), "inspire");
    assert!(resolved.payload.source().contains("@article{Aad:2012tfa,"));
    assert_eq!(resolved.description.title, "Observation");

    assert!(
        records
            .iter()
            .all(|record| record.key.as_str() != "Notes:2026")
    );
}

#[tokio::test]
async fn importing_refuses_identifiers_that_resolve_to_different_records() {
    let mut by_doi = provider_record("inspire", "1", "ByDoi", "DOI paper");
    by_doi.identifiers.doi = Some(Doi::new("10.1/doi-paper").unwrap());
    let mut by_arxiv = provider_record("inspire", "2", "ByArxiv", "arXiv paper");
    by_arxiv.identifiers.arxiv = Some(ArxivId::new("1207.7214").unwrap());
    let network = Arc::new(
        FakeProvider::new("inspire")
            .with_record("doi:10.1/doi-paper", by_doi)
            .with_record("arxiv:1207.7214", by_arxiv),
    );
    let project = Project::new(network);
    let path = project.file(
        "conflicting.bib",
        "@article{Conflicting,title={Conflicting},doi={10.1/doi-paper},eprint={1207.7214}}\n",
    );

    let report = add_file(
        &project.services,
        &project.store(),
        &AddFileRequest {
            source: InputSource::Path(path),
            provider: Provider::Inspire,
            overwrite: false,
            dry_run: false,
        },
    )
    .await
    .unwrap();

    assert_eq!(report.items.failures.len(), 1);
    assert!(
        report.items.failures[0]
            .message
            .contains("different provider records")
    );
    assert!(!project.store().exists());
}

#[tokio::test]
async fn importing_accepts_two_identifiers_for_the_same_provider_record() {
    let mut record = provider_record("inspire", "1", "Same", "Same paper");
    record.identifiers.doi = Some(Doi::new("10.1/same").unwrap());
    record.identifiers.arxiv = Some(ArxivId::new("1207.7214").unwrap());
    let network = Arc::new(
        FakeProvider::new("inspire")
            .with_record("doi:10.1/same", record.clone())
            .with_record("arxiv:1207.7214", record),
    );
    let project = Project::new(network);
    let path = project.file(
        "same.bib",
        "@article{Imported,title={Imported},doi={10.1/same},eprint={1207.7214}}\n",
    );

    let report = add_file(
        &project.services,
        &project.store(),
        &AddFileRequest {
            source: InputSource::Path(path),
            provider: Provider::Inspire,
            overwrite: false,
            dry_run: false,
        },
    )
    .await
    .unwrap();

    assert_eq!(report.items.successes.len(), 1);
    assert!(report.items.failures.is_empty());
}

#[tokio::test]
async fn importing_does_not_ignore_a_failure_after_an_identifier_resolved() {
    let mut record = provider_record("inspire", "1", "Found", "Found paper");
    record.identifiers.doi = Some(Doi::new("10.1/found").unwrap());
    let failing = Arc::new(
        FakeProvider::new("inspire")
            .with_record("doi:10.1/found", record)
            .failing_retrieval("timed out"),
    );
    let project = Project::with(failing);
    let path = project.file(
        "partial.bib",
        "@article{Partial,title={Partial},doi={10.1/found},eprint={1207.7214}}\n",
    );

    let report = add_file(
        &project.services,
        &project.store(),
        &AddFileRequest {
            source: InputSource::Path(path),
            provider: Provider::Inspire,
            overwrite: false,
            dry_run: false,
        },
    )
    .await
    .unwrap();

    assert_eq!(report.items.failures.len(), 1);
    assert!(report.items.failures[0].message.contains("timed out"));
    assert!(!project.store().exists());
}

#[tokio::test]
async fn explicit_local_stores_entries_a_provider_would_have_resolved() {
    let project = Project::new(network());
    let path = project.file(
        "mine.bib",
        "@article{Mine:2012,\n  title = {My formatting},\n  eprint = {1207.7214}\n}\n",
    );
    let report = add_file(
        &project.services,
        &project.store(),
        &AddFileRequest {
            source: InputSource::Path(path),
            provider: Provider::Local,
            overwrite: false,
            dry_run: false,
        },
    )
    .await
    .unwrap();

    assert_eq!(report.items.successes.len(), 1);
    let records = list(&project.loaded(), &ListRequest::default());
    assert_eq!(records[0].provenance.provider.as_str(), "local");
    assert!(records[0].payload.source().contains("My formatting"));
    // No provider was consulted at all.
    assert!(network().calls().is_empty());
}

#[tokio::test]
async fn a_provider_failure_never_becomes_a_local_record() {
    let failing = Arc::new(FakeProvider::new("inspire").failing_retrieval("timed out"));
    let project = Project::new(failing);
    let path = project.file(
        "colleague.bib",
        "@article{TheirKey:2012,\n  title = {Their formatting},\n  eprint = {1207.7214}\n}\n",
    );
    let report = add_file(
        &project.services,
        &project.store(),
        &AddFileRequest {
            source: InputSource::Path(path),
            provider: Provider::Inspire,
            overwrite: false,
            dry_run: false,
        },
    )
    .await
    .unwrap();

    assert!(report.items.successes.is_empty());
    assert_eq!(report.items.failures.len(), 1);
    assert!(report.items.failures[0].message.contains("timed out"));
    assert!(!project.store().exists(), "nothing was committed");
}

#[tokio::test]
async fn an_imported_key_collision_is_skipped_or_identifies_the_overwrite_target() {
    let project = Project::new(network());
    add(
        &project,
        &["10.1/other"],
        AddRequest {
            key: Some(CitationKey::new("Taken").unwrap()),
            ..AddRequest::default()
        },
    )
    .await;
    let path = project.file(
        "colleague.bib",
        "@misc{Taken,\n  title = {A different work entirely}\n}\n",
    );
    let import = |overwrite| AddFileRequest {
        source: InputSource::Path(path.clone()),
        provider: Provider::Local,
        overwrite,
        dry_run: false,
    };

    // A file has no per-entry `--key`, so a collision is a skip, not a failure.
    let skipped = add_file(&project.services, &project.store(), &import(false))
        .await
        .unwrap();
    assert_eq!(skipped.items.skipped.len(), 1);
    assert!(!skipped.items.has_failures());

    // With `--overwrite`, the colliding key names the record to replace.
    let overwritten = add_file(&project.services, &project.store(), &import(true))
        .await
        .unwrap();
    assert_eq!(overwritten.items.successes[0].kind, AddKind::Overwritten);
    let records = list(&project.loaded(), &ListRequest::default());
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].key.as_str(), "Taken");
    assert_eq!(records[0].description.title, "A different work entirely");
}

#[tokio::test]
async fn two_imported_entries_that_duplicate_each_other_fail_only_one_item() {
    let project = Project::new(network());
    let path = project.file(
        "twice.bib",
        "@article{First,\n  title = {One}, \n  doi = {10.1/same}\n}\n\n@article{Second,\n  title = {Two},\n  doi = {10.1/same}\n}\n",
    );
    let report = add_file(
        &project.services,
        &project.store(),
        &AddFileRequest {
            source: InputSource::Path(path),
            provider: Provider::Local,
            overwrite: false,
            dry_run: false,
        },
    )
    .await
    .unwrap();

    // The second is caught against the growing candidate, so the first commits
    // rather than the whole batch failing validation.
    assert_eq!(report.items.successes.len(), 1);
    assert_eq!(report.items.skipped.len(), 1);
    assert!(report.committed);
}

#[tokio::test]
async fn multiple_identifier_targets_fail_one_item_without_discarding_the_batch() {
    let mut ambiguous = provider_record("inspire", "1", "Combined", "Combined paper");
    ambiguous.identifiers.doi = Some(Doi::new("10.1/combined").unwrap());
    ambiguous.identifiers.arxiv = Some(ArxivId::new("1207.7214").unwrap());
    let new = provider_record("inspire", "2", "New", "New paper");
    let network = Arc::new(
        FakeProvider::new("inspire")
            .with_record("doi:10.1/new", new)
            .with_record("doi:10.1/combined", ambiguous),
    );
    let project = Project::new(network);
    let path = project.file(
        "split-identifiers.bib",
        "@misc{ByDoi,title={By DOI},doi={10.1/combined}}\n\n@misc{ByArxiv,title={By arXiv},eprint={1207.7214}}\n",
    );
    add_file(
        &project.services,
        &project.store(),
        &AddFileRequest {
            source: InputSource::Path(path),
            provider: Provider::Local,
            overwrite: false,
            dry_run: false,
        },
    )
    .await
    .unwrap();

    let skipped = add(&project, &["10.1/combined"], AddRequest::default()).await;
    assert_eq!(skipped.items.failures.len(), 1);
    assert!(skipped.items.skipped.is_empty());

    let overwritten = add(
        &project,
        &["10.1/new", "10.1/combined"],
        AddRequest {
            overwrite: true,
            ..AddRequest::default()
        },
    )
    .await;
    assert_eq!(overwritten.items.successes.len(), 1);
    assert_eq!(overwritten.items.failures.len(), 1);
    assert!(overwritten.committed);
    let records = list(&project.loaded(), &ListRequest::default());
    assert_eq!(records.len(), 3, "two originals plus the unrelated success");
}

#[tokio::test]
async fn the_local_filter_uses_missing_provider_identity() {
    let project = Project::new(network());
    add(&project, &["1207.7214"], AddRequest::default()).await;
    let path = project.file("mine.bib", "@misc{Mine,title={Mine}}\n");
    add_file(
        &project.services,
        &project.store(),
        &AddFileRequest {
            source: InputSource::Path(path),
            provider: Provider::Local,
            overwrite: false,
            dry_run: false,
        },
    )
    .await
    .unwrap();

    let all = list(&project.loaded(), &ListRequest::default());
    assert_eq!(all.len(), 2);
    let local = list(
        &project.loaded(),
        &ListRequest {
            filter: bibi_core::RecordFilter {
                local: true,
                ..bibi_core::RecordFilter::default()
            },
        },
    );
    assert_eq!(local.len(), 1);
    assert_eq!(local[0].key.as_str(), "Mine");
}

#[tokio::test]
async fn the_json_projection_has_the_schema_one_fields_and_no_payload() {
    let project = Project::new(network());
    add(&project, &["1207.7214"], AddRequest::default()).await;
    let records = list(&project.loaded(), &ListRequest::default());
    let json = to_json(&records).unwrap();

    assert!(json.ends_with("]\n"));
    assert!(
        !json.contains("bibtex"),
        "the payload is not part of the projection"
    );
    for field in [
        "id",
        "key",
        "provider",
        "provider_id",
        "revision",
        "doi",
        "arxiv",
        "title",
        "authors",
        "collaborations",
        "year",
    ] {
        assert!(json.contains(&format!("\"{field}\"")), "missing `{field}`");
    }
    // Absent optionals are present as null rather than omitted.
    assert!(json.contains("\"doi\": null"));
}

#[tokio::test]
async fn removing_emits_what_it_deleted_and_renaming_preserves_the_payload() {
    let project = Project::new(network());
    add(&project, &["1207.7214"], AddRequest::default()).await;

    let renamed = rename(
        &project.store(),
        "Aad:2012tfa",
        &CitationKey::new("Higgs").unwrap(),
    )
    .unwrap();
    assert_eq!(renamed.key.as_str(), "Higgs");
    assert!(renamed.payload.source().contains("@article{Aad:2012tfa,"));

    // The record is still reachable by its identifiers after a rename.
    let shown = show(&project.store(), "inspire:1124337").unwrap();
    assert!(shown.starts_with("@article{Higgs,"));

    let report = remove(&project.store(), &["Higgs".to_owned()], false).unwrap();
    assert_eq!(report.items.successes.len(), 1);
    assert_eq!(report.items.successes[0].key.as_str(), "Higgs");
    assert!(list(&project.loaded(), &ListRequest::default()).is_empty());
}

#[tokio::test]
async fn removing_the_same_record_twice_is_an_idempotent_skip() {
    let project = Project::new(network());
    add(&project, &["1207.7214"], AddRequest::default()).await;

    let selectors = ["Aad:2012tfa".to_owned(), "inspire:1124337".to_owned()];
    let report = remove(&project.store(), &selectors, false).unwrap();
    assert_eq!(report.items.successes.len(), 1);
    assert_eq!(report.items.skipped.len(), 1);
    assert!(report.items.failures.is_empty());
    assert!(report.committed);
    assert!(report.items.skipped[0].key.is_none());

    add(&project, &["1207.7214"], AddRequest::default()).await;
    let dry_run = remove(&project.store(), &selectors, true).unwrap();
    assert_eq!(dry_run.items.successes.len(), 1);
    assert_eq!(dry_run.items.skipped.len(), 1);
    assert!(dry_run.items.failures.is_empty());
    assert!(!dry_run.committed);
    assert_eq!(
        list(&project.loaded(), &ListRequest::default()).len(),
        1,
        "the dry run leaves the record in place"
    );
}

#[tokio::test]
async fn a_commit_is_refused_when_the_manifest_changed_during_the_work() {
    let project = Project::new(network());
    add(&project, &["1207.7214"], AddRequest::default()).await;
    let store = project.store();
    let (_manifest, generation) = store.load().unwrap().into_parts();

    // Somebody edits the manifest while a long-running command holds it.
    std::fs::write(store.path(), project.manifest() + "\n").unwrap();
    let mut candidate = store.load().unwrap().manifest.to_candidate();
    let target = candidate.records()[0].id;
    candidate
        .rename(&target, CitationKey::new("Renamed").unwrap())
        .unwrap();
    assert!(store.commit(&generation, candidate).is_err());
}
