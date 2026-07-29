//! `bibi fetch`.

use crate::{cli::FetchArgs, output};
use anyhow::{Context, Result};
use bibi_application::{FetchRequest, Progress, Services, fetch};
use std::path::Path;

pub async fn run_fetch(
    services: &Services,
    target: Option<&Path>,
    args: FetchArgs,
) -> Result<bool> {
    let store = crate::bootstrap::store(target)?;
    let working_directory =
        std::env::current_dir().context("reading the current working directory")?;

    // Suppressed under the same policy as the table and warnings: a non-tty
    // stderr (or `NO_COLOR`) gets a no-op progress reporter and the pipeline
    // stays quiet. `--no-progress` is an explicit override that forces the
    // silent form even when the runtime policy would otherwise render a bar.
    // `args.selector` doubles as the bar's prefix, since the user already
    // typed it and it is what they are waiting on.
    let mut progress = if args.no_progress || !output::color_enabled(&std::io::stderr()) {
        Progress::silent()
    } else {
        Progress::visible(args.selector.clone())
    };

    let result = fetch(
        services,
        &store,
        &FetchRequest {
            selector: args.selector,
            source: args.source,
            url: args.url,
            output: args.output,
            working_directory,
        },
        &mut progress,
    )
    .await;
    // Clear the bar whether the download succeeded, errored, or was a URL.
    // Idempotent if `progress` was a `silent()` no-op.
    progress.finish();
    let target = result?;

    // The result is one line: a path or a URL, so `open $(bibi fetch k)` works.
    // A successful download is its own signal: the bar cleared at the end of
    // its run. The shell decides what to do with the file; `bibi fetch` does
    // not open or launch anything on its own.
    let value = target.as_str().into_owned();
    output::emit(&format!("{value}\n"))?;
    Ok(false)
}
