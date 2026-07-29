//! `bibi list`.

use crate::{
    cli::{FilterArgs, ListArgs, ListFormat},
    output,
};
use anyhow::Result;
use bibi_application::domain::{ProviderName, RecordFilter};
use bibi_application::{
    ListRequest, Services, TargetSelection, list, render_records, to_json, to_keys,
};

/// Turn filter flags into a `RecordFilter`.
///
/// `--local` is not part of the filter here: it is a question about provider
/// capability, which the application answers by asking the registry.
pub fn filter(args: &FilterArgs) -> Result<RecordFilter> {
    Ok(RecordFilter {
        provider: args
            .provider
            .as_deref()
            .map(ProviderName::new)
            .transpose()?,
        unrefreshable_providers: None,
        author: args.author.clone(),
        title: args.title.clone(),
        year: args.year,
    })
}

pub fn run(services: &Services, target: &TargetSelection, args: ListArgs) -> Result<bool> {
    let store = crate::bootstrap::store(services, target)?;
    let records = list(
        services,
        &store,
        &ListRequest {
            filter: filter(&args.filter)?,
            local: args.filter.local,
        },
    )?;
    let rendered = match args.format {
        ListFormat::Table => output::table(&records),
        ListFormat::Keys => to_keys(&records),
        ListFormat::Bibtex => render_records(&records)?,
        ListFormat::Json => to_json(&records)?,
    };
    output::emit(&rendered);
    if records.is_empty() && args.format == ListFormat::Table {
        output::note("no records match");
    }
    Ok(false)
}
