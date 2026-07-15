use anyhow::{Context, Result, bail};
use paperdb_core::{Manifest, Paper};
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
        .context("paperdb.toml is not inside a Git repository")?;
    let relative = manifest_path
        .strip_prefix(&root)
        .context("paperdb.toml is not inside the discovered Git repository")?;
    let relative_text = relative.to_string_lossy();
    let current_bytes = fs::read(manifest_path)?;
    let old_bytes = git_output_allow_failure(&root, &["show", &format!("HEAD:{}", relative_text)]);
    if old_bytes.as_deref() == Some(current_bytes.as_slice()) {
        println!("paperdb.toml has no Git changes");
        return Ok(());
    }

    let current = Manifest::load(manifest_path)?;
    let old_manifest;
    let old = if let Some(bytes) = &old_bytes {
        let temporary = tempfile::tempdir()?;
        let path = temporary.path().join("paperdb.toml");
        fs::write(&path, bytes)?;
        old_manifest = Manifest::load(path)?;
        Some(old_manifest.papers())
    } else {
        None
    };
    let (subject, body) = commit_message(old, current.papers());

    run_git(&root, &["add", "--", &relative_text])?;
    let mut args = vec!["commit", "--only", "-m", &subject];
    if !body.is_empty() {
        args.extend(["-m", &body]);
    }
    args.extend(["--", &relative_text]);
    run_git(&root, &args)?;
    println!("Committed paperdb.toml: {subject}");
    Ok(())
}

fn commit_message(old: Option<&[Paper]>, new: &[Paper]) -> (String, String) {
    let Some(old) = old else {
        return (
            "references: initialize paperdb".into(),
            body_lines([], new, []),
        );
    };
    let old_by_key = old
        .iter()
        .map(|paper| (&paper.key, paper))
        .collect::<BTreeMap<_, _>>();
    let new_by_key = new
        .iter()
        .map(|paper| (&paper.key, paper))
        .collect::<BTreeMap<_, _>>();
    let added = new
        .iter()
        .filter(|paper| !old_by_key.contains_key(&paper.key))
        .collect::<Vec<_>>();
    let removed = old
        .iter()
        .filter(|paper| !new_by_key.contains_key(&paper.key))
        .collect::<Vec<_>>();
    let modified = new
        .iter()
        .filter(|paper| old_by_key.get(&paper.key).is_some_and(|old| *old != *paper))
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
    removed: impl IntoIterator<Item = &'a Paper>,
    added: impl IntoIterator<Item = &'a Paper>,
    modified: impl IntoIterator<Item = &'a Paper>,
) -> String {
    removed
        .into_iter()
        .map(|paper| format!("- {} — {}", paper.key, paper.title))
        .chain(
            added
                .into_iter()
                .map(|paper| format!("+ {} — {}", paper.key, paper.title)),
        )
        .chain(
            modified
                .into_iter()
                .map(|paper| format!("~ {} — {}", paper.key, paper.title)),
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
        let a = Paper {
            key: "A".into(),
            title: "Alpha".into(),
            source: "inspire".into(),
            ..Paper::default()
        };
        let b = Paper {
            key: "B".into(),
            title: "Beta".into(),
            source: "inspire".into(),
            ..Paper::default()
        };
        assert_eq!(
            commit_message(None, &[]).0,
            "references: initialize paperdb"
        );
        assert_eq!(
            commit_message(Some(&[]), &[a.clone(), b]).0,
            "references: add 2 papers"
        );
        assert_eq!(
            commit_message(Some(std::slice::from_ref(&a)), &[]).0,
            "references: remove 1 paper"
        );
        assert_eq!(
            commit_message(Some(std::slice::from_ref(&a)), std::slice::from_ref(&a)).0,
            "references: update manifest"
        );
    }
}
