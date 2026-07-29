//! `bibi add`.

use crate::{cli::AddArgs, output};
use anyhow::{Result, bail};
use bibi_application::domain::{CitationKey, ProviderName};
use bibi_application::{
    AddFileRequest, AddKind, AddReport, AddRequest, InputSource, Services, add_file, add_locators,
};
use std::path::Path;

pub async fn run(services: &Services, target: Option<&Path>, args: AddArgs) -> Result<bool> {
    let store = crate::bootstrap::store(target)?;
    let provider = args
        .provider
        .as_deref()
        .map(ProviderName::new)
        .transpose()?;

    let report = match &args.file {
        Some(path) => {
            if !args.locators.is_empty() {
                bail!("`add -f` reads entries from a file, so it takes no locators");
            }
            let source = if path == Path::new("-") {
                InputSource::Stdin
            } else {
                InputSource::Path(crate::bootstrap::resolver()?.input(path))
            };
            add_file(
                services,
                &store,
                &AddFileRequest {
                    source,
                    provider,
                    overwrite: args.overwrite,
                    force_local: args.force_local,
                    dry_run: args.dry_run,
                },
            )
            .await?
        }
        None => {
            if args.locators.is_empty() {
                bail!("give at least one locator, or `-f <file>` to read entries from a file");
            }
            add_locators(
                services,
                &store,
                &args.locators,
                &AddRequest {
                    key: args.key.as_deref().map(CitationKey::new).transpose()?,
                    provider,
                    overwrite: args.overwrite,
                    dry_run: args.dry_run,
                },
            )
            .await?
        }
    };
    present(&report, args.dry_run)
}

/// Write the BibTeX to stdout and the explanations to stderr.
///
/// Every item bibi was asked about produces an entry on stdout — added,
/// overwritten, or skipped — so the output is the complete set of entries the
/// command concerns, usable as input to something else.
fn present(report: &AddReport, dry_run: bool) -> Result<bool> {
    let mut entries = Vec::new();
    for record in &report.items.successes {
        entries.push(record.bibtex.clone());
    }
    for skipped in &report.items.skipped {
        if let Some(bibtex) = &skipped.bibtex {
            entries.push(bibtex.clone());
        }
    }
    if !entries.is_empty() {
        output::emit(&(entries.join("\n\n") + "\n"))?;
    }
    output::report(&report.items);

    let added = report
        .items
        .successes
        .iter()
        .filter(|record| record.kind == AddKind::Added)
        .count();
    let overwritten = report.items.successes.len() - added;
    if dry_run {
        output::note(format!(
            "dry run: would add {added}, overwrite {overwritten}, skip {}",
            report.items.skipped.len()
        ));
    } else if report.committed {
        output::note(format!("added {added}, overwrote {overwritten}"));
    }
    Ok(report.items.has_failures())
}
