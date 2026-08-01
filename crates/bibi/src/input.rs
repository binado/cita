//! Resolve omitted positional inputs before constructing services.

use crate::cli::Command;
use std::io::{BufRead, Read};

#[derive(Debug)]
pub enum Error {
    Usage(String),
    Read(std::io::Error),
}

pub fn resolve(
    command: &mut Command,
    stdin_is_terminal: bool,
    mut reader: impl BufRead,
) -> Result<(), Error> {
    match command {
        Command::Add(args) => many(&mut args.locators, stdin_is_terminal, reader),
        Command::Remove(args) => many(&mut args.selectors, stdin_is_terminal, reader),
        Command::Show(args) if args.selector.is_none() => {
            if stdin_is_terminal {
                return Err(missing("locator"));
            }
            let values = lines(reader)?;
            if values.len() != 1 {
                return Err(Error::Usage(format!(
                    "`show` requires exactly one locator, but stdin contained {}",
                    values.len()
                )));
            }
            args.selector = values.into_iter().next();
            Ok(())
        }
        Command::Import(args) if args.file.is_none() => {
            if stdin_is_terminal {
                return Err(Error::Usage(
                    "`import` requires FILE or a redirected BibTeX stream".into(),
                ));
            }
            let mut source = String::new();
            Read::read_to_string(&mut reader, &mut source).map_err(Error::Read)?;
            args.stdin = Some(source);
            Ok(())
        }
        _ => Ok(()),
    }
}

fn many(explicit: &mut Vec<String>, terminal: bool, reader: impl BufRead) -> Result<(), Error> {
    if !explicit.is_empty() {
        return Ok(());
    }
    if terminal {
        return Err(missing("locator"));
    }
    *explicit = lines(reader)?;
    Ok(())
}

fn lines(reader: impl BufRead) -> Result<Vec<String>, Error> {
    reader
        .lines()
        .map(|line| {
            line.map(|value| value.trim().to_owned())
                .map_err(Error::Read)
        })
        .filter(|line| !matches!(line, Ok(value) if value.is_empty()))
        .collect()
}

fn missing(name: &str) -> Error {
    Error::Usage(format!(
        "no {name} was provided; pass one or pipe newline-delimited input"
    ))
}
