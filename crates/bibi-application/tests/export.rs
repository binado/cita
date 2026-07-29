//! Export and check: offline, deterministic, and refusing to eat the manifest.

use bibi_application::{
    AddRequest, CheckOutcome, ExportRequest, PlatformPaths, RenderOptions, Services, SyncRequest,
    add_locators, check, domain::ManifestStore, export, sync,
};
use bibi_core::RecordFilter;
use bibi_provider::{
    LocalProvider, Provider, ProviderRegistry, RefreshState,
    testing::{FakeProvider, payload, provider_metadata, provider_record},
};
use std::{path::PathBuf, sync::Arc};
use tempfile::TempDir;

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

    fn store(&self) -> ManifestStore {
        ManifestStore::new(self.directory.path().join("bibi.toml"))
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }
}

/// A provider holding two records, one from each of two years.
fn stocked() -> Arc<FakeProvider> {
    let mut first = provider_record("inspire", "1", "Alpha:2012", "First paper");
    first.description.year = Some(2012);
    first.description.authors = vec!["Aad, G.".into()];
    let mut second = provider_record("inspire", "2", "Zed:2020", "Second paper");
    second.description.year = Some(2020);
    second.description.authors = vec!["Roe, Richard".into()];
    Arc::new(
        FakeProvider::new("inspire")
            .with_record("arxiv:2401.00001", first)
            .with_record("arxiv:2401.00002", second),
    )
}

async fn project_with_two_records() -> Project {
    let project = Project::with(vec![stocked(), Arc::new(LocalProvider::new())]);
    add_locators(
        &project.services,
        &project.store(),
        &["2401.00001".to_owned(), "2401.00002".to_owned()],
        &AddRequest::default(),
    )
    .await
    .unwrap();
    project
}

#[tokio::test]
async fn a_plain_export_writes_the_default_file_beside_the_manifest() {
    let project = project_with_two_records().await;
    let report = export(
        &project.services,
        &project.store(),
        &ExportRequest::default(),
    )
    .await
    .unwrap();

    assert_eq!(report.path, project.path("references.bib"));
    assert_eq!(report.records, 2);
    assert!(report.sync.is_none(), "a plain export syncs nothing");
    let written = std::fs::read_to_string(&report.path).unwrap();
    // Local-key order, one blank line between entries, one final newline.
    assert_eq!(
        written,
        "@article{Alpha:2012,title={First paper}}\n\n@article{Zed:2020,title={Second paper}}\n"
    );
}

#[tokio::test]
async fn a_plain_export_never_touches_a_provider() {
    let provider = stocked();
    let project = Project::with(vec![provider.clone(), Arc::new(LocalProvider::new())]);
    add_locators(
        &project.services,
        &project.store(),
        &["2401.00001".to_owned()],
        &AddRequest::default(),
    )
    .await
    .unwrap();
    let before = provider.calls().len();

    export(
        &project.services,
        &project.store(),
        &ExportRequest::default(),
    )
    .await
    .unwrap();
    // Rendering is a pure function of the manifest and its options: no
    // request, no cache probe, nothing ambient.
    assert_eq!(provider.calls().len(), before);
}

#[tokio::test]
async fn rendering_the_same_manifest_twice_produces_the_same_bytes() {
    let project = project_with_two_records().await;
    let request = ExportRequest {
        output: Some(PathBuf::from("first.bib")),
        ..ExportRequest::default()
    };
    export(&project.services, &project.store(), &request)
        .await
        .unwrap();
    let first = std::fs::read_to_string(project.path("first.bib")).unwrap();

    export(
        &project.services,
        &project.store(),
        &ExportRequest {
            output: Some(PathBuf::from("second.bib")),
            ..ExportRequest::default()
        },
    )
    .await
    .unwrap();
    let second = std::fs::read_to_string(project.path("second.bib")).unwrap();
    assert_eq!(first, second);
}

#[tokio::test]
async fn a_filtered_export_renders_only_what_matches() {
    let project = project_with_two_records().await;
    let report = export(
        &project.services,
        &project.store(),
        &ExportRequest {
            output: Some(PathBuf::from("subset.bib")),
            options: RenderOptions {
                filter: RecordFilter {
                    year: Some(2020),
                    ..RecordFilter::default()
                },
            },
            ..ExportRequest::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(report.records, 1);
    let written = std::fs::read_to_string(project.path("subset.bib")).unwrap();
    assert_eq!(written, "@article{Zed:2020,title={Second paper}}\n");
}

#[tokio::test]
async fn an_empty_selection_renders_zero_bytes() {
    let project = project_with_two_records().await;
    export(
        &project.services,
        &project.store(),
        &ExportRequest {
            output: Some(PathBuf::from("none.bib")),
            options: RenderOptions {
                filter: RecordFilter {
                    year: Some(1900),
                    ..RecordFilter::default()
                },
            },
            ..ExportRequest::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(project.path("none.bib")).unwrap(),
        ""
    );
}

#[tokio::test]
async fn an_output_path_that_resolves_to_the_manifest_is_refused() {
    let project = project_with_two_records().await;
    for output in ["bibi.toml", "./bibi.toml", "sub/../bibi.toml"] {
        let error = export(
            &project.services,
            &project.store(),
            &ExportRequest {
                output: Some(PathBuf::from(output)),
                ..ExportRequest::default()
            },
        )
        .await;
        // `sub/..` only resolves if `sub` exists; create it so the alternate
        // spelling is really exercised rather than silently allowed.
        if output.contains("..") {
            std::fs::create_dir_all(project.path("sub")).unwrap();
            let retried = export(
                &project.services,
                &project.store(),
                &ExportRequest {
                    output: Some(PathBuf::from(output)),
                    ..ExportRequest::default()
                },
            )
            .await;
            assert!(retried.is_err(), "{output} should be refused");
            continue;
        }
        assert!(error.is_err(), "{output} should be refused");
    }
    // The manifest is intact.
    assert!(
        std::fs::read_to_string(project.store().path())
            .unwrap()
            .starts_with("schema = 1")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_symlink_to_the_manifest_is_refused_too() {
    let project = project_with_two_records().await;
    std::os::unix::fs::symlink(project.store().path(), project.path("alias.toml")).unwrap();
    let error = export(
        &project.services,
        &project.store(),
        &ExportRequest {
            output: Some(PathBuf::from("alias.toml")),
            ..ExportRequest::default()
        },
    )
    .await;
    assert!(error.is_err(), "a symlink is another spelling of the path");
}

#[tokio::test]
async fn export_with_a_provider_syncs_first_and_renders_the_committed_manifest() {
    let project = project_with_two_records().await;
    let updated = Arc::new(
        FakeProvider::new("inspire")
            .with_refresh(
                "1",
                RefreshState::Metadata(Box::new(provider_metadata("1", Some("r2"), "Corrected"))),
            )
            .with_payload("1", Some(payload("Alpha:2012", "Corrected")))
            .with_refresh(
                "2",
                RefreshState::Metadata(Box::new(provider_metadata(
                    "2",
                    Some("r1"),
                    "Second paper",
                ))),
            )
            .with_payload("2", Some(payload("Zed:2020", "Second paper"))),
    );
    let services = Services::new(
        Arc::new(ProviderRegistry::new(vec![
            updated,
            Arc::new(LocalProvider::new()),
        ])),
        project.services.paths.clone(),
    );

    let report = export(
        &services,
        &project.store(),
        &ExportRequest {
            provider: Some(bibi_core::ProviderName::new("inspire").unwrap()),
            ..ExportRequest::default()
        },
    )
    .await
    .unwrap();

    assert!(report.sync.is_some());
    // Both records are exported: --provider selects what to sync, never what to
    // render.
    assert_eq!(report.records, 2);
    let written = std::fs::read_to_string(&report.path).unwrap();
    assert!(written.contains("Corrected"));
    assert!(written.contains("Second paper"));
}

#[tokio::test]
async fn export_writes_nothing_when_the_preliminary_sync_fails() {
    let project = project_with_two_records().await;
    let broken = Arc::new(
        FakeProvider::new("inspire")
            .with_refresh(
                "1",
                RefreshState::Metadata(Box::new(provider_metadata("1", Some("r2"), "Corrected"))),
            )
            .failing_payloads("texkey claimed twice"),
    );
    let services = Services::new(
        Arc::new(ProviderRegistry::new(vec![
            broken,
            Arc::new(LocalProvider::new()),
        ])),
        project.services.paths.clone(),
    );

    let error = export(
        &services,
        &project.store(),
        &ExportRequest {
            provider: Some(bibi_core::ProviderName::new("inspire").unwrap()),
            ..ExportRequest::default()
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("no bibliography was written"));
    assert!(!project.path("references.bib").exists());
}

#[tokio::test]
async fn force_without_a_provider_is_a_usage_error() {
    let project = project_with_two_records().await;
    let error = export(
        &project.services,
        &project.store(),
        &ExportRequest {
            force: true,
            ..ExportRequest::default()
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("--provider"));
}

#[tokio::test]
async fn check_matches_drifts_and_reports_a_missing_file_as_missing() {
    let project = project_with_two_records().await;
    let bibliography = project.path("references.bib");

    // Missing is its own answer, not drift against nothing.
    assert!(matches!(
        check(&project.store(), &bibliography, &RenderOptions::default()).unwrap(),
        CheckOutcome::Missing { .. }
    ));

    export(
        &project.services,
        &project.store(),
        &ExportRequest::default(),
    )
    .await
    .unwrap();
    assert!(matches!(
        check(&project.store(), &bibliography, &RenderOptions::default()).unwrap(),
        CheckOutcome::Match { .. }
    ));

    // A hand edit to the rendered file is drift.
    std::fs::write(&bibliography, "@article{Alpha:2012,title={Edited}}\n").unwrap();
    let outcome = check(&project.store(), &bibliography, &RenderOptions::default()).unwrap();
    let CheckOutcome::Drift { summary, .. } = &outcome else {
        panic!("expected drift, got {outcome:?}");
    };
    assert!(summary.contains("first difference at line 1"));
    assert!(outcome.failed());

    // Checking never repairs.
    assert_eq!(
        std::fs::read_to_string(&bibliography).unwrap(),
        "@article{Alpha:2012,title={Edited}}\n"
    );
}

#[tokio::test]
async fn a_manifest_change_makes_a_previously_matching_bibliography_drift() {
    let project = project_with_two_records().await;
    export(
        &project.services,
        &project.store(),
        &ExportRequest::default(),
    )
    .await
    .unwrap();
    let bibliography = project.path("references.bib");

    let updated = Arc::new(
        FakeProvider::new("inspire")
            .with_refresh(
                "1",
                RefreshState::Metadata(Box::new(provider_metadata("1", Some("r2"), "Corrected"))),
            )
            .with_payload("1", Some(payload("Alpha:2012", "Corrected"))),
    );
    let services = Services::new(
        Arc::new(ProviderRegistry::new(vec![
            updated,
            Arc::new(LocalProvider::new()),
        ])),
        project.services.paths.clone(),
    );
    sync(&services, &project.store(), &SyncRequest::default())
        .await
        .unwrap();

    // The file the user committed is now stale, and `check` is how that
    // boundary is covered without making the file maintained state.
    assert!(matches!(
        check(&project.store(), &bibliography, &RenderOptions::default()).unwrap(),
        CheckOutcome::Drift { .. }
    ));
}
