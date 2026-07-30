//! `bibi fetch`.

use crate::{cli::FetchArgs, output};
use anyhow::{Context, Result};
use bibi_application::{FetchOutcome, FetchRequest, Progress, Services, fetch};
use std::path::Path;

pub async fn run_fetch(
    services: &Services,
    target: Option<&Path>,
    args: FetchArgs,
) -> Result<bool> {
    let store = crate::bootstrap::store(target)?;
    let working_directory =
        std::env::current_dir().context("reading the current working directory")?;
    let visible = !args.no_progress && output::color_enabled(&std::io::stderr());

    let report = fetch(
        services,
        &store,
        &FetchRequest {
            selectors: args.selectors,
            source: args.source,
            url: args.url,
            output: args.output,
            force: args.force,
            working_directory,
        },
        |selector| {
            if visible {
                Progress::visible(selector.to_owned())
            } else {
                Progress::silent()
            }
        },
    )
    .await?;

    let mut values = Vec::new();
    for outcome in &report.outcomes {
        match outcome {
            FetchOutcome::Success { target, .. } => {
                values.push(target.as_str().into_owned());
            }
            FetchOutcome::Skipped { selector, first } => {
                output::warn(format!(
                    "skipped `{selector}`: same artifact as earlier selector `{first}`"
                ));
            }
            FetchOutcome::Failure { selector, message } => {
                output::note(format!("error: `{selector}`: {message}"));
            }
        }
    }
    if !values.is_empty() {
        output::emit(&(values.join("\n") + "\n"))?;
    }
    Ok(report.has_failures())
}
