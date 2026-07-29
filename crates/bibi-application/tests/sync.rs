//! Conditional refresh: what is fetched, what is written, and what is not.

use bibi_application::{
    AddRequest, ListRequest, Services, SyncRequest, add_locators, domain::Manifest,
    domain::ManifestStore, list, sync,
};
use bibi_provider::{
    LocalProvider, Provider, ProviderRegistry, RefreshState,
    testing::{FakeProvider, ProviderCall, payload, provider_metadata, provider_record},
};
use std::sync::Arc;
use tempfile::TempDir;

struct Project {
    directory: TempDir,
    services: Services,
}

impl Project {
    fn with(providers: Vec<Arc<dyn Provider>>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let services = Services::new(Arc::new(ProviderRegistry::new(providers)));
        Self {
            directory,
            services,
        }
    }

    fn store(&self) -> ManifestStore {
        ManifestStore::new(self.directory.path().join("bibi.toml"))
    }

    /// The manifest as the commands see it, loaded fresh.
    fn loaded(&self) -> Manifest {
        self.store().load().unwrap().manifest
    }

    fn rebuild(self, providers: Vec<Arc<dyn Provider>>) -> Self {
        let services = Services::new(Arc::new(ProviderRegistry::new(providers)));
        Self {
            directory: self.directory,
            services,
        }
    }
}

/// A provider holding `count` records, keyed `1..=count`.
fn stocked(count: usize) -> FakeProvider {
    let mut provider = FakeProvider::new("inspire");
    for index in 1..=count {
        let id = index.to_string();
        provider = provider
            .with_record(
                &format!("arxiv:2401.{index:05}"),
                provider_record(
                    "inspire",
                    &id,
                    &format!("Key:{index}"),
                    &format!("Title {index}"),
                ),
            )
            .with_refresh(
                &id,
                RefreshState::Metadata(Box::new(provider_metadata(
                    &id,
                    Some("r1"),
                    &format!("Title {index}"),
                ))),
            )
            .with_payload(
                &id,
                Some(payload(&format!("Key:{index}"), &format!("Title {index}"))),
            );
    }
    provider
}

async fn seed(project: &Project, count: usize) {
    let locators = (1..=count)
        .map(|index| format!("2401.{index:05}"))
        .collect::<Vec<_>>();
    let report = add_locators(
        &project.services,
        &project.store(),
        &locators,
        &AddRequest::default(),
    )
    .await
    .unwrap();
    assert_eq!(report.items.successes.len(), count, "seeding failed");
}

fn resolves(call: &ProviderCall) -> bool {
    matches!(call, ProviderCall::Resolve(_))
}

fn fetches_payloads(call: &ProviderCall) -> bool {
    matches!(call, ProviderCall::FetchPayloads(_))
}

#[tokio::test]
async fn an_unchanged_sync_fetches_no_bibtex_at_all() {
    let provider = Arc::new(stocked(3));
    let project = Project::with(vec![provider.clone(), Arc::new(LocalProvider::new())]);
    seed(&project, 3).await;
    let before = provider.call_count(fetches_payloads);

    let report = sync(&project.services, &project.store(), &SyncRequest::default())
        .await
        .unwrap();

    assert_eq!(report.examined, 3);
    assert_eq!(report.unchanged, 3);
    assert!(report.refreshed.is_empty());
    assert!(!report.committed, "nothing changed, so nothing was written");
    assert_eq!(
        provider.call_count(fetches_payloads),
        before,
        "a revision comparison is enough; no payload was requested"
    );
}

#[tokio::test]
async fn a_forced_sync_refetches_everything_in_batched_calls() {
    let provider = Arc::new(stocked(3));
    let project = Project::with(vec![provider.clone(), Arc::new(LocalProvider::new())]);
    seed(&project, 3).await;
    let resolves_before = provider.call_count(resolves);

    let report = sync(
        &project.services,
        &project.store(),
        &SyncRequest {
            force: true,
            ..SyncRequest::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(report.refreshed.len(), 3);
    assert!(report.committed);
    // One metadata call and one payload call for the whole project, however
    // many records it holds: batching is the provider's business, and the
    // application asks once per provider.
    let calls = provider.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|call| matches!(call, ProviderCall::RefreshMetadata(_)))
            .count(),
        1
    );
    assert_eq!(provider.call_count(fetches_payloads), 1);
    assert_eq!(provider.call_count(resolves), resolves_before);
}

#[tokio::test]
async fn a_changed_revision_refreshes_the_record_and_keeps_its_identity() {
    let project = Project::with(vec![Arc::new(stocked(1)), Arc::new(LocalProvider::new())]);
    seed(&project, 1).await;
    let before = list(&project.loaded(), &ListRequest::default());

    let updated = Arc::new(
        FakeProvider::new("inspire")
            .with_refresh(
                "1",
                RefreshState::Metadata(Box::new(provider_metadata("1", Some("r2"), "Corrected"))),
            )
            .with_payload("1", Some(payload("Key:1", "Corrected"))),
    );
    let project = project.rebuild(vec![updated, Arc::new(LocalProvider::new())]);
    let report = sync(&project.services, &project.store(), &SyncRequest::default())
        .await
        .unwrap();

    assert_eq!(report.refreshed.len(), 1);
    assert_eq!(report.description_changes, 1);
    assert!(report.committed);
    let after = list(&project.loaded(), &ListRequest::default());
    assert_eq!(after[0].id, before[0].id, "the bibi id is immutable");
    assert_eq!(after[0].key, before[0].key, "a refresh never renames");
    assert_eq!(after[0].description.title, "Corrected");
    assert_eq!(
        after[0].provenance.revision.as_ref().unwrap().as_str(),
        "r2"
    );
}

#[tokio::test]
async fn a_record_whose_payload_never_arrives_keeps_its_old_revision() {
    let project = Project::with(vec![Arc::new(stocked(1)), Arc::new(LocalProvider::new())]);
    seed(&project, 1).await;

    // Metadata says the record changed; the BibTeX search returns nothing for it.
    let half = Arc::new(
        FakeProvider::new("inspire")
            .with_refresh(
                "1",
                RefreshState::Metadata(Box::new(provider_metadata("1", Some("r2"), "Corrected"))),
            )
            .with_payload("1", None),
    );
    let project = project.rebuild(vec![half, Arc::new(LocalProvider::new())]);
    let report = sync(&project.services, &project.store(), &SyncRequest::default())
        .await
        .unwrap();

    assert!(report.refreshed.is_empty());
    assert_eq!(report.absences.len(), 1);
    assert_eq!(
        report.absences[0].reason,
        bibi_application::SyncAbsenceReason::PayloadAbsent
    );
    assert!(!report.committed);
    let after = list(&project.loaded(), &ListRequest::default());
    // Nothing was written — above all not the revision, which would have made
    // the next sync compare equal and skip the repair forever.
    assert_eq!(
        after[0].provenance.revision.as_ref().unwrap().as_str(),
        "r1"
    );
    assert_eq!(after[0].description.title, "Title 1");
}

#[tokio::test]
async fn an_ambiguous_join_fails_its_provider_while_others_commit() {
    let broken = Arc::new(
        FakeProvider::new("inspire")
            .with_record(
                "arxiv:2401.00001",
                provider_record("inspire", "1", "Key:1", "One"),
            )
            .with_refresh(
                "1",
                RefreshState::Metadata(Box::new(provider_metadata("1", Some("r2"), "One"))),
            )
            .failing_payloads("texkey `Shared` is claimed by records 1 and 2"),
    );
    let working = Arc::new(
        FakeProvider::new("other")
            .with_record("doi:10.1/b", provider_record("other", "9", "Key:9", "Nine"))
            .with_refresh(
                "9",
                RefreshState::Metadata(Box::new(provider_metadata("9", Some("r2"), "Nine again"))),
            )
            .with_payload("9", Some(payload("Key:9", "Nine again"))),
    );
    let project = Project::with(vec![
        broken.clone(),
        working.clone(),
        Arc::new(LocalProvider::new()),
    ]);
    add_locators(
        &project.services,
        &project.store(),
        &["2401.00001".to_owned(), "10.1/b".to_owned()],
        &AddRequest::default(),
    )
    .await
    .unwrap();

    let report = sync(&project.services, &project.store(), &SyncRequest::default())
        .await
        .unwrap();

    assert_eq!(report.failures.len(), 1);
    assert!(report.failures[0].message.contains("claimed by records"));
    // The other provider's record still refreshed, and the write happened.
    assert_eq!(report.refreshed.len(), 1);
    assert!(report.committed);
    let after = list(&project.loaded(), &ListRequest::default());
    let nine = after
        .iter()
        .find(|record| record.key.as_str() == "Key:9")
        .unwrap();
    assert_eq!(nine.description.title, "Nine again");
    let one = after
        .iter()
        .find(|record| record.key.as_str() == "Key:1")
        .unwrap();
    assert_eq!(one.description.title, "One", "left exactly as it was");
}

#[tokio::test]
async fn a_missing_record_is_a_warning_and_never_a_rebind() {
    let project = Project::with(vec![Arc::new(stocked(1)), Arc::new(LocalProvider::new())]);
    seed(&project, 1).await;
    // The provider no longer holds it: the fake answers Missing by default.
    let forgetful = Arc::new(FakeProvider::new("inspire"));
    let project = project.rebuild(vec![forgetful, Arc::new(LocalProvider::new())]);

    let report = sync(&project.services, &project.store(), &SyncRequest::default())
        .await
        .unwrap();
    assert_eq!(report.absences.len(), 1);
    assert_eq!(
        report.absences[0].reason,
        bibi_application::SyncAbsenceReason::ProviderGone
    );
    assert!(report.failures.is_empty(), "absence is not a failure");
    assert!(!report.committed);
}

#[tokio::test]
async fn a_replaced_identifier_fails_its_record_while_an_added_one_is_reported() {
    // One record is seeded with an arXiv id already stored, so a provider that
    // reports a different one is proposing a replacement rather than a gain.
    let mut stored = provider_record("inspire", "2", "Key:2", "Title 2");
    stored.identifiers.arxiv = Some(bibi_core::ArxivId::new("2401.00002").unwrap());
    let seeder = Arc::new(
        FakeProvider::new("inspire")
            .with_record(
                "arxiv:2401.00001",
                provider_record("inspire", "1", "Key:1", "Title 1"),
            )
            .with_record("arxiv:2401.00002", stored),
    );
    let project = Project::with(vec![seeder, Arc::new(LocalProvider::new())]);
    add_locators(
        &project.services,
        &project.store(),
        &["2401.00001".to_owned(), "2401.00002".to_owned()],
        &AddRequest::default(),
    )
    .await
    .unwrap();

    let mut gains_a_doi = provider_metadata("1", Some("r2"), "Title 1");
    gains_a_doi.identifiers.doi = Some(bibi_core::Doi::new("10.1/new").unwrap());
    let mut replaces_an_arxiv_id = provider_metadata("2", Some("r2"), "Title 2");
    replaces_an_arxiv_id.identifiers.arxiv = Some(bibi_core::ArxivId::new("2912.99999").unwrap());

    let provider = Arc::new(
        FakeProvider::new("inspire")
            .with_refresh("1", RefreshState::Metadata(Box::new(gains_a_doi)))
            .with_payload("1", Some(payload("Key:1", "Title 1")))
            .with_refresh("2", RefreshState::Metadata(Box::new(replaces_an_arxiv_id)))
            .with_payload("2", Some(payload("Key:2", "Title 2"))),
    );
    let project = project.rebuild(vec![provider, Arc::new(LocalProvider::new())]);
    let report = sync(&project.services, &project.store(), &SyncRequest::default())
        .await
        .unwrap();

    // A preprint gaining a DOI on publication is ordinary, and named.
    assert_eq!(report.identifier_additions.len(), 1);
    assert_eq!(report.identifier_additions[0].kind, "DOI");
    assert_eq!(report.identifier_additions[0].value, "10.1/new");
    assert_eq!(report.refreshed.len(), 1);

    // A replacement contradicts the claim that identifiers never change once
    // set, so the record fails rather than being quietly rewritten.
    assert_eq!(report.failures.len(), 1);
    assert!(
        report.failures[0]
            .message
            .contains("do not change once set")
    );
    assert!(report.committed, "the good record still commits");
    let after = list(&project.loaded(), &ListRequest::default());
    let untouched = after.iter().find(|r| r.key.as_str() == "Key:2").unwrap();
    assert_eq!(
        untouched.identifiers.arxiv.as_ref().unwrap().as_str(),
        "2401.00002"
    );
    assert_eq!(
        untouched.provenance.revision.as_ref().unwrap().as_str(),
        "r1"
    );
}

#[tokio::test]
async fn a_local_record_is_counted_rather_than_asked_about() {
    let project = Project::with(vec![Arc::new(stocked(1)), Arc::new(LocalProvider::new())]);
    seed(&project, 1).await;
    bibi_application::add_file(
        &project.services,
        &project.store(),
        &bibi_application::AddFileRequest {
            source: bibi_application::InputSource::Path({
                let path = project.directory.path().join("mine.bib");
                std::fs::write(&path, "@misc{Mine,title={Mine}}\n").unwrap();
                path
            }),
            provider: None,
            overwrite: false,
            force_local: true,
            dry_run: false,
        },
    )
    .await
    .unwrap();

    let report = sync(&project.services, &project.store(), &SyncRequest::default())
        .await
        .unwrap();
    assert_eq!(report.examined, 2);
    assert_eq!(report.unrefreshable, 1);
    assert_eq!(report.unchanged, 1);
}

#[tokio::test]
async fn an_uninstalled_provider_is_skipped_by_a_plain_sync_and_named_by_an_explicit_one() {
    let project = Project::with(vec![Arc::new(stocked(1)), Arc::new(LocalProvider::new())]);
    seed(&project, 1).await;
    // This build no longer carries `inspire`, but the manifest still names it.
    let project = project.rebuild(vec![Arc::new(LocalProvider::new())]);

    let report = sync(&project.services, &project.store(), &SyncRequest::default())
        .await
        .unwrap();
    assert_eq!(report.unavailable.len(), 1);
    assert!(report.failures.is_empty());
    assert!(!report.committed);

    // Naming it explicitly is a usage error, raised before any I/O.
    let error = sync(
        &project.services,
        &project.store(),
        &SyncRequest {
            provider: Some(bibi_core::ProviderName::new("inspire").unwrap()),
            ..SyncRequest::default()
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("not available"));
}

#[tokio::test]
async fn a_dry_run_does_the_work_and_writes_nothing() {
    let project = Project::with(vec![Arc::new(stocked(1)), Arc::new(LocalProvider::new())]);
    seed(&project, 1).await;
    let before = std::fs::read_to_string(project.store().path()).unwrap();

    let updated = Arc::new(
        FakeProvider::new("inspire")
            .with_refresh(
                "1",
                RefreshState::Metadata(Box::new(provider_metadata("1", Some("r2"), "Corrected"))),
            )
            .with_payload("1", Some(payload("Key:1", "Corrected"))),
    );
    let project = project.rebuild(vec![updated.clone(), Arc::new(LocalProvider::new())]);
    let report = sync(
        &project.services,
        &project.store(),
        &SyncRequest {
            dry_run: true,
            ..SyncRequest::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(report.refreshed.len(), 1);
    assert!(!report.committed);
    assert_eq!(
        std::fs::read_to_string(project.store().path()).unwrap(),
        before
    );
    // The requests still happened, so the report is real rather than guessed.
    assert_eq!(updated.call_count(fetches_payloads), 1);
}
