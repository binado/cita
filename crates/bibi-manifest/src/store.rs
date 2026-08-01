//! Reading and atomically publishing `bibi.toml`.

use crate::{Error, atomic::atomic_replace, schema};
use bibi_core::Bibliography;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Exact state observed when a bibliography was read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Generation(State);

#[derive(Clone, Debug, Eq, PartialEq)]
enum State {
    Missing,
    Loaded(String),
}

impl Generation {
    /// True when no file existed at load time.
    pub fn is_missing(&self) -> bool {
        matches!(self.0, State::Missing)
    }
}

/// One loaded bibliography and its exact generation.
#[derive(Debug)]
pub struct LoadedBibliography {
    /// Validated domain aggregate.
    pub bibliography: Bibliography,
    generation: Generation,
}

impl LoadedBibliography {
    /// Generation observed during the load.
    pub fn generation(&self) -> &Generation {
        &self.generation
    }

    /// Split the load into its domain value and generation.
    pub fn into_parts(self) -> (Bibliography, Generation) {
        (self.bibliography, self.generation)
    }
}

/// A schema-1 bibliography file at one explicit path.
#[derive(Clone, Debug)]
pub struct BibliographyStore {
    path: PathBuf,
}

impl BibliographyStore {
    /// Construct a store over one file.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Manifest path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Directory inputs resolve against.
    pub fn directory(&self) -> &Path {
        match self.path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        }
    }

    /// Whether the file exists.
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Load an existing bibliography.
    pub fn load(&self) -> Result<LoadedBibliography, Error> {
        match self.read()? {
            Some(source) => self.decode(source),
            None => Err(Error::NoBibliography {
                path: self.path.clone(),
            }),
        }
    }

    /// Load or return an empty aggregate with a missing generation.
    pub fn load_or_empty(&self) -> Result<LoadedBibliography, Error> {
        match self.read()? {
            Some(source) => self.decode(source),
            None => Ok(LoadedBibliography {
                bibliography: Bibliography::empty(),
                generation: Generation(State::Missing),
            }),
        }
    }

    /// Create an empty bibliography without overwriting anything.
    pub fn create_empty(&self) -> Result<(), Error> {
        if self.path.exists() {
            return Err(Error::AlreadyExists {
                path: self.path.clone(),
            });
        }
        atomic_replace(
            &self.path,
            schema::render(&Bibliography::empty())?.as_bytes(),
        )
    }

    /// Publish one complete validated bibliography if the generation is current.
    pub fn commit(
        &self,
        expected: &Generation,
        bibliography: &Bibliography,
    ) -> Result<Generation, Error> {
        let rendered = schema::render(bibliography)?;
        let current = self.read()?;
        let unchanged = match (&expected.0, &current) {
            (State::Missing, None) => true,
            (State::Loaded(expected), Some(current)) => expected == current,
            _ => false,
        };
        if !unchanged {
            return Err(Error::StaleBibliography {
                path: self.path.clone(),
            });
        }
        atomic_replace(&self.path, rendered.as_bytes())?;
        Ok(Generation(State::Loaded(rendered)))
    }

    fn read(&self) -> Result<Option<String>, Error> {
        match fs::read_to_string(&self.path) {
            Ok(source) => Ok(Some(source)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(Error::io(&self.path, source)),
        }
    }

    fn decode(&self, source: String) -> Result<LoadedBibliography, Error> {
        Ok(LoadedBibliography {
            bibliography: schema::parse(&self.path, &source)?,
            generation: Generation(State::Loaded(source)),
        })
    }
}
