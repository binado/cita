//! `bibi show` and `bibi remove`.

use crate::{
    cli::{RemoveArgs, ShowArgs},
    output,
};
use anyhow::Result;
use bibi_application::{remove, show};
use std::path::Path;

pub fn run_show(target: Option<&Path>, args: ShowArgs) -> Result<bool> {
    let store = crate::bootstrap::store(target)?;
    output::emit(&show(
        &store,
        args.selector.as_deref().expect("stdin resolved"),
    )?)?;
    Ok(false)
}

pub fn run_remove(target: Option<&Path>, args: RemoveArgs) -> Result<bool> {
    let store = crate::bootstrap::store(target)?;
    let report = remove(&store, &args.selectors, args.dry_run)?;
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
    output::note(if args.dry_run {
        format!("dry run: would remove {}", report.results.len())
    } else {
        format!("removed {}", report.results.len())
    });
    Ok(false)
}
