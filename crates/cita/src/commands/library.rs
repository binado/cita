use super::{generate, init, sync_outcome};
use crate::{AddArgs, FetchArgs, ImportArgs, ListArgs, RemoveArgs};
use anyhow::{Context, Result, bail};
use cita_manifest::{Library, MANIFEST_FILE};
use std::{fs, path::Path};

/// A routable shelf command: every `ShelfCommand` variant except `Init`,
/// which `main.rs` handles before routing here.
pub(crate) enum ShelfAction {
    Import(ImportArgs),
    Add(AddArgs),
    Sync,
    Remove(RemoveArgs),
    List(ListArgs),
    Generate,
    Fetch(FetchArgs),
    Commit,
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

pub(crate) async fn run_shelf_command(cwd: &Path, name: &str, action: ShelfAction) -> Result<()> {
    let library = Library::discover(cwd)?;
    let directory = library.shelf_directory(name)?;
    ensure_direct_shelf(&directory)?;
    match action {
        ShelfAction::Import(args) => super::import(&directory, &args.path, args.overwrite),
        ShelfAction::Add(args) => {
            super::add(
                &directory,
                args.key.as_deref(),
                &args.locators,
                args.overwrite,
            )
            .await
        }
        ShelfAction::Sync => super::sync(&directory).await,
        ShelfAction::Remove(args) => super::remove(&directory, &args.selectors),
        ShelfAction::List(args) => {
            super::list(&directory, args.sort_by, args.order, !args.no_wrap_title)
        }
        ShelfAction::Generate => generate(&directory),
        ShelfAction::Fetch(args) => {
            let (selector, options) = args.into_options();
            super::fetch(&directory, &selector, options).await
        }
        ShelfAction::Commit => crate::git::commit(&directory.join(MANIFEST_FILE)),
    }
}

pub(crate) fn batch_generate(cwd: &Path) -> Result<bool> {
    let library = Library::discover(cwd)?;
    let mut failed = false;
    for (name, shelf) in library.shelves() {
        let directory = library.root().join(shelf.path());
        let outcome =
            ensure_direct_shelf(&directory).and_then(|()| super::generate_outcome(&directory));
        match outcome {
            Ok(path) => println!("Shelf {name}: generated {}", path.display()),
            Err(error) => {
                failed = true;
                print_batch_failure(name, &error);
            }
        }
    }
    Ok(failed)
}

pub(crate) async fn batch_sync(cwd: &Path) -> Result<bool> {
    let library = Library::discover(cwd)?;
    let mut failed = false;
    for (name, shelf) in library.shelves() {
        let directory = library.root().join(shelf.path());
        let outcome = match ensure_direct_shelf(&directory) {
            Ok(()) => sync_outcome(&directory).await,
            Err(error) => Err(error),
        };
        match outcome {
            Ok(outcome) => println!("Shelf {name}: {}", outcome.batch_message()),
            Err(error) => {
                failed = true;
                print_batch_failure(name, &error);
            }
        }
    }
    Ok(failed)
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
