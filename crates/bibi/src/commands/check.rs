//! `bibi check`.

use crate::{cli::CheckArgs, output};
use anyhow::Result;
use bibi_application::{CheckOutcome, RenderOptions, check};
use std::path::Path;

pub fn run(target: Option<&Path>, args: CheckArgs) -> Result<bool> {
    let store = crate::bootstrap::store(target)?;
    let bibliography = store.load()?.bibliography;
    let path = crate::bootstrap::resolver()?.input(&args.bibfile);
    let outcome = check(
        &bibliography,
        &path,
        &RenderOptions {
            filter: crate::commands::list::filter(&args.filter)?,
        },
    )?;
    match &outcome {
        CheckOutcome::Match { path } => {
            output::note(format!("{} matches the bibliography", path.display()));
        }
        CheckOutcome::Drift { path, summary } => {
            output::note(format!(
                "{} has drifted from the bibliography",
                path.display()
            ));
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
