//! `bibi sync`.

use crate::{cli::SyncArgs, output};
use anyhow::Result;
use bibi_application::{Services, SyncRequest, sync};
use std::path::Path;

pub async fn run(services: &Services, target: Option<&Path>, args: SyncArgs) -> Result<bool> {
    let store = crate::bootstrap::store(target)?;
    let report = sync(
        services,
        &store,
        &SyncRequest {
            provider: args
                .provider
                .as_deref()
                .map(crate::commands::provider::installed)
                .transpose()?,
            dry_run: args.dry_run,
        },
    )
    .await?;
    if !report.results.is_empty() {
        output::emit(
            &(report
                .results
                .iter()
                .map(|result| result.texkey.as_str())
                .collect::<Vec<_>>()
                .join("\n")
                + "\n"),
        )?;
    }
    let changed = report
        .results
        .iter()
        .filter(|result| result.changed)
        .count();
    output::note(format!(
        "{}resolved {}, changed {changed}, local {}",
        if args.dry_run { "dry run: " } else { "" },
        report.results.len(),
        report.local
    ));
    Ok(false)
}
