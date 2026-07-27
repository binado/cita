use super::{Target, print_add_outcomes};
use anyhow::{Context, Result};
use cita_bibliography::parse as parse_bibtex;
use cita_manifest::{ConflictPolicy, KeyRequest, PendingReference, SourceSnapshot};
use std::{
    fs,
    io::{self, Read},
    path::Path,
};

pub(crate) fn import(target: &Target, caller: &Path, input: &str, overwrite: bool) -> Result<()> {
    let mut source = String::new();
    if input == "-" {
        io::stdin()
            .read_to_string(&mut source)
            .context("could not read BibTeX from stdin")?;
    } else {
        let path = caller.join(input);
        source = fs::read_to_string(&path)
            .with_context(|| format!("could not read {}", path.display()))?;
    }
    let pending = parse_bibtex(&source)?
        .into_iter()
        .map(|(key, snapshot)| PendingReference {
            key: KeyRequest::Exact(key),
            source: SourceSnapshot::Import(snapshot),
        })
        .collect();
    let policy = if overwrite {
        ConflictPolicy::Overwrite
    } else {
        ConflictPolicy::Skip
    };
    let _lock = target.lock()?;
    let mut manifest = target.load()?;
    let outcomes = manifest.add_batch(pending, policy)?;
    print_add_outcomes(&outcomes);
    Ok(())
}
