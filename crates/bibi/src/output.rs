//! Pipeable stdout and human diagnostics on stderr.

use crate::cli::Field;
use anyhow::Context;
use bibi_application::domain::{Record, Source};
use std::io::{IsTerminal, Write};

pub fn emit(text: &str) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    match stdout
        .write_all(text.as_bytes())
        .and_then(|_| stdout.flush())
    {
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => std::process::exit(0),
        result => result.context("writing stdout"),
    }
}

pub fn note(message: impl std::fmt::Display) {
    let _ = writeln!(std::io::stderr(), "{message}");
}

pub fn warn(message: impl std::fmt::Display) {
    let _ = writeln!(std::io::stderr(), "warning: {message}");
}

pub fn table(records: &[Record]) -> String {
    if records.is_empty() {
        return String::new();
    }
    let mut output = String::from("KEY\tAUTHOR\tYEAR\tARXIV\tTITLE\n");
    for record in records {
        let state = record.state();
        let author = state
            .description()
            .collaborations()
            .first()
            .or_else(|| state.description().authors().first())
            .map(String::as_str)
            .unwrap_or_default();
        output.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\n",
            record.texkey(),
            author,
            state
                .description()
                .year()
                .map(|year| year.to_string())
                .unwrap_or_default(),
            state
                .identifiers()
                .arxiv()
                .map(ToString::to_string)
                .unwrap_or_default(),
            state.description().title()
        ));
    }
    output
}

pub fn fields(records: &[Record], fields: &[Field]) -> String {
    let mut output = String::new();
    for record in records {
        let values = fields
            .iter()
            .map(|field| field_value(record, *field))
            .collect::<Vec<_>>();
        output.push_str(&values.join("\t"));
        output.push('\n');
    }
    output
}

fn field_value(record: &Record, field: Field) -> String {
    let state = record.state();
    match field {
        Field::Key => record.texkey().to_owned(),
        Field::Title => state.description().title().to_owned(),
        Field::Year => state
            .description()
            .year()
            .map(|year| year.to_string())
            .unwrap_or_default(),
        Field::Source => match state.source() {
            Source::Local => "local".into(),
            Source::Managed { provider, .. } => provider.to_string(),
        },
        Field::Doi => state
            .identifiers()
            .doi()
            .map(ToString::to_string)
            .unwrap_or_default(),
        Field::Arxiv => state
            .identifiers()
            .arxiv()
            .map(ToString::to_string)
            .unwrap_or_default(),
        Field::ArxivUrl => state
            .identifiers()
            .arxiv()
            .map(|id| format!("https://arxiv.org/pdf/{id}"))
            .unwrap_or_default(),
    }
}

#[allow(dead_code)]
pub fn color_enabled(stream: &impl IsTerminal) -> bool {
    stream.is_terminal() && std::env::var_os("NO_COLOR").is_none()
}
