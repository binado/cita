//! `bibi list`.

use crate::{
    cli::{FilterArgs, ListArgs, ListFormat},
    commands::provider,
    output,
};
use anyhow::Result;
use bibi_application::domain::{Manifest, RecordFilter};
use bibi_application::{ListRequest, Services, list, render_records, to_json};
use std::path::Path;

/// Turn filter flags into a complete `RecordFilter`.
///
/// `--provider` is checked against the providers this build carries plus those
/// the manifest already names, so a name nothing knows is reported rather than
/// silently matching no records. `--local` is answered by asking the registry
/// which providers ingest without refreshing, never by comparing a stored
/// provider name with a literal.
pub fn filter(services: &Services, manifest: &Manifest, args: &FilterArgs) -> Result<RecordFilter> {
    Ok(RecordFilter {
        provider: args
            .provider
            .as_deref()
            .map(|value| provider::selectable(value, &services.providers, manifest))
            .transpose()?,
        unrefreshable_providers: args.local.then(|| services.providers.unrefreshable_names()),
        author: args.author.clone(),
        title: args.title.clone(),
        year: args.year,
    })
}

pub fn run(services: &Services, target: Option<&Path>, args: ListArgs) -> Result<bool> {
    let store = crate::bootstrap::store(target)?;
    let manifest = store.load()?.manifest;
    let records = list(
        &manifest,
        &ListRequest {
            filter: filter(services, &manifest, &args.filter)?,
        },
    );
    let rendered = if args.fields.is_empty() {
        match args.format {
            ListFormat::Table => output::table(&records),
            ListFormat::Bibtex => render_records(&records)?,
            ListFormat::Json => to_json(&records)?,
        }
    } else {
        output::fields(&records, &args.fields)?
    };
    output::emit(&rendered)?;
    // Only the table says so. Empty `--fields` output is exactly what a
    // pipeline over an empty selection should look like.
    if records.is_empty() && args.fields.is_empty() && args.format == ListFormat::Table {
        output::note("no records match");
    }
    Ok(false)
}
