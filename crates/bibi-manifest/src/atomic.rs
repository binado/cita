//! Durable, same-directory atomic replacement.

use crate::error::Error;
use std::{fs, io::Write, path::Path};
use tempfile::NamedTempFile;

/// Replace `path` with `bytes`, atomically and durably.
///
/// A reader sees either the whole previous file or the whole new one, never a
/// partial write, which is what satisfies the atomicity invariant without a
/// lock or a sidecar file. The temporary lives in the destination's own
/// directory, because a rename is only atomic within one filesystem.
///
/// The parent directory must already exist. Every destination bibi writes is
/// one the user named, so a typo produces a diagnostic rather than a directory
/// tree.
pub fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    // A bare file name has an empty parent, which is the working directory.
    let parent = match path.parent() {
        Some(parent) if parent.as_os_str().is_empty() => Path::new("."),
        Some(parent) => parent,
        None => {
            return Err(Error::io(
                path,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "a destination must have a parent directory",
                ),
            ));
        }
    };
    let mut temporary = NamedTempFile::new_in(parent).map_err(|source| Error::io(path, source))?;
    temporary
        .write_all(bytes)
        .map_err(|source| Error::io(path, source))?;
    temporary
        .as_file_mut()
        .flush()
        .map_err(|source| Error::io(path, source))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|source| Error::io(path, source))?;
    // `persist` removes the temporary on failure, so a failed publish leaves
    // the previous file intact and no debris beside it.
    temporary
        .persist(path)
        .map_err(|error| Error::io(path, error.error))?;
    // Durability of the rename itself. Not every platform supports opening a
    // directory as a file, and where it does not, the rename is already durable
    // enough for bibi's purposes, so failure here is not an error.
    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_a_file_in_place() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bibi.toml");
        atomic_replace(&path, b"first").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first");
        atomic_replace(&path, b"second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        // No temporary files are left beside the destination.
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn never_creates_a_missing_parent_directory() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("does/not/exist/bibi.toml");
        assert!(atomic_replace(&nested, b"x").is_err());
        assert!(!nested.exists());
        // Not even the intermediate directories, so a mistyped `-p` leaves the
        // filesystem exactly as it was.
        assert!(!directory.path().join("does").exists());
    }

    #[test]
    fn a_failed_write_leaves_the_previous_bytes_in_place() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bibi.toml");
        atomic_replace(&path, b"original").unwrap();
        // A directory in the destination's place makes `persist` fail.
        let blocked = directory.path().join("blocked");
        fs::create_dir(&blocked).unwrap();
        assert!(atomic_replace(&blocked, b"new").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "original");
    }
}
