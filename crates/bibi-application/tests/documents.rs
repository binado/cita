//! What `fetch` decides before it ever reaches the network.
//!
//! Downloading itself is covered in `bibi-documents`. What belongs here is the
//! branching: which artifact was asked for, where it would land, and whether an
//! occupied destination is an outcome or a collision. None of these paths build
//! a client, which is what makes them testable without a server.

use bibi_application::{
    AddRequest, FetchRequest, FetchTarget, Services, add_locators, domain::ManifestStore, fetch,
};
use bibi_core::{ArxivId, Identifiers};
use bibi_provider::{
    LocalProvider, Provider, ProviderRegistry,
    testing::{FakeProvider, provider_record},
};
use std::{path::PathBuf, sync::Arc};
use tempfile::TempDir;

struct Project {
    directory: TempDir,
    services: Services,
}

impl Project {
    fn store(&self) -> ManifestStore {
        ManifestStore::new(self.directory.path().join("bibi.toml"))
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }

    /// A request against the project directory, as the CLI builds one.
    fn request(&self, selector: &str) -> FetchRequest {
        FetchRequest {
            selector: selector.to_owned(),
            working_directory: self.directory.path().to_path_buf(),
            ..FetchRequest::default()
        }
    }
}

/// One INSPIRE-owned record carrying an arXiv identifier, and one local record
/// carrying none — the ordinary case of an unpublished note.
async fn project() -> Project {
    let mut managed = provider_record("inspire", "1", "Aad:2012tfa", "Observation");
    managed.identifiers = Identifiers {
        arxiv: Some(ArxivId::new("1207.7214").unwrap()),
        doi: None,
    };
    let providers: Vec<Arc<dyn Provider>> = vec![
        Arc::new(FakeProvider::new("inspire").with_record("arxiv:1207.7214", managed)),
        Arc::new(LocalProvider::new()),
    ];
    let directory = tempfile::tempdir().unwrap();
    let project = Project {
        directory,
        services: Services::new(Arc::new(ProviderRegistry::new(providers))),
    };
    add_locators(
        &project.services,
        &project.store(),
        &["1207.7214".to_owned()],
        &AddRequest::default(),
    )
    .await
    .unwrap();

    let notes = project.path("notes.bib");
    std::fs::write(&notes, "@unpublished{Roe:2026,title={Lecture notes}}\n").unwrap();
    bibi_application::add_file(
        &project.services,
        &project.store(),
        &bibi_application::AddFileRequest {
            source: bibi_application::InputSource::Path(notes),
            provider: None,
            overwrite: false,
            force_local: true,
            dry_run: false,
        },
    )
    .await
    .unwrap();
    project
}

#[tokio::test]
async fn a_url_is_answered_without_building_a_client() {
    let project = project().await;

    let pdf = fetch(
        &project.services,
        &project.store(),
        &FetchRequest {
            url: true,
            ..project.request("Aad:2012tfa")
        },
        false,
    )
    .await
    .unwrap();
    assert_eq!(pdf.as_str(), "https://arxiv.org/pdf/1207.7214");

    // `--source --url` names the other artifact, which the previous `fetch`
    // refused to combine at all.
    let source = fetch(
        &project.services,
        &project.store(),
        &FetchRequest {
            url: true,
            source: true,
            ..project.request("Aad:2012tfa")
        },
        false,
    )
    .await
    .unwrap();
    assert_eq!(source.as_str(), "https://arxiv.org/e-print/1207.7214");
}

#[tokio::test]
async fn an_already_present_default_destination_is_an_outcome_when_opening() {
    let project = project().await;
    let destination = project.path("1207.7214.pdf");
    std::fs::write(&destination, "%PDF-1.7\nearlier").unwrap();

    // `--open` asks for the file to be in front of the user, and it already is.
    let target = fetch(
        &project.services,
        &project.store(),
        &project.request("Aad:2012tfa"),
        true,
    )
    .await
    .unwrap();

    let FetchTarget::Present(path) = &target else {
        panic!("expected Present, got {target:?}");
    };
    assert_eq!(path, &destination);
    // Reported, never replaced, and no request was made to say so.
    assert_eq!(
        std::fs::read_to_string(&destination).unwrap(),
        "%PDF-1.7\nearlier"
    );
}

#[tokio::test]
async fn the_same_destination_without_opening_is_a_collision() {
    let project = project().await;
    let destination = project.path("1207.7214.pdf");
    std::fs::write(&destination, "mine").unwrap();

    // A plain `fetch` asked for a download and did not get one.
    let error = fetch(
        &project.services,
        &project.store(),
        &project.request("Aad:2012tfa"),
        false,
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("already exists"));
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "mine");
}

#[tokio::test]
async fn an_explicit_output_is_never_accepted_as_already_present() {
    let project = project().await;
    let named = project.path("higgs.pdf");
    std::fs::write(&named, "mine").unwrap();

    // Even under `--open`: the caller named this path, so treating whatever is
    // there as the artifact they wanted would be a guess.
    let error = fetch(
        &project.services,
        &project.store(),
        &FetchRequest {
            output: Some(PathBuf::from("higgs.pdf")),
            ..project.request("Aad:2012tfa")
        },
        true,
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("already exists"));
    assert_eq!(std::fs::read_to_string(&named).unwrap(), "mine");
}

#[tokio::test]
async fn a_record_without_an_arxiv_identifier_says_so_rather_than_guessing() {
    let project = project().await;

    let error = fetch(
        &project.services,
        &project.store(),
        &FetchRequest {
            url: true,
            ..project.request("Roe:2026")
        },
        false,
    )
    .await
    .unwrap_err();

    // No DOI resolver, no publisher page: arXiv is the only source.
    assert!(error.to_string().contains("no arXiv identifier"));
}
