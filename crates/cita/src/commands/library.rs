use super::{global_root, sync_outcome};
use anyhow::{Result, anyhow};
use cita_manifest::{
    DEFAULT_SHELF, LIBRARY_FILE, Library, LibraryError, MANIFEST_FILE, Manifest, ShelfLock,
};
use std::path::{Path, PathBuf};

/// One resolved global shelf.
#[derive(Clone)]
pub(crate) struct Target {
    library: Library,
    pub(crate) name: String,
    pub(crate) manifest_path: PathBuf,
}

impl Target {
    pub(crate) fn load(&self) -> Result<Manifest> {
        Ok(Manifest::load(&self.manifest_path)?)
    }

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

pub(crate) fn init_global() -> Result<()> {
    let root = global_root()?;
    let existed = root.join(LIBRARY_FILE).is_file();
    let library = Library::open_or_create(&root)?;
    if existed {
        println!("Already initialized {}", library.path().display());
    } else {
        println!("Initialized {}", library.path().display());
    }
    Ok(())
}

pub(crate) fn resolve_target(shelf: Option<&str>) -> Result<Target> {
    let library = Library::open_or_create(global_root()?)?;
    let name = shelf.unwrap_or(DEFAULT_SHELF);
    let manifest_path = library
        .shelf_manifest(name)
        .map_err(|error| explain_lookup_failure(error, &library))?;
    Ok(Target {
        library,
        name: name.into(),
        manifest_path,
    })
}

fn explain_lookup_failure(error: LibraryError, library: &Library) -> anyhow::Error {
    let LibraryError::UnknownShelf(name) = &error else {
        return error.into();
    };
    let known = library
        .shelves()
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    anyhow!("{error}; registered: {known}; create it with `cita shelf new {name}`")
}

pub(crate) fn new_shelf(name: &str) -> Result<()> {
    let mut library = Library::open_or_create(global_root()?)?;
    if library.create_shelf(name)? {
        println!(
            "Created shelf {name} at {}",
            library.root().join("shelves").join(name).display()
        );
    } else {
        println!(
            "Shelf {name} already exists at {}",
            library.root().join("shelves").join(name).display()
        );
    }
    Ok(())
}

pub(crate) fn list_shelves() -> Result<()> {
    let library = Library::open_or_create(global_root()?)?;
    println!("Shelf  Default");
    for name in library.shelves() {
        println!("{name}{}", if name == DEFAULT_SHELF { "  yes" } else { "" });
    }
    Ok(())
}

async fn run_batch<F>(mut op: F) -> Result<bool>
where
    F: AsyncFnMut(&Target) -> Result<String>,
{
    let library = Library::open_or_create(global_root()?)?;
    let mut failed = false;
    for name in library.shelves() {
        let target = Target {
            manifest_path: library
                .root()
                .join("shelves")
                .join(name)
                .join(MANIFEST_FILE),
            library: library.clone(),
            name: name.clone(),
        };
        match op(&target).await {
            Ok(message) => println!("Shelf {name}: {message}"),
            Err(error) => {
                failed = true;
                println!("Shelf {name}: failed:");
                for line in format!("{error:#}").lines() {
                    println!("  {line}");
                }
            }
        }
    }
    Ok(failed)
}

pub(crate) async fn batch_sync() -> Result<bool> {
    run_batch(async |target| Ok(sync_outcome(target).await?.batch_message())).await
}
