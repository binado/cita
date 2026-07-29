//! `bibi export` and `bibi check`.

use crate::{
    cli::{CheckArgs, ExportArgs},
    output,
};
use anyhow::Result;
use bibi_application::{
    CheckOutcome, ExportRequest, RenderOptions, Services, TargetSelection, check,
    domain::ProviderName, export,
};

pub async fn run_export(
    services: &Services,
    target: &TargetSelection,
    args: ExportArgs,
) -> Result<bool> {
    let store = crate::bootstrap::store(services, target)?;
    let report = export(
        services,
        &store,
        &ExportRequest {
            provider: args
                .provider
                .as_deref()
                .map(ProviderName::new)
                .transpose()?,
            force: args.force,
            output: args.output,
            options: RenderOptions {
                filter: crate::commands::list::render_filter(services, &args.filter),
            },
        },
    )
    .await?;

    if let Some(synced) = &report.sync {
        output::note(format!(
            "synced: {} refreshed, {} unchanged",
            synced.refreshed.len(),
            synced.unchanged
        ));
    }
    // The result is the file; its path is the pipeable answer.
    output::note(format!(
        "wrote {} record(s) to {}",
        report.records,
        report.path.display()
    ));
    Ok(false)
}

pub fn run_check(services: &Services, target: &TargetSelection, args: CheckArgs) -> Result<bool> {
    let store = crate::bootstrap::store(services, target)?;
    let path = crate::bootstrap::resolver(services)?.input(&args.bibfile);
    let outcome = check(
        &store,
        &path,
        &RenderOptions {
            filter: crate::commands::list::render_filter(services, &args.filter),
        },
    )?;
    match &outcome {
        CheckOutcome::Match { path } => {
            output::note(format!("{} matches the manifest", path.display()));
        }
        CheckOutcome::Drift { path, summary } => {
            output::note(format!("{} has drifted from the manifest", path.display()));
            if args.diff {
                output::note(summary);
            }
        }
        CheckOutcome::Missing { path } => {
            output::note(format!("no bibliography at {}", path.display()));
        }
    }
    Ok(outcome.failed())
}
