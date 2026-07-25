use super::{init, sync_outcome};
use anyhow::{Context, Result, anyhow, bail};
use cita_manifest::{Library, LibraryError, MANIFEST_FILE};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// A resolved operation target: the directory to work in, plus the stable
/// registered name when that directory is a shelf.
///
/// The name is not decoration. A shelf's export is named for its registered
/// name, which can differ from the directory it is registered at, so the name
/// has to survive scope resolution.
pub(crate) struct Target {
    pub(crate) directory: PathBuf,
    name: Option<String>,
}

impl Target {
    /// The default export path for a shelf, or `None` for an ordinary project,
    /// which derives its export name from its own directory instead.
    pub(crate) fn export_path(&self) -> Option<PathBuf> {
        self.name
            .as_deref()
            .map(|name| shelf_export_path(&self.directory, name))
    }
}

/// Resolve `--shelf`, or fall back to the caller's directory so ordinary
/// commands keep discovering their project by walking up from the cwd.
pub(crate) fn resolve_target(cwd: &Path, shelf: Option<&str>) -> Result<Target> {
    let Some(name) = shelf else {
        return Ok(Target {
            directory: cwd.to_path_buf(),
            name: None,
        });
    };
    let library = Library::discover(cwd)?;
    let directory = library
        .shelf_directory(name)
        .map_err(|error| explain_lookup_failure(error, &library))?;
    ensure_direct_shelf(&directory)?;
    Ok(Target {
        directory,
        name: Some(name.to_owned()),
    })
}

/// Name the alternatives when a shelf lookup misses.
///
/// A mistyped `--shelf` is the common failure, and the answer is already in the
/// registry that was just loaded, so listing the registered names beats sending
/// the user to `cita library list`. The hint also names the creating verb,
/// because `--shelf` only ever selects an existing shelf; it never registers
/// one, so a typo cannot silently produce a half-populated shelf.
fn explain_lookup_failure(error: LibraryError, library: &Library) -> anyhow::Error {
    let LibraryError::UnknownShelf(name) = &error else {
        return error.into();
    };
    let registered = library
        .shelves()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let known = if registered.is_empty() {
        "no shelves are registered".to_owned()
    } else {
        format!("registered: {}", registered.join(", "))
    };
    anyhow!("{error}; {known}; create it with `cita library new {name}`")
}

pub(crate) fn init_library(cwd: &Path, path: Option<&Path>) -> Result<()> {
    let directory = resolve_from_caller(cwd, path);
    let existed = directory.join(cita_manifest::LIBRARY_FILE).exists();
    let library = Library::create(&directory)?;
    if existed {
        println!("Already initialized {}", library.path().display());
    } else {
        println!("Initialized {}", library.path().display());
    }
    Ok(())
}

pub(crate) fn list_shelves(cwd: &Path) -> Result<()> {
    let library = Library::discover(cwd)?;
    let name_width = library
        .shelves()
        .keys()
        .map(String::len)
        .max()
        .unwrap_or(0)
        .max("Shelf".len());
    println!("{:<name_width$}  Path", "Shelf");
    for (name, shelf) in library.shelves() {
        println!("{name:<name_width$}  {}", shelf.path().display());
    }
    Ok(())
}

pub(crate) fn init_shelf(cwd: &Path, name: &str, path: Option<&Path>) -> Result<()> {
    let mut library = Library::discover(cwd)?;
    let relative = match library.shelves().get(name) {
        Some(shelf) => {
            if let Some(requested) = path
                && requested != shelf.path()
            {
                bail!(
                    "shelf `{name}` is already registered at {}",
                    shelf.path().display()
                );
            }
            shelf.path().to_path_buf()
        }
        None => path.map_or_else(|| name.into(), Path::to_path_buf),
    };
    library.validate_registration(name, &relative)?;
    let directory = library.root().join(&relative);
    if !directory.exists() {
        fs::create_dir_all(&directory)
            .with_context(|| format!("could not create shelf directory {}", directory.display()))?;
    } else if !directory.is_dir() {
        bail!("shelf path {} is not a directory", directory.display());
    }

    // A shelf is fully initialized and verified before the registry commit.
    init(&directory, None)?;
    let was_registered = library.shelves().contains_key(name);
    library.register(name, &relative)?;
    if was_registered {
        println!("Shelf {name} is registered at {}", relative.display());
    } else {
        println!("Registered shelf {name} at {}", relative.display());
    }
    Ok(())
}

/// A shelf export is named for its stable registered name, which can differ
/// from the directory the shelf is registered at.
fn shelf_export_path(directory: &Path, name: &str) -> PathBuf {
    directory.join(format!("{name}.bib"))
}

/// Run one operation in every registered shelf, in name order.
///
/// A shelf failure is reported and the run continues, so one broken shelf never
/// hides the rest; the returned flag says whether any shelf failed, which the
/// caller turns into the exit status. Library-wide operations are ordered
/// collections of independent shelf mutations, not one crash-atomic transaction,
/// so nothing is rolled back here.
async fn run_batch<F>(cwd: &Path, mut op: F) -> Result<bool>
where
    F: AsyncFnMut(&Target) -> Result<String>,
{
    let library = Library::discover(cwd)?;
    let mut failed = false;
    for (name, shelf) in library.shelves() {
        let target = Target {
            directory: library.root().join(shelf.path()),
            name: Some(name.clone()),
        };
        let outcome = match ensure_direct_shelf(&target.directory) {
            Ok(()) => op(&target).await,
            Err(error) => Err(error),
        };
        match outcome {
            Ok(message) => println!("Shelf {name}: {message}"),
            Err(error) => {
                failed = true;
                print_batch_failure(name, &error);
            }
        }
    }
    Ok(failed)
}

pub(crate) async fn batch_generate(cwd: &Path) -> Result<bool> {
    run_batch(cwd, async |target| {
        let path = super::generate_outcome(&target.directory)?;
        Ok(format!("generated {}", path.display()))
    })
    .await
}

pub(crate) async fn batch_export(cwd: &Path) -> Result<bool> {
    run_batch(cwd, async |target| {
        // A batch export takes no --output, so it always lands on the shelf's
        // own default path and relative resolution never reaches the caller.
        let default = target.export_path();
        let path = super::export_outcome(&target.directory, &target.directory, default.as_deref())?;
        Ok(format!("exported {}", path.display()))
    })
    .await
}

pub(crate) async fn batch_sync(cwd: &Path) -> Result<bool> {
    run_batch(cwd, async |target| {
        Ok(sync_outcome(&target.directory).await?.batch_message())
    })
    .await
}

fn resolve_from_caller(cwd: &Path, path: Option<&Path>) -> std::path::PathBuf {
    match path {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => cwd.join(path),
        None => cwd.to_path_buf(),
    }
}

fn print_batch_failure(name: &str, error: &anyhow::Error) {
    println!("Shelf {name}: failed:");
    for line in format!("{error:#}").lines() {
        println!("  {line}");
    }
}

fn ensure_direct_shelf(directory: &Path) -> Result<()> {
    let manifest = directory.join(MANIFEST_FILE);
    // Follows symlinks intentionally: nothing else in the manifest crate
    // rejects a symlinked cita.toml, so this would be an inconsistent place
    // to start.
    if !manifest.is_file() {
        bail!(
            "registered shelf directory {} does not contain cita.toml",
            directory.display()
        );
    }
    Ok(())
}
