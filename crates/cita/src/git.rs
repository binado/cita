use anyhow::{Context, Result, bail};
use cita_core::ReferenceSource;
use cita_manifest::{BIBLIOGRAPHY_FILE, Manifest, StoredReference};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

/// `Ok(None)` means git itself reported that `path` is outside any worktree;
/// failing to run git at all is an error for the caller to interpret.
pub fn repository_root(path: &Path) -> Result<Option<PathBuf>> {
    let output = git_output(path, &["rev-parse", "--show-toplevel"])?;
    Ok(output
        .status
        .success()
        .then(|| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())))
}

pub fn commit(manifest_path: &Path) -> Result<()> {
    let current = Manifest::load_verified(manifest_path)?;
    let root = repository_root(manifest_path.parent().unwrap_or_else(|| Path::new(".")))?
        .context("cita.toml is not inside a Git repository")?;
    let manifest_relative = manifest_path
        .strip_prefix(&root)
        .context("cita.toml is outside the Git root")?
        .to_string_lossy();
    let bibliography_path = manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(BIBLIOGRAPHY_FILE);
    let bibliography_relative = bibliography_path
        .strip_prefix(&root)
        .context("references.bib is outside the Git root")?
        .to_string_lossy();
    let manifest_bytes = fs::read(manifest_path)?;
    let bib_bytes = fs::read(&bibliography_path)?;
    let old_manifest_bytes = committed_bytes(&root, &manifest_relative)?;
    let old_bib_bytes = committed_bytes(&root, &bibliography_relative)?;
    if old_manifest_bytes.as_deref() == Some(manifest_bytes.as_slice())
        && old_bib_bytes.as_deref() == Some(bib_bytes.as_slice())
    {
        println!("Cita references have no Git changes");
        return Ok(());
    }
    let (subject, body) = match old_manifest_bytes.as_deref() {
        None => commit_message(None, current.references()),
        Some(bytes) => match parse_committed_manifest(bytes) {
            Ok(old) => commit_message(Some(old.references()), current.references()),
            Err(error) => {
                eprintln!(
                    "warning: the HEAD cita.toml is unreadable ({error:#}); using a generic commit message"
                );
                ("references: update bibliography".into(), String::new())
            }
        },
    };
    run_git(
        &root,
        &[
            "add",
            "-f",
            "--",
            &manifest_relative,
            &bibliography_relative,
        ],
    )?;
    let mut args = vec!["commit", "--only", "-m", &subject];
    if !body.is_empty() {
        args.extend(["-m", &body]);
    }
    args.extend(["--", &manifest_relative, &bibliography_relative]);
    run_git(&root, &args)?;
    println!("Committed Cita references: {subject}");
    Ok(())
}

/// Bytes of `HEAD:relative`, or `None` when git reports that the path or
/// HEAD itself does not exist yet. Any other failure is an error so a broken
/// repository is never mistaken for a first commit.
fn committed_bytes(root: &Path, relative: &str) -> Result<Option<Vec<u8>>> {
    const ABSENT_MARKERS: [&str; 4] = [
        "does not exist in",
        "exists on disk, but not in",
        "invalid object name",
        "unknown revision or path not in the working tree",
    ];
    let reference = format!("HEAD:{relative}");
    let output = git_output(root, &["show", &reference])?;
    if output.status.success() {
        return Ok(Some(output.stdout));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if ABSENT_MARKERS.iter().any(|marker| stderr.contains(marker)) {
        return Ok(None);
    }
    bail!("git show {reference} failed: {}", stderr.trim())
}

fn parse_committed_manifest(bytes: &[u8]) -> Result<Manifest> {
    let directory = tempfile::tempdir().context("could not create a temporary directory")?;
    let path = directory.path().join("cita.toml");
    fs::write(&path, bytes).context("could not stage the HEAD manifest")?;
    Ok(Manifest::load(path)?)
}

fn commit_message(
    old: Option<&BTreeMap<String, StoredReference>>,
    new: &BTreeMap<String, StoredReference>,
) -> (String, String) {
    let Some(old) = old else {
        return (
            "references: initialize cita".into(),
            body_lines([], new, []),
        );
    };
    let added = new
        .iter()
        .filter(|(key, _)| !old.contains_key(*key))
        .collect::<Vec<_>>();
    let removed = old
        .iter()
        .filter(|(key, _)| !new.contains_key(*key))
        .collect::<Vec<_>>();
    let modified = new
        .iter()
        .filter(|(key, value)| old.get(*key).is_some_and(|old| old != *value))
        .collect::<Vec<_>>();
    let subject = match (added.len(), removed.len(), modified.len()) {
        (count, 0, 0) if count > 0 => format!("references: add {count} {}", plural(count)),
        (0, count, 0) if count > 0 => format!("references: remove {count} {}", plural(count)),
        _ => "references: update bibliography".into(),
    };
    (subject, body_lines(removed, added, modified))
}
fn plural(count: usize) -> &'static str {
    if count == 1 {
        "reference"
    } else {
        "references"
    }
}
fn body_lines<'a>(
    removed: impl IntoIterator<Item = (&'a String, &'a StoredReference)>,
    added: impl IntoIterator<Item = (&'a String, &'a StoredReference)>,
    modified: impl IntoIterator<Item = (&'a String, &'a StoredReference)>,
) -> String {
    removed
        .into_iter()
        .map(|(key, item)| format!("- {key} — {}", title(item)))
        .chain(
            added
                .into_iter()
                .map(|(key, item)| format!("+ {key} — {}", title(item))),
        )
        .chain(
            modified
                .into_iter()
                .map(|(key, item)| format!("~ {key} — {}", title(item))),
        )
        .collect::<Vec<_>>()
        .join("\n")
}
fn title(item: &StoredReference) -> String {
    item.source
        .project()
        .map(|reference| reference.title)
        .unwrap_or_else(|error| format!("unknown title ({error})"))
}
fn git_output(root: &Path, args: &[&str]) -> Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .with_context(|| format!("could not run git {}", args.join(" ")))
}
fn run_git(root: &Path, args: &[&str]) -> Result<()> {
    let output = git_output(root, args)?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
    Ok(())
}
