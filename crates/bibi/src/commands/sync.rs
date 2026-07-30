//! `bibi sync`.

use crate::{cli::SyncArgs, output};
use anyhow::Result;
use bibi_application::{Services, SyncReport, SyncRequest, sync};
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
                .map(|value| crate::commands::provider::installed(value, &services.providers))
                .transpose()?,
            force: args.force,
            dry_run: args.dry_run,
        },
    )
    .await?;
    Ok(present(&report, args.dry_run))
}

/// Sync's result is the summary, so all of it goes to stderr.
///
/// Nothing is written to stdout: the records it touched are already in the
/// manifest, and `list` or `export` is how they are read back.
fn present(report: &SyncReport, dry_run: bool) -> bool {
    for absence in &report.absences {
        output::warn(format!("`{}`: {}", absence.key, absence.reason));
    }
    // An identifier addition is rare and consequential, so each one is named.
    for addition in &report.identifier_additions {
        output::note(format!(
            "`{}` gained {} {}",
            addition.key, addition.kind, addition.value
        ));
    }
    for failure in &report.failures {
        output::note(format!("error: `{}`: {}", failure.item, failure.message));
    }

    let refreshed = report.refreshed.len();
    let mut summary = format!(
        "examined {}, unchanged {}, refreshed {refreshed}",
        report.examined, report.unchanged
    );
    if report.description_changes > 0 {
        summary.push_str(&format!(
            ", {} description change(s)",
            report.description_changes
        ));
    }
    if report.unrefreshable > 0 {
        // Counted so that a growing population of records nothing can refresh
        // stays visible rather than quietly rotting.
        summary.push_str(&format!(", {} unrefreshable", report.unrefreshable));
    }
    if !report.absences.is_empty() {
        summary.push_str(&format!(", {} left unchanged", report.absences.len()));
    }
    if dry_run {
        summary.push_str(" (dry run: nothing written)");
    }
    output::note(summary);
    report.has_failures()
}
