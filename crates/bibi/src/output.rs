//! stdout carries the result; stderr carries everything meant for a human.

use crate::cli::Field;
use anstyle::{AnsiColor, Style};
use anyhow::Context;
use bibi_application::domain::Record;
use bibi_application::{BatchReport, ItemFailure, SkippedItem};
use bibi_documents::{ArtifactKind, public_url};
use std::io::{IsTerminal, Write};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The variable that turns styling off, per <https://no-color.org>.
const NO_COLOR_ENV: &str = "NO_COLOR";

/// Whether to style what is written to `stream`.
///
/// `NO_COLOR` set to any non-empty value disables styling, as does a stream
/// that is not a terminal. *All* styling goes, not only colour: a bold header
/// is as unwanted in a pipeline as a coloured one.
pub fn color_enabled(stream: &impl IsTerminal) -> bool {
    let suppressed =
        std::env::var_os(NO_COLOR_ENV).is_some_and(|value| !value.as_encoded_bytes().is_empty());
    !suppressed && stream.is_terminal()
}

/// The terminal's width, or a reasonable assumption when there is no terminal.
///
/// Ambient, which is why it is resolved here at the edge and handed to
/// [`table`] rather than read inside it. The layout is then a function of its
/// arguments alone and can be tested at a fixed width — which matters more than
/// it looks, because `terminal_size` falls back to *stdin*, so a layout test
/// would otherwise pick up the width of whatever terminal ran `cargo test`.
pub fn terminal_width() -> usize {
    terminal_size::terminal_size()
        .map(|(terminal_size::Width(width), _)| usize::from(width))
        .unwrap_or(100)
}

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
    let style = if color_enabled(&std::io::stderr()) {
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

/// The columns, in order. The last one wraps; the rest are padded.
const HEADERS: [&str; 5] = ["KEY", "AUTHOR", "YEAR", "ARXIV", "TITLE"];
/// Spaces between one column and the next.
const GAP: usize = 2;
/// The widest an author cell may be before it is elided.
const AUTHOR_WIDTH: usize = 24;
/// The narrowest the title column may become, whatever the terminal says.
const TITLE_FLOOR: usize = 24;

/// Render a listing as an aligned table `width` columns wide.
///
/// Description is display data and nothing else (I7), which is what makes it
/// safe to elide an author or wrap a title here: no rendered bibliography is
/// affected by anything decided in this function.
///
/// `provider` is deliberately not a column. It is the same value on nearly
/// every row of a real bibliography, so it spent a fixed width to say almost
/// nothing; `--fields provider` still asks for it directly.
///
/// Width and colour are arguments rather than ambient reads so that the layout
/// is a pure function and can be pinned by tests at a fixed width.
pub fn table(records: &[Record], width: usize, color: bool) -> String {
    if records.is_empty() {
        // The caller's "no records match" note is the whole of the output.
        return String::new();
    }
    let rows = records.iter().map(Row::from_record).collect::<Vec<_>>();
    let widths = [
        column_width(HEADERS[0], rows.iter().map(|row| &row.key)),
        column_width(HEADERS[1], rows.iter().map(|row| &row.author)),
        column_width(HEADERS[2], rows.iter().map(|row| &row.year)),
        column_width(HEADERS[3], rows.iter().map(|row| &row.arxiv)),
    ];
    let offset = widths.iter().map(|column| column + GAP).sum::<usize>();
    // Whatever is left, but never more than half the terminal: a title is not
    // more readable for being stretched across two hundred columns.
    let title_width = width.saturating_sub(offset).min(width / 2).max(TITLE_FLOOR);

    let mut rendered = String::new();
    let style = if color {
        Style::new().bold().fg_color(Some(AnsiColor::Cyan.into()))
    } else {
        Style::new()
    };
    let heading = leading(&HEADERS[..4], &widths) + HEADERS[4];
    push_line(&mut rendered, &format!("{style}{heading}{style:#}"));

    for row in &rows {
        let cells = leading(&[&row.key, &row.author, &row.year, &row.arxiv], &widths);
        let mut wrapped = wrap(&row.title, title_width);
        // A record always occupies at least one line, even titleless.
        if wrapped.is_empty() {
            wrapped.push(String::new());
        }
        for (at, segment) in wrapped.iter().enumerate() {
            // Continuations line up under the title rather than at column zero,
            // so the block of text reads as one cell.
            let prefix = if at == 0 {
                cells.clone()
            } else {
                " ".repeat(offset)
            };
            push_line(&mut rendered, &(prefix + segment));
        }
    }
    rendered
}

/// One record reduced to the strings a listing shows.
struct Row {
    key: String,
    author: String,
    year: String,
    arxiv: String,
    title: String,
}

impl Row {
    fn from_record(record: &Record) -> Self {
        Self {
            key: record.key.to_string(),
            author: authorship(record),
            year: record
                .description
                .year
                .map(|year| year.to_string())
                .unwrap_or_default(),
            arxiv: record
                .identifiers
                .arxiv
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            title: record.description.title.clone(),
        }
    }
}

/// How a record's authorship reads in one column.
///
/// A collaboration wins when there is one: an ATLAS paper lists three thousand
/// authors, and naming the first of them is a worse answer for a reader than
/// naming the collaboration. `RecordFilter` already searches authors and
/// collaborations as one space, so a column spanning both is not a new idea
/// here.
///
/// The family name is the text before the first comma, which is a heuristic —
/// safe because this is display data that reaches no generated artifact, and
/// not something that would be acceptable anywhere near a payload.
fn authorship(record: &Record) -> String {
    if let Some(collaboration) = record.description.collaborations.first() {
        return elide(collaboration, AUTHOR_WIDTH);
    }
    let Some(first) = record.description.authors.first() else {
        return String::new();
    };
    let family = first.split(',').next().unwrap_or(first).trim();
    if record.description.authors.len() > 1 {
        // The name gives way, never the suffix: `et al.` is the part that says
        // there are more authors, so eliding it loses what the cell is for.
        const ET_AL: &str = " et al.";
        format!("{}{ET_AL}", elide(family, AUTHOR_WIDTH - ET_AL.width()))
    } else {
        elide(family, AUTHOR_WIDTH)
    }
}

/// A column is as wide as its widest cell, or its own header.
fn column_width<'a>(header: &str, cells: impl Iterator<Item = &'a String>) -> usize {
    cells
        .map(|cell| cell.width())
        .chain([header.width()])
        .max()
        .unwrap_or(0)
}

/// The padded columns that precede the title.
fn leading(cells: &[&str], widths: &[usize; 4]) -> String {
    cells
        .iter()
        .zip(widths)
        .map(|(cell, width)| pad(cell, *width))
        .collect()
}

/// `value` followed by enough spaces to fill its column and the gap after it.
///
/// The padding is computed from the display width rather than left to `{:<n$}`,
/// which counts `char`s: a combining accent is a `char` that occupies no column,
/// so formatter padding would shift every column to the right of it.
fn pad(value: &str, width: usize) -> String {
    let padding = width.saturating_sub(value.width()) + GAP;
    format!("{value}{:padding$}", "")
}

/// Break `text` into lines no wider than `width`, preferring spaces.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.width() + 1 + word.width() > width {
            lines.push(std::mem::take(&mut line));
        }
        // A word wider than the whole column has no space to break against, so
        // it is cut at the boundary rather than allowed to run off the row.
        let mut rest = word;
        while line.is_empty() && rest.width() > width {
            let (head, tail) = split_at_width(rest, width);
            lines.push(head.to_owned());
            rest = tail;
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(rest);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Split `value` at the last boundary that keeps its width within `width`.
fn split_at_width(value: &str, width: usize) -> (&str, &str) {
    let mut taken = 0;
    for (at, character) in value.char_indices() {
        let next = taken + character.width().unwrap_or(0);
        // Always take at least one character, so that a column narrower than a
        // single wide glyph still makes progress instead of looping.
        if next > width && at > 0 {
            return value.split_at(at);
        }
        taken = next;
    }
    (value, "")
}

/// `value`, or as much of it as fits in `width` followed by an ellipsis.
fn elide(value: &str, width: usize) -> String {
    if value.width() <= width {
        return value.to_owned();
    }
    let (head, _) = split_at_width(value, width.saturating_sub(1));
    format!("{head}…")
}

/// Append one line, dropping the padding that would otherwise trail it.
fn push_line(rendered: &mut String, line: &str) {
    rendered.push_str(line.trim_end());
    rendered.push('\n');
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

#[cfg(test)]
mod tests {
    use super::*;
    use bibi_application::domain::CitationKey;
    use bibi_core::{ArxivId, BibiId};

    /// A record carrying only what a listing shows.
    fn record(key: &str, title: &str) -> Record {
        let owned = bibi_provider::testing::provider_record("inspire", "1", key, title);
        Record::new(BibiId::new(), CitationKey::new(key).unwrap(), owned).expect("test record")
    }

    /// Where a rendered line's title column begins.
    fn title_offset(line: &str) -> usize {
        let trimmed = line.trim_start();
        line[..line.len() - trimmed.len()].width()
    }

    /// The display column `needle` starts at.
    ///
    /// Not `str::find`, which answers in *bytes*: a combining mark is two bytes
    /// and zero columns, so a byte offset would report two aligned rows as
    /// differing — the very confusion these tests exist to catch.
    fn column_of(line: &str, needle: &str) -> usize {
        let at = line
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} in {line:?}"));
        line[..at].width()
    }

    #[test]
    fn a_header_row_labels_every_column() {
        let rendered = table(&[record("K", "T")], 100, false);
        let heading = rendered.lines().next().unwrap();
        let positions = HEADERS
            .iter()
            .map(|header| heading.find(header).expect(header))
            .collect::<Vec<_>>();
        // Present, and in the documented order.
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "{heading}"
        );
    }

    #[test]
    fn a_title_wraps_at_its_column_and_continues_beneath_itself() {
        let mut wide = record("K", "Gravitational waves from super-Hubble bubbles");
        wide.description.year = Some(2026);
        let rendered = table(&[wide], 60, false);
        let lines = rendered.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 3, "header plus two wrapped lines: {rendered}");
        // The continuation starts under the title, not at column zero.
        assert_eq!(title_offset(lines[2]), column_of(lines[1], "Gravitational"));
        assert!(lines[2].trim().starts_with("super-Hubble"), "{rendered}");
    }

    #[test]
    fn a_word_wider_than_the_column_is_split_rather_than_overflowing() {
        // Never spaces, so there is nothing to break against.
        let long = "a".repeat(70);
        let rendered = table(&[record("K", &long)], 60, false);
        for line in rendered.lines() {
            assert!(line.width() <= 60, "{line:?} overruns the terminal");
        }
        assert!(rendered.contains(&"a".repeat(24)));
    }

    #[test]
    fn one_author_is_named_and_several_become_et_al() {
        let mut sole = record("K", "T");
        sole.description.authors = vec!["Ghoderao, Pulkit S.".into()];
        assert!(table(&[sole], 100, false).contains("Ghoderao "));

        let mut several = record("K", "T");
        several.description.authors = vec!["Ghoderao, Pulkit S.".into(), "Rajantie, Arttu".into()];
        assert!(table(&[several], 100, false).contains("Ghoderao et al."));
    }

    #[test]
    fn a_collaboration_stands_in_for_a_long_author_list() {
        let mut record = record("Aad:2012tfa", "Observation of a new particle");
        record.description.authors = (0..3000).map(|at| format!("Author{at}, A.")).collect();
        record.description.collaborations = vec!["ATLAS".into()];
        let rendered = table(&[record], 100, false);
        // Naming the first of three thousand authors would be the worse answer.
        assert!(rendered.contains("ATLAS"), "{rendered}");
        assert!(!rendered.contains("Author0"), "{rendered}");
    }

    #[test]
    fn an_elided_author_keeps_its_et_al() {
        let mut record = record("K", "T");
        record.description.authors = vec![
            "A Very Long Family Name Indeed, Q.".into(),
            "Roe, R.".into(),
        ];
        let rendered = table(&[record], 100, false);
        assert!(rendered.contains("… et al."), "{rendered}");
    }

    #[test]
    fn an_absent_year_or_arxiv_leaves_a_blank_column() {
        let mut dated = record("Dated", "First");
        dated.description.year = Some(2012);
        dated.identifiers.arxiv = Some(ArxivId::new("1207.7214").unwrap());
        let bare = record("Bare", "Second");

        let rendered = table(&[dated, bare], 100, false);
        let lines = rendered.lines().collect::<Vec<_>>();
        // The row is still a row: same title column, blanks in between.
        assert_eq!(title_offset(lines[1]), 0);
        assert!(lines[2].starts_with("Bare "), "{rendered}");
        assert_eq!(
            column_of(lines[1], "First"),
            column_of(lines[2], "Second"),
            "{rendered}"
        );
    }

    #[test]
    fn columns_stay_aligned_when_a_name_carries_combining_marks() {
        let mut decomposed = record("A", "First");
        // "Carrón" written with a combining acute: one more `char` than column.
        decomposed.description.authors = vec!["Carro\u{0301}n Duque, J.".into()];
        let mut plain = record("B", "Second");
        // Deliberately the *wider* of the two, so the decomposed cell is padded
        // rather than setting the column width. A miscount is invisible when the
        // miscounted value is itself the widest: `saturating_sub` floors both
        // answers at zero and the rows agree for the wrong reason.
        plain.description.authors = vec!["Przybycien-Baker, R.".into()];

        let rendered = table(&[decomposed, plain], 100, false);
        let lines = rendered.lines().collect::<Vec<_>>();
        // Padding by `char` count instead of display width shifts the first row.
        assert_eq!(
            column_of(lines[1], "First"),
            column_of(lines[2], "Second"),
            "titles must start in the same column:\n{rendered}"
        );
    }

    #[test]
    fn styling_is_absent_unless_it_is_asked_for() {
        let plain = table(&[record("K", "T")], 100, false);
        assert!(!plain.contains('\u{1b}'), "{plain:?}");
        let styled = table(&[record("K", "T")], 100, true);
        assert!(styled.contains('\u{1b}'));
        // Only the header is styled; the data is left alone.
        assert_eq!(
            styled
                .lines()
                .filter(|line| line.contains('\u{1b}'))
                .count(),
            1
        );
    }

    #[test]
    fn an_empty_listing_renders_no_bytes() {
        // Not even headers: the caller's "no records match" note is the answer.
        assert_eq!(table(&[], 100, false), "");
    }

    #[test]
    fn no_line_carries_trailing_whitespace() {
        let mut titled = record("K", "A title long enough to wrap across the column");
        titled.description.year = Some(2012);
        let untitled = record("Untitled", "x");
        let rendered = table(&[titled, untitled], 60, true);
        for line in rendered.lines() {
            assert_eq!(line.trim_end(), line, "{line:?} has trailing whitespace");
        }
    }

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
