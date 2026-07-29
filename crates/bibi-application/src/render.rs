//! The one entry point that turns records into a bibliography.

use crate::error::Error;
use bibi_core::{Record, RecordFilter};
use bibi_manifest::Manifest;

/// What a rendering depends on, besides the manifest.
///
/// Rendering is a pure function of these two things (I6): no network, no cache
/// probing, no clock, no working directory. Two runs with the same manifest and
/// the same options produce the same bytes.
#[derive(Clone, Debug, Default)]
pub struct RenderOptions {
    /// Which records to include.
    pub filter: RecordFilter,
}

/// Render a filtered bibliography from a manifest.
///
/// `export`, `check`, and `list --format bibtex` all come through here, so the
/// question "which records, in what order" is answered in one place. The bytes
/// themselves are produced by [`render_records`], which is the only caller of
/// the BibTeX renderer, so separator and trailing-newline rules cannot drift
/// between stdout and a written file.
pub fn render_manifest(manifest: &Manifest, options: &RenderOptions) -> Result<String, Error> {
    let selected = manifest.filter(&options.filter).collect::<Vec<_>>();
    render_records(selected)
}

/// Render exactly these records, in the order given.
pub fn render_records<'a>(records: impl IntoIterator<Item = &'a Record>) -> Result<String, Error> {
    let entries = records
        .into_iter()
        .map(|record| (&record.key, &record.payload))
        .collect::<Vec<_>>();
    Ok(bibi_bibtex::render(entries)?)
}
