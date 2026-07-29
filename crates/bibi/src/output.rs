//! stdout carries the result; stderr carries everything meant for a human.

use anstyle::{AnsiColor, Style};
use anyhow::Context;
use bibi_application::domain::Record;
use bibi_application::{BatchReport, ItemFailure, SkippedItem};
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
