use super::{find_manifest, highlight_style};
use crate::{Order, SortBy};
use anyhow::Result;
use bibi_manifest::Manifest;
use std::{
    io::{self, IsTerminal},
    path::Path,
};

struct Row {
    key: String,
    title: String,
    author: String,
    year_num: Option<i32>,
    year: String,
}

pub(crate) fn list(cwd: &Path, sort_by: SortBy, order: Order, wrap_title: bool) -> Result<()> {
    let manifest = Manifest::load_verified(find_manifest(cwd)?)?;
    let mut rows = manifest
        .projected()?
        .into_iter()
        .map(|item| {
            let author = match item.reference.authors.first() {
                Some(first) if item.reference.authors.len() > 1 => format!("{first} et al."),
                Some(first) => first.clone(),
                None => item
                    .reference
                    .collaborations
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "—".into()),
            };
            Row {
                key: item.key,
                title: item.reference.title,
                author,
                year_num: item.reference.year,
                year: item
                    .reference
                    .year
                    .map_or_else(|| "—".into(), |year| year.to_string()),
            }
        })
        .collect::<Vec<_>>();
    match sort_by {
        SortBy::Key => {
            if let Order::Desc = order {
                rows.reverse();
            }
        }
        SortBy::Title => rows.sort_by(|a, b| ordered(a.title.cmp(&b.title), order)),
        SortBy::Author => rows.sort_by(|a, b| ordered(a.author.cmp(&b.author), order)),
        SortBy::Year => rows.sort_by(|a, b| match (a.year_num, b.year_num) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (Some(a), Some(b)) => ordered(a.cmp(&b), order),
        }),
    }
    print_rows(rows, wrap_title);
    Ok(())
}
fn ordered(value: std::cmp::Ordering, order: Order) -> std::cmp::Ordering {
    if matches!(order, Order::Desc) {
        value.reverse()
    } else {
        value
    }
}

fn print_rows(rows: Vec<Row>, wrap_title: bool) {
    if rows.is_empty() {
        return;
    }
    let headers = ["Key", "Title", "Author", "Year"];
    let key_width = column_width(headers[0], rows.iter().map(|row| row.key.as_str()));
    let author_width = column_width(headers[2], rows.iter().map(|row| row.author.as_str()));
    let year_width = column_width(headers[3], rows.iter().map(|row| row.year.as_str()));
    let title_width = terminal_size::terminal_size_of(io::stdout()).map(|(width, _)| {
        (width.0 as usize)
            .saturating_sub(key_width + author_width + year_width + 6)
            .max(10)
    });
    let displayed_title_width = title_width
        .unwrap_or_else(|| column_width(headers[1], rows.iter().map(|row| row.title.as_str())));
    let header = format!(
        "{:<kw$}  {:<tw$}  {:<aw$}  {:<yw$}",
        headers[0],
        headers[1],
        headers[2],
        headers[3],
        kw = key_width,
        tw = displayed_title_width,
        aw = author_width,
        yw = year_width
    );
    match highlight_style(io::stdout().is_terminal()) {
        Some(style) => println!("{style}{header}{style:#}"),
        None => println!("{header}"),
    }
    for row in rows {
        let titles = match title_width {
            Some(width) if wrap_title => wrap(&row.title, width),
            Some(width) => vec![truncate(&row.title, width)],
            None => vec![row.title],
        };
        for (index, title) in titles.into_iter().enumerate() {
            println!(
                "{:<kw$}  {:<tw$}  {:<aw$}  {:<yw$}",
                if index == 0 { row.key.as_str() } else { "" },
                title,
                if index == 0 { row.author.as_str() } else { "" },
                if index == 0 { row.year.as_str() } else { "" },
                kw = key_width,
                tw = displayed_title_width,
                aw = author_width,
                yw = year_width
            );
        }
    }
}
fn column_width<'a>(header: &str, values: impl Iterator<Item = &'a str>) -> usize {
    values.fold(header.chars().count(), |width, value| {
        width.max(value.chars().count())
    })
}
fn truncate(value: &str, width: usize) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() <= width {
        return value.into();
    }
    if width == 0 {
        return String::new();
    }
    chars[..width - 1]
        .iter()
        .chain(std::iter::once(&'…'))
        .collect()
}
fn wrap(value: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![value.into()];
    };
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in value.split_whitespace() {
        let len = word.chars().count();
        if len > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            lines.extend(
                word.chars()
                    .collect::<Vec<_>>()
                    .chunks(width)
                    .map(|c| c.iter().collect()),
            );
        } else if !current.is_empty() && current.chars().count() + 1 + len > width {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        } else {
            if !current.is_empty() {
                current.push(' ')
            }
            current.push_str(word)
        }
    }
    if !current.is_empty() {
        lines.push(current)
    }
    if lines.is_empty() {
        lines.push(String::new())
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_title_helpers() {
        assert_eq!(truncate("αβγ", 2), "α…");
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
    }
}
