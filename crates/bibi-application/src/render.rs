//! Pure bibliography rendering.

use bibi_core::{Bibliography, Record, RecordFilter};

/// Rendering inputs besides the aggregate.
#[derive(Clone, Debug, Default)]
pub struct RenderOptions {
    /// Records to include.
    pub filter: RecordFilter,
}

/// Render one filtered bibliography.
pub fn render_bibliography(bibliography: &Bibliography, options: &RenderOptions) -> String {
    render_records(bibliography.filter(&options.filter))
}

/// Render exact entries in caller order.
pub fn render_records<'a>(records: impl IntoIterator<Item = &'a Record>) -> String {
    let entries = records
        .into_iter()
        .map(|record| record.state().bibtex().source())
        .collect::<Vec<_>>();
    bibi_bibtex::render(entries)
}
