use super::{find_manifest, print_add_outcomes};
use anyhow::{Context, Result};
use cita_bibliography::parse as parse_bibtex;
use cita_manifest::{ConflictPolicy, KeyRequest, Manifest, PendingReference, SourceSnapshot};
use std::{
    fs,
    io::{self, Read},
    path::Path,
};

pub(crate) fn import(cwd: &Path, input: &str, overwrite: bool) -> Result<()> {
    let mut source = String::new();
    if input == "-" {
        io::stdin()
            .read_to_string(&mut source)
            .context("could not read BibTeX from stdin")?;
    } else {
        source = fs::read_to_string(input).with_context(|| format!("could not read {input}"))?;
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
    let mut manifest = Manifest::load_verified(find_manifest(cwd)?)?;
    let outcomes = manifest.add_batch(pending, policy)?;
    print_add_outcomes(&outcomes);
    Ok(())
}
