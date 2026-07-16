use anyhow::{Context, Result, bail};
use cita_core::PaperRecord;
use cita_manifest::Manifest;
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
    let root = repository_root(manifest_path.parent().unwrap_or_else(|| Path::new(".")))
        .context("cita.toml is not inside a Git repository")?;
    let relative = manifest_path
        .strip_prefix(&root)
        .context("cita.toml is not inside the discovered Git repository")?;
    let relative_text = relative.to_string_lossy();
    let current_bytes = fs::read(manifest_path)?;
    let old_bytes = git_output_allow_failure(&root, &["show", &format!("HEAD:{relative_text}")]);
    if old_bytes.as_deref() == Some(current_bytes.as_slice()) {
        println!("cita.toml has no Git changes");
        return Ok(());
    }

    let current = Manifest::load(manifest_path)?;
    let (subject, body) = if let Some(bytes) = &old_bytes {
        match parse_committed_manifest(bytes)? {
            Some(old_manifest) => commit_message(Some(old_manifest.papers()), current.papers()),
            // The committed manifest predates the current format (or is
            // otherwise unreadable); commit with a generic message rather
            // than refusing to commit the fixed file.
            None => ("references: update bibliography".into(), String::new()),
        }
    } else {
        commit_message(None, current.papers())
    };

    run_git(&root, &["add", "--", &relative_text])?;
    let mut args = vec!["commit", "--only", "-m", &subject];
    if !body.is_empty() {
        args.extend(["-m", &body]);
    }
    args.extend(["--", &relative_text]);
    run_git(&root, &args)?;
    println!("Committed cita.toml: {subject}");
    Ok(())
}

/// Parse the manifest bytes committed at HEAD; `None` means the blob exists
/// but is not a readable current-format manifest.
fn parse_committed_manifest(bytes: &[u8]) -> Result<Option<Manifest>> {
    let temporary = tempfile::tempdir()?;
    let path = temporary.path().join("cita.toml");
    fs::write(&path, bytes)?;
    Ok(Manifest::load(path).ok())
}

fn commit_message(
    old: Option<&BTreeMap<String, PaperRecord>>,
    new: &BTreeMap<String, PaperRecord>,
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
        .filter(|(key, record)| old.get(*key).is_some_and(|old| old != *record))
        .collect::<Vec<_>>();
    let subject = match (added.len(), removed.len(), modified.len()) {
        (count, 0, 0) if count > 0 => format!("references: add {count} {}", plural(count)),
        (0, count, 0) if count > 0 => format!("references: remove {count} {}", plural(count)),
        (0, 0, 0) => "references: update manifest".into(),
        _ => "references: update bibliography".into(),
    };
    (subject, body_lines(removed, added, modified))
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "paper" } else { "papers" }
}

fn body_lines<'a>(
    removed: impl IntoIterator<Item = (&'a String, &'a PaperRecord)>,
    added: impl IntoIterator<Item = (&'a String, &'a PaperRecord)>,
    modified: impl IntoIterator<Item = (&'a String, &'a PaperRecord)>,
) -> String {
    removed
        .into_iter()
        .map(|(key, record)| format!("- {key} — {}", record.title))
        .chain(
            added
                .into_iter()
                .map(|(key, record)| format!("+ {key} — {}", record.title)),
        )
        .chain(
            modified
                .into_iter()
                .map(|(key, record)| format!("~ {key} — {}", record.title)),
        )
        .collect::<Vec<_>>()
        .join("\n")
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
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_specific_messages() {
        fn record(title: &str) -> PaperRecord {
            PaperRecord {
                title: title.into(),
                source: "inspire".into(),
                ..PaperRecord::default()
            }
        }
        let empty = BTreeMap::new();
        let just_a = BTreeMap::from([("A".to_string(), record("Alpha"))]);
        let a_and_b = BTreeMap::from([
            ("A".to_string(), record("Alpha")),
            ("B".to_string(), record("Beta")),
        ]);
        assert_eq!(
            commit_message(None, &empty).0,
            "references: initialize cita"
        );
        assert_eq!(
            commit_message(Some(&empty), &a_and_b).0,
            "references: add 2 papers"
        );
        assert_eq!(
            commit_message(Some(&just_a), &empty).0,
            "references: remove 1 paper"
        );
        assert_eq!(
            commit_message(Some(&just_a), &just_a).0,
            "references: update manifest"
        );
    }
}
