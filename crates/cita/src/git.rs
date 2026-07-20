use anyhow::{Context, Result, bail};
use cita_core::ReferenceSource;
use cita_manifest::{BIBLIOGRAPHY_FILE, Manifest, SourceSnapshot};
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
    if output.status.success() {
        return Ok(Some(PathBuf::from(
            String::from_utf8_lossy(&output.stdout).trim(),
        )));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() == Some(128)
        && is_not_repository_diagnostic(&stderr)
        && !has_git_marker(path)?
    {
        return Ok(None);
    }
    bail!("git rev-parse --show-toplevel failed: {}", stderr.trim())
}

fn is_not_repository_diagnostic(stderr: &str) -> bool {
    stderr
        .trim_start()
        .starts_with("fatal: not a git repository")
}

fn has_git_marker(path: &Path) -> Result<bool> {
    for ancestor in path.ancestors() {
        let marker = ancestor.join(".git");
        match fs::symlink_metadata(&marker) {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("could not inspect {}", marker.display()));
            }
        }
    }
    Ok(false)
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
    // Managed paths must start at HEAD so rollback can restore tracked entries
    // and remove newly staged ones without disturbing unrelated index state.
    ensure_managed_files_unstaged(
        &root,
        &[manifest_relative.as_ref(), bibliography_relative.as_ref()],
    )?;
    let manifest_bytes = fs::read(manifest_path)?;
    let bib_bytes = fs::read(&bibliography_path)?;
    let head_exists = head_exists(&root)?;
    let old_manifest_bytes = committed_bytes(&root, &manifest_relative, head_exists)?;
    let old_bib_bytes = committed_bytes(&root, &bibliography_relative, head_exists)?;
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
    let manifest_in_head = old_manifest_bytes.is_some();
    let bibliography_in_head = old_bib_bytes.is_some();
    if let Err(error) = run_git(
        &root,
        &[
            "add",
            "-f",
            "--",
            &manifest_relative,
            &bibliography_relative,
        ],
    ) {
        return Err(rollback_error(
            &root,
            &manifest_relative,
            manifest_in_head,
            &bibliography_relative,
            bibliography_in_head,
            error,
        ));
    }
    let mut args = vec!["commit", "--only", "-m", &subject];
    if !body.is_empty() {
        args.extend(["-m", &body]);
    }
    args.extend(["--", &manifest_relative, &bibliography_relative]);
    if let Err(error) = run_git(&root, &args) {
        return Err(rollback_error(
            &root,
            &manifest_relative,
            manifest_in_head,
            &bibliography_relative,
            bibliography_in_head,
            error,
        ));
    }
    println!("Committed Cita references: {subject}");
    Ok(())
}

/// Bytes of `HEAD:relative`, after HEAD and exact path existence have been
/// established separately. This keeps a damaged object database distinct from
/// an unborn branch or a path that was not present in the commit.
fn committed_bytes(root: &Path, relative: &str, head_exists: bool) -> Result<Option<Vec<u8>>> {
    if !head_exists || !path_exists_in_head(root, relative)? {
        return Ok(None);
    }
    let reference = format!("HEAD:{relative}");
    let output = git_output(root, &["show", &reference])?;
    if output.status.success() {
        return Ok(Some(output.stdout));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!("git show {reference} failed: {}", stderr.trim())
}

fn head_exists(root: &Path) -> Result<bool> {
    let output = git_output(root, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) if output.stderr.is_empty() => Ok(false),
        _ => bail!(
            "git rev-parse --verify HEAD failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

fn path_exists_in_head(root: &Path, relative: &str) -> Result<bool> {
    let output = git_output(
        root,
        &[
            "ls-tree",
            "--full-name",
            "--name-only",
            "-z",
            "HEAD",
            "--",
            relative,
        ],
    )?;
    if !output.status.success() {
        bail!(
            "git ls-tree HEAD -- {relative} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    if output.stdout.is_empty() {
        return Ok(false);
    }
    let mut expected = relative.as_bytes().to_vec();
    expected.push(0);
    if output.stdout == expected {
        Ok(true)
    } else {
        bail!("git ls-tree returned an unexpected path for `{relative}`")
    }
}

fn ensure_managed_files_unstaged(root: &Path, paths: &[&str]) -> Result<()> {
    let mut args = vec!["diff", "--cached", "--quiet", "--"];
    args.extend_from_slice(paths);
    let output = git_output(root, &args)?;
    match output.status.code() {
        Some(0) => Ok(()),
        Some(1) => bail!(
            "cita.toml or references.bib already has staged changes; commit or unstage the managed files before running `cita commit`"
        ),
        _ => bail!(
            "git diff --cached failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

fn rollback_error(
    root: &Path,
    manifest: &str,
    manifest_in_head: bool,
    bibliography: &str,
    bibliography_in_head: bool,
    original: anyhow::Error,
) -> anyhow::Error {
    match rollback_index(
        root,
        manifest,
        manifest_in_head,
        bibliography,
        bibliography_in_head,
    ) {
        Ok(()) => original,
        Err(cleanup) => anyhow::anyhow!(
            "{original:#}; additionally failed to restore Cita's index entries: {cleanup:#}"
        ),
    }
}

fn rollback_index(
    root: &Path,
    manifest: &str,
    manifest_in_head: bool,
    bibliography: &str,
    bibliography_in_head: bool,
) -> Result<()> {
    let existing = [
        (manifest, manifest_in_head),
        (bibliography, bibliography_in_head),
    ]
    .into_iter()
    .filter_map(|(path, exists)| exists.then_some(path))
    .collect::<Vec<_>>();
    let added = [
        (manifest, manifest_in_head),
        (bibliography, bibliography_in_head),
    ]
    .into_iter()
    .filter_map(|(path, exists)| (!exists).then_some(path))
    .collect::<Vec<_>>();
    let mut failures = Vec::new();
    if !existing.is_empty() {
        let mut args = vec!["reset", "--quiet", "HEAD", "--"];
        args.extend(existing);
        if let Err(error) = run_git(root, &args) {
            failures.push(error);
        }
    }
    if !added.is_empty() {
        let mut args = vec!["rm", "--cached", "-f", "--ignore-unmatch", "--"];
        args.extend(added);
        if let Err(error) = run_git(root, &args) {
            failures.push(error);
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        bail!(
            "{}",
            failures
                .into_iter()
                .map(|error| format!("{error:#}"))
                .collect::<Vec<_>>()
                .join("; ")
        )
    }
}

fn parse_committed_manifest(bytes: &[u8]) -> Result<Manifest> {
    let directory = tempfile::tempdir().context("could not create a temporary directory")?;
    let path = directory.path().join("cita.toml");
    fs::write(&path, bytes).context("could not stage the HEAD manifest")?;
    Ok(Manifest::load(path)?)
}

fn commit_message(
    old: Option<&BTreeMap<String, SourceSnapshot>>,
    new: &BTreeMap<String, SourceSnapshot>,
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
    removed: impl IntoIterator<Item = (&'a String, &'a SourceSnapshot)>,
    added: impl IntoIterator<Item = (&'a String, &'a SourceSnapshot)>,
    modified: impl IntoIterator<Item = (&'a String, &'a SourceSnapshot)>,
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
fn title(item: &SourceSnapshot) -> String {
    item.project()
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

#[cfg(test)]
mod tests {
    use super::is_not_repository_diagnostic;

    #[test]
    fn classifies_ordinary_not_repository_diagnostic() {
        assert!(is_not_repository_diagnostic(
            "fatal: not a git repository (or any of the parent directories): .git\n"
        ));
    }

    #[test]
    fn classifies_mount_point_not_repository_diagnostic() {
        assert!(is_not_repository_diagnostic(
            "fatal: not a git repository (or any parent up to mount point /work)\n\
             Stopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set).\n"
        ));
    }
}
