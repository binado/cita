use super::global_root;
use anyhow::{Context, Result, bail};
use cita_store::{
    DATABASE_FILE, DEFAULT_SHELF, Interchange, Library, LibraryError, ProjectedReference,
    ShelfEntry, ShelfName, SyncCandidate,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// One resolved shelf in the global SQLite library.
pub(crate) struct Target<'a> {
    library: &'a Library,
    name: ShelfName,
}

impl Target<'_> {
    pub(crate) fn name(&self) -> &ShelfName {
        &self.name
    }

    pub(crate) fn library(&self) -> &Library {
        self.library
    }

    pub(crate) fn entries(&self) -> Result<Vec<ShelfEntry>> {
        Ok(self.library.entries(&self.name)?)
    }

    pub(crate) fn projected(&self) -> Result<Vec<ProjectedReference>> {
        Ok(self.library.projected(&self.name)?)
    }

    pub(crate) fn find(&self, selector: &str) -> Result<Option<ProjectedReference>> {
        Ok(self.library.find(&self.name, selector)?)
    }

    pub(crate) fn sync_candidates(&self) -> Result<Vec<SyncCandidate>> {
        Ok(self.library.sync_candidates(Some(&self.name))?)
    }

    pub(crate) fn files_root(&self) -> PathBuf {
        self.library.files_root()
    }

    pub(crate) fn store_root(&self) -> &Path {
        self.library.root()
    }
}

pub(crate) fn open_library() -> Result<Library> {
    Ok(Library::open_or_create(global_root()?)?)
}

pub(crate) fn init_global(caller: &Path, from_file: Option<&Path>, overwrite: bool) -> Result<()> {
    let root = global_root()?;
    let database = root.join(DATABASE_FILE);
    if let Some(input) = from_file {
        let path = if input.is_absolute() {
            input.to_path_buf()
        } else {
            caller.join(input)
        };
        let source = fs::read_to_string(&path)
            .with_context(|| format!("could not read {}", path.display()))?;
        let interchange = match path.extension().and_then(|value| value.to_str()) {
            Some("json") => Interchange::from_json(&source)?,
            Some("toml") => Interchange::from_toml(&source)?,
            _ => bail!(
                "unsupported initialization file {}; expected .json or .toml",
                path.display()
            ),
        };
        if database.exists() && !overwrite {
            bail!(
                "{} already exists; pass --overwrite to replace it",
                database.display()
            );
        }
        let library = Library::open_or_create(&root)?;
        library.replace_from_interchange(&interchange)?;
        println!("Initialized {} from {}", database.display(), path.display());
        return Ok(());
    }
    if overwrite {
        bail!("--overwrite requires --from-file");
    }
    let existed = database.is_file();
    Library::open_or_create(&root)?;
    if existed {
        println!("Already initialized {}", database.display());
    } else {
        println!("Initialized {}", database.display());
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
    library.validate_shelf(name)?;
    Ok(Target {
        library,
        name: name.clone(),
    })
}

fn explain_lookup_failure(error: LibraryError, library: &Library) -> anyhow::Error {
    let LibraryError::UnknownShelf(name) = &error else {
        return error.into();
    };
    let known = library
        .shelves()
        .map(|shelves| {
            shelves
                .iter()
                .map(ShelfName::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|_| "(unavailable)".into());
    anyhow::anyhow!("{error}; registered: {known}; create it with `cita shelf new {name}`")
}

pub(crate) fn new_shelf(library: &Library, name: &str) -> Result<()> {
    let name = ShelfName::try_from(name)?;
    if library.create_shelf(&name)? {
        println!("Created shelf {name}");
    } else {
        println!("Shelf {name} already exists");
    }
    Ok(())
}

pub(crate) fn list_shelves(library: &Library) -> Result<()> {
    const HEADER: &str = "Shelf";
    let shelves = library.shelves()?;
    let width = shelves
        .iter()
        .map(|name| name.as_str().len())
        .chain([HEADER.len()])
        .max()
        .unwrap_or(HEADER.len());
    println!("{HEADER:<width$}  Default");
    for name in shelves {
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

pub(crate) fn report_shelf_failure(name: &ShelfName, error: &anyhow::Error) {
    eprintln!("Shelf {name}: failed:");
    for line in format!("{error:#}").lines() {
        eprintln!("  {line}");
    }
}

pub(crate) fn summarize_batch(failed: usize, total: usize) -> bool {
    if failed > 0 {
        eprintln!("{failed} of {total} shelves failed");
    }
    failed > 0
}
