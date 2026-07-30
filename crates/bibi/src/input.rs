//! Optional newline-delimited command inputs.
//!
//! Argument parsing establishes the command shape. This module then resolves
//! an omitted positional from standard input before any provider, store, or
//! network collaborator is constructed.

use crate::cli::Command;
use std::io::BufRead;

/// A failure while resolving optional standard input.
#[derive(Debug)]
pub enum Error {
    /// The command did not receive the cardinality its interface requires.
    Usage(String),
    /// Standard input could not be read.
    Read(std::io::Error),
}

/// Fill omitted command positionals from newline-delimited standard input.
///
/// Explicit positionals always win and leave `reader` untouched. An omitted
/// positional on a terminal is a usage mistake; a redirected empty stream is
/// valid for batch commands and deliberately means there is no work.
pub fn resolve(
    command: &mut Command,
    stdin_is_terminal: bool,
    reader: impl BufRead,
) -> Result<(), Error> {
    match command {
        Command::Add(args) if args.file.is_none() => {
            resolve_many(&mut args.locators, stdin_is_terminal, reader, "locator")?;
            if args.key.is_some() && args.locators.len() != 1 {
                return Err(Error::Usage(
                    "`add --key` requires exactly one locator".to_owned(),
                ));
            }
        }
        Command::Fetch(args) => {
            resolve_many(&mut args.selectors, stdin_is_terminal, reader, "selector")?;
        }
        Command::Remove(args) => {
            resolve_many(&mut args.selectors, stdin_is_terminal, reader, "selector")?;
        }
        Command::Show(args) if args.selector.is_none() => {
            if stdin_is_terminal {
                return Err(missing("selector"));
            }
            let values = read_lines(reader)?;
            if values.len() != 1 {
                return Err(Error::Usage(format!(
                    "`show` requires exactly one selector, but standard input contained {}",
                    values.len()
                )));
            }
            args.selector = values.into_iter().next();
        }
        _ => {}
    }
    Ok(())
}

fn resolve_many(
    explicit: &mut Vec<String>,
    stdin_is_terminal: bool,
    reader: impl BufRead,
    name: &str,
) -> Result<(), Error> {
    if !explicit.is_empty() {
        return Ok(());
    }
    if stdin_is_terminal {
        return Err(missing(name));
    }
    *explicit = read_lines(reader)?;
    Ok(())
}

fn missing(name: &str) -> Error {
    Error::Usage(format!(
        "no {name} was provided; pass one as an argument or pipe newline-delimited input"
    ))
}

fn read_lines(reader: impl BufRead) -> Result<Vec<String>, Error> {
    reader
        .lines()
        .map(|line| {
            line.map(|value| value.trim().to_owned())
                .map_err(Error::Read)
        })
        .filter_map(|line| match line {
            Ok(value) if value.is_empty() => None,
            other => Some(other),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{AddArgs, FetchArgs, RemoveArgs, ShowArgs};
    use std::{
        io::{self, Cursor, Read},
        path::PathBuf,
    };

    fn add(locators: &[&str]) -> Command {
        Command::Add(AddArgs {
            locators: locators.iter().map(ToString::to_string).collect(),
            file: None,
            key: None,
            provider: None,
            overwrite: false,
            dry_run: false,
        })
    }

    fn fetch(selectors: &[&str]) -> Command {
        Command::Fetch(FetchArgs {
            selectors: selectors.iter().map(ToString::to_string).collect(),
            source: false,
            url: false,
            output: None,
            force: false,
            no_progress: false,
        })
    }

    #[test]
    fn explicit_many_input_wins_without_reading_stdin() {
        struct PanicReader;
        impl Read for PanicReader {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                panic!("explicit input must leave stdin untouched")
            }
        }
        impl io::BufRead for PanicReader {
            fn fill_buf(&mut self) -> io::Result<&[u8]> {
                panic!("explicit input must leave stdin untouched")
            }
            fn consume(&mut self, _: usize) {}
        }

        let mut command = add(&["1207.7214"]);
        resolve(&mut command, false, PanicReader).unwrap();
        let Command::Add(args) = command else {
            unreachable!()
        };
        assert_eq!(args.locators, ["1207.7214"]);
    }

    #[test]
    fn redirected_lines_are_trimmed_and_blanks_are_ignored() {
        let mut command = fetch(&[]);
        resolve(
            &mut command,
            false,
            Cursor::new("  first  \n\n\t\n second\n"),
        )
        .unwrap();
        let Command::Fetch(args) = command else {
            unreachable!()
        };
        assert_eq!(args.selectors, ["first", "second"]);
    }

    #[test]
    fn an_empty_redirected_stream_is_a_valid_empty_batch() {
        let mut command = Command::Remove(RemoveArgs {
            selectors: Vec::new(),
            dry_run: false,
        });
        resolve(&mut command, false, Cursor::new(" \n")).unwrap();
        let Command::Remove(args) = command else {
            unreachable!()
        };
        assert!(args.selectors.is_empty());
    }

    #[test]
    fn a_missing_interactive_input_is_a_usage_error() {
        let mut command = fetch(&[]);
        assert!(matches!(
            resolve(&mut command, true, Cursor::new("ignored")),
            Err(Error::Usage(_))
        ));
    }

    #[test]
    fn read_failures_are_preserved_as_io_errors() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("broken pipe"))
            }
        }
        impl io::BufRead for Broken {
            fn fill_buf(&mut self) -> io::Result<&[u8]> {
                Err(io::Error::other("broken pipe"))
            }
            fn consume(&mut self, _: usize) {}
        }

        let mut command = add(&[]);
        assert!(matches!(
            resolve(&mut command, false, Broken),
            Err(Error::Read(_))
        ));
    }

    #[test]
    fn show_accepts_exactly_one_redirected_selector() {
        let mut command = Command::Show(ShowArgs { selector: None });
        resolve(&mut command, false, Cursor::new("\n key \n")).unwrap();
        let Command::Show(args) = command else {
            unreachable!()
        };
        assert_eq!(args.selector.as_deref(), Some("key"));
    }

    #[test]
    fn show_rejects_empty_and_multiple_redirected_selectors() {
        for input in ["", "one\ntwo\n"] {
            let mut command = Command::Show(ShowArgs { selector: None });
            assert!(matches!(
                resolve(&mut command, false, Cursor::new(input)),
                Err(Error::Usage(_))
            ));
        }
    }

    #[test]
    fn add_file_mode_never_reads_implicit_locators() {
        let mut command = add(&[]);
        let Command::Add(args) = &mut command else {
            unreachable!()
        };
        args.file = Some(PathBuf::from("-"));
        resolve(&mut command, true, Cursor::new("@article{x}\n")).unwrap();
        let Command::Add(args) = command else {
            unreachable!()
        };
        assert!(args.locators.is_empty());
    }

    #[test]
    fn add_key_requires_one_resolved_locator() {
        for input in ["", "one\ntwo\n"] {
            let mut command = add(&[]);
            let Command::Add(args) = &mut command else {
                unreachable!()
            };
            args.key = Some("mine".to_owned());
            assert!(matches!(
                resolve(&mut command, false, Cursor::new(input)),
                Err(Error::Usage(_))
            ));
        }
    }
}
