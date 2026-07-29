//! Command flows over fake providers and temporary manifests.

use bibi_application::{
    AddFileRequest, AddKind, AddRequest, InputSource, ListRequest, PlatformPaths, Services,
    add_file, add_locators, domain::CitationKey, domain::ManifestStore, list, remove, rename, show,
    to_json,
};
use bibi_provider::{
    LocalProvider, Provider, ProviderRegistry,
    testing::{FakeProvider, provider_record},
};
use std::{path::PathBuf, sync::Arc};
use tempfile::TempDir;

/// A project directory with an injected provider roster.
struct Project {
    directory: TempDir,
    services: Services,
}

impl Project {
    fn with(providers: Vec<Arc<dyn Provider>>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let services = Services::new(
            Arc::new(ProviderRegistry::new(providers)),
            PlatformPaths::new(directory.path().join("global.toml"), directory.path()),
        );
        Self {
            directory,
            services,
        }
    }

    /// The default roster: one network provider, then local.
    fn new(network: Arc<FakeProvider>) -> Self {
        Self::with(vec![network, Arc::new(LocalProvider::new())])
    }

    fn store(&self) -> ManifestStore {
        ManifestStore::new(self.directory.path().join("bibi.toml"))
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
    assert!(list(&project.services, &project.store(), &ListRequest::default()).is_err());
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
    // The payload keeps the provider's key; only the rendering adopts the local one.
    assert!(
        report.items.successes[0]
            .bibtex
            .starts_with("@article{Higgs,")
    );
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
async fn a_duplicate_is_skipped_and_still_emits_its_bibtex() {
    let project = Project::new(network());
    add(&project, &["1207.7214"], AddRequest::default()).await;
    let report = add(&project, &["1207.7214"], AddRequest::default()).await;

    assert!(report.items.successes.is_empty());
    assert_eq!(report.items.skipped.len(), 1);
    assert!(!report.items.has_failures(), "a skip is not a failure");
    assert!(!report.committed, "nothing changed, so nothing was written");
    assert!(
        report.items.skipped[0]
            .bibtex
            .as_ref()
            .unwrap()
            .contains("@article{Aad:2012tfa,")
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
    let before = list(&project.services, &project.store(), &ListRequest::default()).unwrap();

    let updated = Arc::new(FakeProvider::new("inspire").with_record(
        "arxiv:1207.7214",
        provider_record("inspire", "1124337", "Aad:2012tfa", "Corrected title"),
    ));
    let project = Project {
        directory: project.directory,
        services: Services::new(
            Arc::new(ProviderRegistry::new(vec![
                updated,
                Arc::new(LocalProvider::new()),
            ])),
            project.services.paths.clone(),
        ),
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
    let after = list(&project.services, &project.store(), &ListRequest::default()).unwrap();
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
    assert_eq!(
        list(&project.services, &project.store(), &ListRequest::default())
            .unwrap()
            .len(),
        1
    );
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
async fn importing_resolves_what_it_can_and_keeps_the_rest_locally() {
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
            provider: None,
            overwrite: false,
            force_local: false,
            dry_run: false,
        },
    )
    .await
    .unwrap();

    assert_eq!(report.items.successes.len(), 2);
    let records = list(&project.services, &project.store(), &ListRequest::default()).unwrap();
    let resolved = records
        .iter()
        .find(|record| record.key.as_str() == "TheirKey:2012")
        .unwrap();
    // Resolved: the provider's payload, but the colleague's citation key.
    assert_eq!(resolved.provenance.provider.as_str(), "inspire");
    assert!(resolved.payload.source().contains("@article{Aad:2012tfa,"));
    assert_eq!(resolved.description.title, "Observation");

    let retained = records
        .iter()
        .find(|record| record.key.as_str() == "Notes:2026")
        .unwrap();
    assert_eq!(retained.provenance.provider.as_str(), "local");
    assert!(retained.payload.source().contains("Lecture notes"));
}

#[tokio::test]
async fn force_local_stores_entries_a_provider_would_have_resolved() {
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
            provider: None,
            overwrite: false,
            force_local: true,
            dry_run: false,
        },
    )
    .await
    .unwrap();

    assert_eq!(report.items.successes.len(), 1);
    let records = list(&project.services, &project.store(), &ListRequest::default()).unwrap();
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
            provider: None,
            overwrite: false,
            force_local: false,
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
        provider: None,
        overwrite,
        force_local: false,
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
    let records = list(&project.services, &project.store(), &ListRequest::default()).unwrap();
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
            provider: None,
            overwrite: false,
            force_local: true,
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
async fn the_local_filter_asks_the_registry_rather_than_matching_a_name() {
    let project = Project::new(network());
    add(&project, &["1207.7214"], AddRequest::default()).await;
    let path = project.file("mine.bib", "@misc{Mine,title={Mine}}\n");
    add_file(
        &project.services,
        &project.store(),
        &AddFileRequest {
            source: InputSource::Path(path),
            provider: None,
            overwrite: false,
            force_local: true,
            dry_run: false,
        },
    )
    .await
    .unwrap();

    let all = list(&project.services, &project.store(), &ListRequest::default()).unwrap();
    assert_eq!(all.len(), 2);
    let local = list(
        &project.services,
        &project.store(),
        &ListRequest {
            local: true,
            ..ListRequest::default()
        },
    )
    .unwrap();
    assert_eq!(local.len(), 1);
    assert_eq!(local[0].key.as_str(), "Mine");
}

#[tokio::test]
async fn the_json_projection_has_the_schema_one_fields_and_no_payload() {
    let project = Project::new(network());
    add(&project, &["1207.7214"], AddRequest::default()).await;
    let records = list(&project.services, &project.store(), &ListRequest::default()).unwrap();
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
    assert!(report.items.successes[0].bibtex.contains("@article{Higgs,"));
    assert!(
        list(&project.services, &project.store(), &ListRequest::default())
            .unwrap()
            .is_empty()
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
