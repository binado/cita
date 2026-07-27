use super::{global_root, sync_outcome};
use anyhow::{Result, anyhow};
use cita_manifest::{
    DEFAULT_SHELF, LIBRARY_FILE, Library, LibraryError, Manifest, ShelfLock, ShelfName,
};
use std::path::{Path, PathBuf};

/// One resolved global shelf.
///
/// The three fields correspond: `name` is registered in `library`, and
/// `manifest_path` is that shelf's `shelf.toml` under `library`'s root, verified at
/// construction to exist as direct (non-symlink) entries. Construction only happens
/// through [`target_in`], and the fields are private, so the correspondence cannot
/// be broken from outside this module. Borrowing the library rather than cloning it
/// keeps every target in a command consistent with the registry snapshot it came from.
pub(crate) struct Target<'a> {
    library: &'a Library,
    name: ShelfName,
    manifest_path: PathBuf,
}

impl Target<'_> {
    pub(crate) fn name(&self) -> &ShelfName {
        &self.name
    }

    pub(crate) fn load(&self) -> Result<Manifest> {
        Ok(Manifest::load(&self.manifest_path)?)
    }

    /// Acquire this shelf's advisory lock.
    ///
    /// Load the manifest through [`ShelfLock::manifest`] rather than
    /// [`Self::load`] so the mutation is provably paired with the lock covering it.
    pub(crate) fn lock(&self) -> Result<ShelfLock> {
        Ok(self.library.lock_shelf(&self.name)?)
    }

    pub(crate) fn files_root(&self) -> PathBuf {
        self.library.files_root()
    }

    pub(crate) fn store_root(&self) -> &Path {
        self.library.root()
    }
}

/// Open, and lazily create, the one global library.
pub(crate) fn open_library() -> Result<Library> {
    Ok(Library::open_or_create(global_root()?)?)
}

pub(crate) fn init_global() -> Result<()> {
    let root = global_root()?;
    let existed = root.join(LIBRARY_FILE).is_file();
    let repaired = existed && !root.join("shelves").join(DEFAULT_SHELF).is_dir();
    let library = Library::open_or_create(&root)?;
    let path = library.path().display().to_string();
    match (existed, repaired) {
        (false, _) => println!("Initialized {path}"),
        (true, true) => println!("Repaired {path}: recreated the `{DEFAULT_SHELF}` shelf"),
        (true, false) => println!("Already initialized {path}"),
    }
    Ok(())
}

pub(crate) fn resolve_target<'a>(library: &'a Library, shelf: Option<&str>) -> Result<Target<'a>> {
    let name = ShelfName::try_from(shelf.unwrap_or(DEFAULT_SHELF))?;
    target_in(library, &name).map_err(|error| explain_lookup_failure(error, library))
}

pub(crate) fn target_in<'a>(
    library: &'a Library,
    name: &ShelfName,
) -> Result<Target<'a>, LibraryError> {
    Ok(Target {
        manifest_path: library.shelf_manifest(name)?,
        library,
        name: name.clone(),
    })
}

/// Name the registered shelves and the command that creates one.
///
/// The hint names the creating verb because `--shelf` only ever *selects*: a typo
/// must not silently produce a new, half-populated shelf.
fn explain_lookup_failure(error: LibraryError, library: &Library) -> anyhow::Error {
    let LibraryError::UnknownShelf(name) = &error else {
        return error.into();
    };
    let known = library
        .shelves()
        .iter()
        .map(ShelfName::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    anyhow!("{error}; registered: {known}; create it with `cita shelf new {name}`")
}

pub(crate) fn new_shelf(library: &mut Library, name: &str) -> Result<()> {
    let name = ShelfName::try_from(name)?;
    let directory = library.root().join("shelves").join(name.as_str());
    if library.create_shelf(&name)? {
        println!("Created shelf {name} at {}", directory.display());
    } else {
        println!("Shelf {name} already exists at {}", directory.display());
    }
    Ok(())
}

pub(crate) fn list_shelves(library: &Library) -> Result<()> {
    const HEADER: &str = "Shelf";
    let width = library
        .shelves()
        .iter()
        .map(|name| name.as_str().len())
        .chain([HEADER.len()])
        .max()
        .unwrap_or(HEADER.len());
    println!("{HEADER:<width$}  Default");
    for name in library.shelves() {
        let default = if name.as_str() == DEFAULT_SHELF {
            "yes"
        } else {
            ""
        };
        println!(
            "{}",
            format!("{:<width$}  {default}", name.as_str()).trim_end()
        );
    }
    Ok(())
}

/// Run one operation against every shelf, in name order.
///
/// A library-wide operation is an ordered collection of independent shelf
/// mutations, not one crash-atomic transaction: a failure part-way through leaves
/// the shelves already processed updated, and nothing is rolled back.
///
/// Successes go to stdout and failures to stderr, so redirecting stdout to a file
/// still shows the user what went wrong. Returns whether any shelf failed; the
/// caller turns that into the exit status.
async fn run_batch<F>(library: &Library, mut op: F) -> Result<bool>
where
    F: AsyncFnMut(&Target<'_>) -> Result<String>,
{
    let mut failed = 0usize;
    for name in library.shelves() {
        let outcome = match target_in(library, name) {
            Ok(target) => op(&target).await,
            Err(error) => Err(error.into()),
        };
        match outcome {
            Ok(message) => println!("Shelf {name}: {message}"),
            Err(error) => {
                failed += 1;
                report_shelf_failure(name, &error);
            }
        }
    }
    Ok(summarize_batch(failed, library.shelves().len()))
}

/// Print one shelf's failure to stderr, indenting the cause under its heading.
pub(crate) fn report_shelf_failure(name: &ShelfName, error: &anyhow::Error) {
    eprintln!("Shelf {name}: failed:");
    for line in format!("{error:#}").lines() {
        eprintln!("  {line}");
    }
}

/// Report the batch total on stderr and return whether anything failed.
pub(crate) fn summarize_batch(failed: usize, total: usize) -> bool {
    if failed > 0 {
        eprintln!("{failed} of {total} shelves failed");
    }
    failed > 0
}

pub(crate) async fn batch_sync(library: &Library) -> Result<bool> {
    run_batch(library, async |target| {
        Ok(sync_outcome(target).await?.batch_message())
    })
    .await
}
