use cita_core::PaperRecord;
use std::collections::BTreeMap;

pub fn export_bibtex(papers: &BTreeMap<String, PaperRecord>) -> String {
    let mut output = String::new();
    for (index, (key, record)) in papers.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        let is_article = record
            .publication
            .as_ref()
            .is_some_and(|publication| publication.is_journal());
        let kind = if is_article { "article" } else { "misc" };
        output.push_str(&format!("@{kind}{{{key},\n"));
        field(&mut output, "title", &record.title);
        if !record.authors.is_empty() {
            field(&mut output, "author", &record.authors.join(" and "));
        }
        if !record.collaborations.is_empty() {
            field(
                &mut output,
                "collaboration",
                &record.collaborations.join(" and "),
            );
        }
        // An @article cites the journal version, so its year is the journal
        // year when known; @misc keeps the citation-display year.
        let year = if is_article {
            record
                .publication
                .as_ref()
                .and_then(|publication| publication.year)
                .or(record.year)
        } else {
            record.year
        };
        if let Some(year) = year {
            field(&mut output, "year", &year.to_string());
        }
        if let Some(publication) = &record.publication {
            optional_field(&mut output, "journal", publication.journal.as_deref());
            optional_field(&mut output, "volume", publication.volume.as_deref());
            optional_field(&mut output, "number", publication.issue.as_deref());
            optional_field(&mut output, "pages", publication.pages.as_deref());
        }
        optional_field(&mut output, "doi", record.dois.first().map(String::as_str));
        if let Some(arxiv) = record.arxiv_ids.first() {
            field(&mut output, "eprint", arxiv);
            field(&mut output, "archivePrefix", "arXiv");
        }
        optional_field(
            &mut output,
            "primaryClass",
            record.primary_category.as_deref(),
        );
        optional_field(&mut output, "url", record.url.as_deref());
        output.push_str("}\n");
    }
    output
}

fn optional_field(output: &mut String, name: &str, value: Option<&str>) {
    if let Some(value) = value {
        field(output, name, value);
    }
}

fn field(output: &mut String, name: &str, value: &str) {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    output.push_str(&format!("  {name} = {{{value}}},\n"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use cita_core::Publication;

    #[test]
    fn produces_stable_sorted_tex_preserving_output() {
        let record = PaperRecord {
            title: "A  $Z\\to e^+e^-$ result".into(),
            authors: vec!["Doe, Jane".into()],
            collaborations: vec!["ATLAS".into()],
            year: Some(2020),
            arxiv_ids: vec!["2001.00001".into()],
            publication: Some(Publication {
                journal: Some("JHEP".into()),
                pages: Some("42".into()),
                ..Publication::default()
            }),
            source: "inspire".into(),
            ..PaperRecord::default()
        };
        let bib = export_bibtex(&BTreeMap::from([("A".to_string(), record)]));
        assert!(bib.starts_with("@article{A,\n  title = {A $Z\\to e^+e^-$ result},\n"));
        assert!(bib.contains("  archivePrefix = {arXiv},\n"));
        assert!(bib.ends_with("}\n"));
    }

    #[test]
    fn article_year_prefers_publication_year() {
        let record = PaperRecord {
            title: "Journal version".into(),
            year: Some(2019),
            publication: Some(Publication {
                journal: Some("JHEP".into()),
                year: Some(2020),
                ..Publication::default()
            }),
            source: "inspire".into(),
            ..PaperRecord::default()
        };
        let bib = export_bibtex(&BTreeMap::from([("A".to_string(), record.clone())]));
        assert!(bib.contains("  year = {2020},\n"), "{bib}");

        let preprint = PaperRecord {
            publication: None,
            ..record
        };
        let bib = export_bibtex(&BTreeMap::from([("A".to_string(), preprint)]));
        assert!(bib.contains("  year = {2019},\n"), "{bib}");
    }
}
