//! Golden fixtures for the two output boundaries that are not the manifest.
//!
//! The manifest's own bytes are pinned by `bibi-manifest`'s golden. These two
//! are the other things bibi produces from one manifest: a rendered
//! bibliography, and the metadata projection. Both must be reproducible from
//! the same input and options alone.

use bibi_application::{
    AddFileRequest, InputSource, ListRequest, RenderOptions, Services, add_file,
    domain::ManifestStore, list, render_manifest, to_json,
};
use bibi_provider::{LocalProvider, ProviderRegistry};
use std::sync::Arc;

const BIBLIOGRAPHY: &str = include_str!("golden/bibliography.bib");
const LISTING: &str = include_str!("golden/list.json");

/// The source entries, deliberately in the wrong order and awkwardly formatted.
const SOURCE: &str = "@unpublished{zed:2026,\n  title = {Notes on nothing},\n  author = {Roe, Richard},\n  year = 2026\n}\n\n@software{alpha:2024,\n  title   =   {A tool},\n  author  =   {Doe, Jane and Roe, Richard},\n  year    =   2024,\n  doi     =   {10.5281/ZENODO.1}\n}\n";

async fn project() -> (tempfile::TempDir, Services, ManifestStore) {
    let directory = tempfile::tempdir().unwrap();
    let services = Services::new(Arc::new(ProviderRegistry::new(vec![Arc::new(
        LocalProvider::new(),
    )])));
    let store = ManifestStore::new(directory.path().join("bibi.toml"));
    let source = directory.path().join("source.bib");
    std::fs::write(&source, SOURCE).unwrap();
    add_file(
        &services,
        &store,
        &AddFileRequest {
            source: InputSource::Path(source),
            provider: None,
            overwrite: false,
            force_local: true,
            dry_run: false,
        },
    )
    .await
    .unwrap();
    (directory, services, store)
}

#[tokio::test]
async fn a_rendered_bibliography_matches_its_golden() {
    let (_directory, _services, store) = project().await;
    let manifest = store.load().unwrap().manifest;
    let rendered = render_manifest(&manifest, &RenderOptions::default()).unwrap();
    assert_eq!(rendered, BIBLIOGRAPHY);

    // Only the local key and the payload participate: records are sorted by
    // key, entries keep their own bytes, and nothing about the description
    // reaches the output.
    assert!(rendered.starts_with("@software{alpha:2024,"));
    assert!(rendered.contains("title   =   {A tool}"));
    assert_eq!(rendered.matches("\n\n").count(), 1);
    assert!(rendered.ends_with("}\n"));
}

#[tokio::test]
async fn the_json_projection_matches_its_golden() {
    let (_directory, _services, store) = project().await;
    let records = list(&store.load().unwrap().manifest, &ListRequest::default());
    // Every field but the id is a function of the manifest. The id is minted,
    // and deliberately so — identity is bibi's own, not derived from content —
    // so the golden pins its shape rather than a value that cannot recur.
    assert_eq!(mask_ids(&to_json(&records).unwrap()), LISTING);
}

/// Replace each canonical UUID with a placeholder.
fn mask_ids(json: &str) -> String {
    let mut masked = String::with_capacity(json.len());
    let mut rest = json;
    while let Some(at) = rest.find("\"id\": \"") {
        let value = at + "\"id\": \"".len();
        masked.push_str(&rest[..value]);
        masked.push_str("<minted>");
        rest = &rest[value + 36..];
    }
    masked.push_str(rest);
    masked
}

#[tokio::test]
async fn both_outputs_are_stable_across_repeated_runs() {
    let (_directory, _services, store) = project().await;
    let manifest = store.load().unwrap().manifest;
    let records = list(&store.load().unwrap().manifest, &ListRequest::default());
    for _ in 0..3 {
        let reloaded = store.load().unwrap().manifest;
        assert_eq!(
            render_manifest(&reloaded, &RenderOptions::default()).unwrap(),
            render_manifest(&manifest, &RenderOptions::default()).unwrap()
        );
        assert_eq!(
            to_json(&list(
                &store.load().unwrap().manifest,
                &ListRequest::default()
            ))
            .unwrap(),
            to_json(&records).unwrap()
        );
        // Reloading does not re-mint: identity survives every read.
        assert!(
            to_json(&records)
                .unwrap()
                .contains(records[0].id.to_string().as_str())
        );
    }
}
