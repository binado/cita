use super::{generate, init, sync_outcome};
use crate::{FetchArgs, ShelfCommand};
use anyhow::{Context, Result, bail};
use cita_manifest::{Library, MANIFEST_FILE};
use std::{fs, path::Path};

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

pub(crate) async fn run_shelf_command(cwd: &Path, name: &str, command: ShelfCommand) -> Result<()> {
    let library = Library::discover(cwd)?;
    let directory = library.shelf_directory(name)?;
    ensure_direct_shelf(&directory)?;
    match command {
        ShelfCommand::Init { .. } => unreachable!("shelf init is handled before routing"),
        ShelfCommand::Import(args) => super::import(&directory, &args.path, args.overwrite),
        ShelfCommand::Add(args) => {
            super::add(
                &directory,
                args.key.as_deref(),
                &args.locators,
                args.overwrite,
            )
            .await
        }
        ShelfCommand::Sync => super::sync(&directory).await,
        ShelfCommand::Remove(args) => super::remove(&directory, &args.selectors),
        ShelfCommand::List(args) => {
            super::list(&directory, args.sort_by, args.order, !args.no_wrap_title)
        }
        ShelfCommand::Generate => generate(&directory),
        ShelfCommand::Fetch(FetchArgs {
            force,
            cache_only,
            url,
            source,
            open,
            save,
            selector,
        }) => {
            super::fetch(
                &directory,
                &selector,
                super::FetchOptions {
                    force,
                    cache_only,
                    return_url: url,
                    source,
                    open,
                    save,
                },
            )
            .await
        }
        ShelfCommand::Commit => crate::git::commit(&directory.join(MANIFEST_FILE)),
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
                println!("Shelf {name}: failed: {}", one_line(&error));
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
                println!("Shelf {name}: failed: {}", one_line(&error));
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

fn one_line(error: &anyhow::Error) -> String {
    format!("{error:#}").replace(['\n', '\r'], " ")
}

fn ensure_direct_shelf(directory: &Path) -> Result<()> {
    let manifest = directory.join(MANIFEST_FILE);
    if !manifest.is_file() {
        bail!(
            "registered shelf directory {} does not contain cita.toml",
            directory.display()
        );
    }
    Ok(())
}
