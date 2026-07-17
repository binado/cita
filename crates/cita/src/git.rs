use anyhow::{Context, Result, bail};
use cita_core::ReferenceSource;
use cita_manifest::{BIBLIOGRAPHY_FILE, Manifest, StoredReference};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub fn repository_root(path: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["-C", path.to_str()?, "rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
}

pub fn commit(manifest_path: &Path) -> Result<()> {
    let current = Manifest::load_verified(manifest_path)?;
    let root = repository_root(manifest_path.parent().unwrap_or_else(|| Path::new(".")))
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
    let old_manifest_bytes =
        git_output_allow_failure(&root, &["show", &format!("HEAD:{manifest_relative}")]);
    let old_bib_bytes =
        git_output_allow_failure(&root, &["show", &format!("HEAD:{bibliography_relative}")]);
    if old_manifest_bytes.as_deref() == Some(manifest_bytes.as_slice())
        && old_bib_bytes.as_deref() == Some(bib_bytes.as_slice())
    {
        println!("Cita references have no Git changes");
        return Ok(());
    }
    let (subject, body) = match old_manifest_bytes
        .as_deref()
        .and_then(parse_committed_manifest)
    {
        Some(old) => commit_message(Some(old.references()), current.references()),
        None if old_manifest_bytes.is_none() => commit_message(None, current.references()),
        None => ("references: update bibliography".into(), String::new()),
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

fn parse_committed_manifest(bytes: &[u8]) -> Option<Manifest> {
    let directory = tempfile::tempdir().ok()?;
    let path = directory.path().join("cita.toml");
    fs::write(&path, bytes).ok()?;
    Manifest::load(path).ok()
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
    if count == 1 { "paper" } else { "papers" }
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
        .unwrap_or_else(|_| "unknown title".into())
}
fn git_output_allow_failure(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}
fn run_git(root: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .with_context(|| format!("could not run git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
    Ok(())
}
