//! `bibi fetch`.

use crate::{cli::FetchArgs, output};
use anyhow::{Context, Result};
use bibi_application::{FetchRequest, FetchTarget, Services, fetch};
use std::path::Path;

pub async fn run_fetch(
    services: &Services,
    target: Option<&Path>,
    args: FetchArgs,
) -> Result<bool> {
    let store = crate::bootstrap::store(target)?;
    let working_directory =
        std::env::current_dir().context("reading the current working directory")?;
    // An already-present default destination is a reportable outcome only when
    // the user asked to open the result. Otherwise it is a collision: they
    // asked for a download and did not get one.
    let target = fetch(
        services,
        &store,
        &FetchRequest {
            selector: args.selector,
            source: args.source,
            url: args.url,
            output: args.output,
            working_directory,
        },
        args.open,
    )
    .await?;

    // The result is one line: a path or a URL, so `open $(bibi fetch k)` works.
    let value = target.as_str().into_owned();
    output::emit(&format!("{value}\n"))?;
    match &target {
        FetchTarget::Downloaded(_) => output::note("downloaded"),
        FetchTarget::Present(_) => output::note("already present; not replaced"),
        FetchTarget::Url(_) => {}
    }
    if args.open {
        // Opening happens only after a successful result, and the target is
        // still written to stdout so the command composes either way.
        opener::open(value.as_str()).with_context(|| format!("opening {value}"))?;
    }
    Ok(false)
}
