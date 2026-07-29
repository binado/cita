//! `bibi list`.

use crate::{
    cli::{FilterArgs, ListArgs, ListFormat, RenderFilterArgs},
    output,
};
use anyhow::Result;
use bibi_application::domain::{ProviderName, RecordFilter};
use bibi_application::{
    ListRequest, Services, TargetSelection, list, render_records, to_json, to_keys,
};

/// Turn filter flags into a complete `RecordFilter`.
///
/// `--local` is answered by asking the registry which providers ingest without
/// refreshing, never by comparing a stored provider name with a literal.
pub fn filter(services: &Services, args: &FilterArgs) -> Result<RecordFilter> {
    Ok(RecordFilter {
        provider: args
            .provider
            .as_deref()
            .map(ProviderName::new)
            .transpose()?,
        unrefreshable_providers: args.local.then(|| services.providers.unrefreshable_names()),
        author: args.author.clone(),
        title: args.title.clone(),
        year: args.year,
    })
}

/// The same, for the narrower set a rendered bibliography accepts.
pub fn render_filter(services: &Services, args: &RenderFilterArgs) -> RecordFilter {
    RecordFilter {
        provider: None,
        unrefreshable_providers: args.local.then(|| services.providers.unrefreshable_names()),
        author: args.author.clone(),
        title: args.title.clone(),
        year: args.year,
    }
}

pub fn run(services: &Services, target: &TargetSelection, args: ListArgs) -> Result<bool> {
    let store = crate::bootstrap::store(services, target)?;
    let records = list(
        &store,
        &ListRequest {
            filter: filter(services, &args.filter)?,
        },
    )?;
    let rendered = match args.format {
        ListFormat::Table => output::table(&records),
        ListFormat::Keys => to_keys(&records),
        ListFormat::Bibtex => render_records(&records)?,
        ListFormat::Json => to_json(&records)?,
    };
    output::emit(&rendered)?;
    if records.is_empty() && args.format == ListFormat::Table {
        output::note("no records match");
    }
    Ok(false)
}
