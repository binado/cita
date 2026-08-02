//! `bibi add` and `bibi import`.

use crate::{
    cli::{AddArgs, ImportArgs},
    output,
};
use anyhow::{Context, Result};
use bibi_application::{
    AddRequest, Admission, AdmissionKind, ImportRequest, MutationReport, Services, add, import,
};
use std::path::Path;

pub async fn run_add(services: &Services, target: Option<&Path>, args: AddArgs) -> Result<bool> {
    let store = crate::bootstrap::store(target)?;
    let report = add(
        services,
        &store,
        &args.locators,
        &AddRequest {
            provider: args
                .provider
                .as_deref()
                .map(crate::commands::provider::installed)
                .transpose()?,
            overwrite: args.overwrite,
            dry_run: args.dry_run,
        },
    )
    .await?;
    present(&report, args.dry_run)?;
    Ok(false)
}

pub fn run_import(target: Option<&Path>, args: ImportArgs) -> Result<bool> {
    let store = crate::bootstrap::store(target)?;
    let source = match args.file {
        Some(path) => {
            let path = crate::bootstrap::resolver()?.input(&path);
            std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?
        }
        None => args.stdin.expect("stdin was resolved before dispatch"),
    };
    let report = import(
        &store,
        &source,
        &ImportRequest {
            overwrite: args.overwrite,
            dry_run: args.dry_run,
        },
    )?;
    present(&report, args.dry_run)?;
    Ok(false)
}

fn present(report: &MutationReport<Admission>, dry_run: bool) -> Result<()> {
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
    let added = report
        .results
        .iter()
        .filter(|result| result.kind == AdmissionKind::Added)
        .count();
    let overwritten = report.results.len() - added;
    output::note(if dry_run {
        format!("dry run: would add {added}, overwrite {overwritten}")
    } else {
        format!("added {added}, overwrote {overwritten}")
    });
    Ok(())
}
