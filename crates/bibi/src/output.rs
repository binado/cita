//! stdout carries the result; stderr carries everything meant for a human.

use crate::cli::Field;
use anstyle::{AnsiColor, Style};
use anyhow::Context;
use bibi_application::domain::Record;
use bibi_application::{BatchReport, ItemFailure, SkippedItem};
use bibi_documents::{ArtifactKind, public_url};
use std::io::{IsTerminal, Write};

/// Write a command's result to stdout.
///
/// A broken pipe is not an error: `bibi list | head` closes the pipe on
/// purpose, and reporting that as a failure would be noise on every use of the
/// tool in a shell pipeline.
pub fn emit(text: &str) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    match write(&mut stdout, text) {
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => quiet_exit(),
        result => result.context("writing stdout"),
    }
}

fn write(writer: &mut impl Write, text: &str) -> std::io::Result<()> {
    writer.write_all(text.as_bytes())?;
    writer.flush()
}

fn quiet_exit() -> ! {
    // The reader is gone; nothing further can be written, including a message
    // about being unable to write.
    std::process::exit(0);
}

/// Write a diagnostic to stderr.
pub fn note(message: impl std::fmt::Display) {
    let _ = writeln!(std::io::stderr(), "{message}");
}

/// Write a warning to stderr, marked.
pub fn warn(message: impl std::fmt::Display) {
    let style = if std::io::stderr().is_terminal() {
        Style::new().fg_color(Some(AnsiColor::Yellow.into()))
    } else {
        Style::new()
    };
    let _ = writeln!(std::io::stderr(), "{style}warning:{style:#} {message}");
}

/// Report the skips and failures of a batch, in input order.
pub fn report<T>(report: &BatchReport<T>) {
    for SkippedItem { item, reason, .. } in &report.skipped {
        warn(format!("skipped `{item}`: {reason}"));
    }
    for ItemFailure { item, message } in &report.failures {
        note(format!("error: `{item}`: {message}"));
    }
}

/// Render a listing as an aligned table.
///
/// Description is display data and nothing else (I7), which is what makes it
/// safe to truncate a title to fit a terminal: no rendered bibliography is
/// affected by anything decided here.
pub fn table(records: &[Record]) -> String {
    if records.is_empty() {
        return String::new();
    }
    let key_width = records
        .iter()
        .map(|record| record.key.as_str().chars().count())
        .max()
        .unwrap_or(0);
    let provider_width = records
        .iter()
        .map(|record| record.provenance.provider.as_str().chars().count())
        .max()
        .unwrap_or(0);
    let terminal = terminal_size::terminal_size()
        .map(|(terminal_size::Width(width), _)| usize::from(width))
        .unwrap_or(100);
    // key + year + provider + three separators, then whatever is left.
    let title_width = terminal
        .saturating_sub(key_width + provider_width + 4 + 3)
        .max(20);

    let mut rendered = String::new();
    for record in records {
        let year = record
            .description
            .year
            .map(|year| year.to_string())
            .unwrap_or_else(|| "    ".to_owned());
        rendered.push_str(&format!(
            "{:<key_width$} {:<4} {:<provider_width$} {}\n",
            record.key.as_str(),
            year,
            record.provenance.provider.as_str(),
            truncate(&record.description.title, title_width),
        ));
    }
    rendered
}

/// Render one tab-separated line per record, in the order the fields were given.
///
/// Line-oriented output for a pipeline, so an absent value is an empty column
/// rather than a skipped line: dropping a line would desynchronize the output
/// from the records it describes, and every `paste`, `cut`, or `xargs` reading
/// it would silently pair the wrong values. With one field this is a bare
/// column, which is what makes `--fields key` a drop-in for a shell loop.
///
/// Nothing here is truncated or aligned. That is the difference between this
/// and [`table`]: a table is for a person looking at a terminal, and these
/// bytes are for another program.
pub fn fields(records: &[Record], fields: &[Field]) -> anyhow::Result<String> {
    let mut rendered = String::new();
    for record in records {
        for (at, field) in fields.iter().enumerate() {
            if at > 0 {
                rendered.push('\t');
            }
            rendered.push_str(&value(record, *field)?);
        }
        rendered.push('\n');
    }
    Ok(rendered)
}

/// One field of one record, absent rendering as the empty string.
fn value(record: &Record, field: Field) -> anyhow::Result<String> {
    let rendered = match field {
        Field::Key => record.key.to_string(),
        Field::Title => record.description.title.clone(),
        Field::Year => record
            .description
            .year
            .map(|year| year.to_string())
            .unwrap_or_default(),
        Field::Provider => record.provenance.provider.to_string(),
        Field::Doi => record
            .identifiers
            .doi
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
        Field::Arxiv => record
            .identifiers
            .arxiv
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
        // The URL `fetch --url` would print, derived rather than stored. Pure:
        // it builds an address and retrieves nothing.
        Field::ArxivUrl => match &record.identifiers.arxiv {
            None => String::new(),
            Some(arxiv) => public_url(arxiv, ArtifactKind::Pdf)
                .context("building an arXiv URL")?
                .to_string(),
        },
    };
    Ok(rendered)
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_owned();
    }
    let kept = width.saturating_sub(1);
    value.chars().take(kept).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("sink failed"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn non_broken_write_errors_are_returned() {
        let error = write(&mut FailingWriter, "result").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Other);
    }
}
