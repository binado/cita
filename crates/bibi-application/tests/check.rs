//! Rendering and checking: offline, deterministic, and never repairing.
//!
//! bibi writes no bibliography of its own. `render_manifest` produces the bytes
//! a caller pipes wherever it likes, and `check` compares a file someone else
//! placed against those same bytes. Both are pure functions of the manifest and
//! the render options.

use bibi_application::{
    AddRequest, CheckOutcome, RenderOptions, Services, SyncRequest, add_locators, check,
    domain::ManifestStore, render_manifest, sync,
};
use bibi_core::{ProviderName, RecordFilter};
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
        let services = Services::new(Arc::new(ProviderRegistry::new(providers)));
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

    /// Render the committed manifest, as `list --format bibtex` does.
    fn render(&self, filter: RecordFilter) -> String {
        let manifest = self.store().load().unwrap().manifest;
        render_manifest(&manifest, &RenderOptions { filter }).unwrap()
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

/// Add one record the local provider owns and no provider can refresh.
async fn add_local_record(project: &Project) {
    let path = project.path("mine.bib");
    std::fs::write(&path, "@misc{Mine,title={Mine}}\n").unwrap();
    bibi_application::add_file(
        &project.services,
        &project.store(),
        &bibi_application::AddFileRequest {
            source: bibi_application::InputSource::Path(path),
            provider: None,
            overwrite: false,
            force_local: true,
            dry_run: false,
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn rendering_puts_every_record_in_local_key_order() {
    let project = project_with_two_records().await;
    // One blank line between entries, one final newline, and nothing else.
    assert_eq!(
        project.render(RecordFilter::default()),
        "@article{Alpha:2012,title={First paper}}\n\n@article{Zed:2020,title={Second paper}}\n"
    );
}

#[tokio::test]
async fn rendering_never_touches_a_provider() {
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

    project.render(RecordFilter::default());
    // Rendering is a pure function of the manifest and its options: no
    // request, no cache probe, nothing ambient.
    assert_eq!(provider.calls().len(), before);
}

#[tokio::test]
async fn rendering_the_same_manifest_twice_produces_the_same_bytes() {
    let project = project_with_two_records().await;
    assert_eq!(
        project.render(RecordFilter::default()),
        project.render(RecordFilter::default())
    );
}

#[tokio::test]
async fn a_filter_narrows_what_is_rendered() {
    let project = project_with_two_records().await;
    assert_eq!(
        project.render(RecordFilter {
            year: Some(2020),
            ..RecordFilter::default()
        }),
        "@article{Zed:2020,title={Second paper}}\n"
    );
}

#[tokio::test]
async fn a_local_filter_renders_only_unrefreshable_owners() {
    let project = project_with_two_records().await;
    add_local_record(&project).await;
    assert_eq!(
        project.render(RecordFilter {
            unrefreshable_providers: Some(project.services.providers.unrefreshable_names()),
            ..RecordFilter::default()
        }),
        "@misc{Mine,title={Mine}}\n"
    );
}

#[tokio::test]
async fn an_empty_selection_renders_zero_bytes() {
    let project = project_with_two_records().await;
    assert_eq!(
        project.render(RecordFilter {
            year: Some(1900),
            ..RecordFilter::default()
        }),
        ""
    );
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

    std::fs::write(&bibliography, project.render(RecordFilter::default())).unwrap();
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
async fn check_accepts_the_filters_the_rendering_was_made_with() {
    let project = project_with_two_records().await;
    add_local_record(&project).await;
    let filter = RecordFilter {
        provider: Some(ProviderName::new("inspire").unwrap()),
        ..RecordFilter::default()
    };
    let subset = project.path("inspire.bib");
    std::fs::write(&subset, project.render(filter.clone())).unwrap();

    // A filtered view matches under the filter it was rendered with. `check`
    // takes `--provider` as a plain filter now that nothing syncs by name.
    let options = RenderOptions {
        filter: filter.clone(),
    };
    assert!(matches!(
        check(&project.store(), &subset, &options).unwrap(),
        CheckOutcome::Match { .. }
    ));
    // And drifts against the unfiltered rendering, which holds one more record.
    assert!(matches!(
        check(&project.store(), &subset, &RenderOptions::default()).unwrap(),
        CheckOutcome::Drift { .. }
    ));
}

#[tokio::test]
async fn a_manifest_change_makes_a_previously_matching_bibliography_drift() {
    let project = project_with_two_records().await;
    let bibliography = project.path("references.bib");
    std::fs::write(&bibliography, project.render(RecordFilter::default())).unwrap();

    let updated = Arc::new(
        FakeProvider::new("inspire")
            .with_refresh(
                "1",
                RefreshState::Metadata(Box::new(provider_metadata("1", Some("r2"), "Corrected"))),
            )
            .with_payload("1", Some(payload("Alpha:2012", "Corrected"))),
    );
    let services = Services::new(Arc::new(ProviderRegistry::new(vec![
        updated,
        Arc::new(LocalProvider::new()),
    ])));
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
