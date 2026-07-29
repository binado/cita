//! `bibi fetch` and `bibi cache clean`.

use crate::{
    cli::{CacheCommand, FetchArgs},
    output,
};
use anyhow::{Context, Result};
use bibi_application::{
    FetchRequest, FetchTarget, Services, clean_cache,
    domain::{CleanMode, CleanReport},
    fetch,
};
use std::path::Path;

pub async fn run_fetch(
    services: &Services,
    target: Option<&Path>,
    args: FetchArgs,
) -> Result<bool> {
    let store = crate::bootstrap::store(target)?;
    let target = fetch(
        services,
        &store,
        &FetchRequest {
            selector: args.selector,
            source: args.source,
            url: args.url,
            force: args.force,
        },
    )
    .await?;

    // The result is one line: a path or a URL, so `open $(bibi fetch k)` works.
    let value = target.as_str().into_owned();
    output::emit(&format!("{value}\n"))?;
    match &target {
        FetchTarget::Cached(_) => output::note("from the cache"),
        FetchTarget::Downloaded(_) => output::note("downloaded"),
        FetchTarget::Url(_) => {}
    }
    if args.open {
        // Opening happens only after a successful result, and the target is
        // still written to stdout so the command composes either way.
        opener::open(value.as_str()).with_context(|| format!("opening {value}"))?;
    }
    Ok(false)
}

pub fn run_cache(services: &Services, command: CacheCommand) -> Result<bool> {
    let CacheCommand::Clean(args) = command;
    let mode = if args.all {
        CleanMode::All
    } else {
        CleanMode::DryRun
    };
    let CleanReport {
        root,
        files,
        directories,
        bytes,
        removed,
    } = clean_cache(services, mode)?;
    let verb = if removed { "removed" } else { "would remove" };
    output::note(format!(
        "{verb} {files} file(s) and {directories} director(ies), {:.1} MiB, under {}",
        bytes as f64 / (1024.0 * 1024.0),
        root.display()
    ));
    Ok(false)
}
