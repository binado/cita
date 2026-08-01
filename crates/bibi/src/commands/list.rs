//! `bibi list`.

use crate::{
    cli::{FilterArgs, ListArgs, ListFormat},
    output,
};
use anyhow::Result;
use bibi_application::domain::RecordFilter;
use bibi_application::{ListRequest, list, render_records, to_json};
use std::path::Path;

pub fn filter(args: &FilterArgs) -> Result<RecordFilter> {
    Ok(RecordFilter {
        provider: args
            .provider
            .as_deref()
            .map(crate::commands::provider::installed)
            .transpose()?,
        local: args.local,
        author: args.author.clone(),
        title: args.title.clone(),
        year: args.year,
    })
}

pub fn run(target: Option<&Path>, args: ListArgs) -> Result<bool> {
    let store = crate::bootstrap::store(target)?;
    let bibliography = store.load()?.bibliography;
    let records = list(
        &bibliography,
        &ListRequest {
            filter: filter(&args.filter)?,
        },
    );
    let rendered = if args.fields.is_empty() {
        match args.format {
            ListFormat::Table => output::table(&records),
            ListFormat::Bibtex => render_records(&records),
            ListFormat::Json => to_json(&records)?,
        }
    } else {
        output::fields(&records, &args.fields)
    };
    output::emit(&rendered)?;
    if records.is_empty() && args.fields.is_empty() && args.format == ListFormat::Table {
        output::note("no records match");
    }
    Ok(false)
}
