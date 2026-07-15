use paperdb_core::Paper;

pub fn export_bibtex(papers: &[Paper]) -> String {
    let mut papers = papers.iter().collect::<Vec<_>>();
    papers.sort_by(|a, b| a.key.cmp(&b.key));
    let mut output = String::new();
    for (index, paper) in papers.into_iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        let kind = if paper.publication.as_ref().is_some_and(|p| p.is_journal()) {
            "article"
        } else {
            "misc"
        };
        output.push_str(&format!("@{kind}{{{},\n", paper.key));
        field(&mut output, "title", &paper.title);
        if !paper.authors.is_empty() {
            field(&mut output, "author", &paper.authors.join(" and "));
        }
        if !paper.collaborations.is_empty() {
            field(
                &mut output,
                "collaboration",
                &paper.collaborations.join(" and "),
            );
        }
        if let Some(year) = paper.year {
            field(&mut output, "year", &year.to_string());
        }
        if let Some(publication) = &paper.publication {
            optional_field(&mut output, "journal", publication.journal.as_deref());
            optional_field(&mut output, "volume", publication.volume.as_deref());
            optional_field(&mut output, "number", publication.issue.as_deref());
            optional_field(&mut output, "pages", publication.pages.as_deref());
        }
        optional_field(&mut output, "doi", paper.dois.first().map(String::as_str));
        if let Some(arxiv) = paper.arxiv_ids.first() {
            field(&mut output, "eprint", arxiv);
            field(&mut output, "archivePrefix", "arXiv");
        }
        optional_field(
            &mut output,
            "primaryClass",
            paper.primary_category.as_deref(),
        );
        optional_field(&mut output, "url", paper.url.as_deref());
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
    use paperdb_core::Publication;

    #[test]
    fn produces_stable_sorted_tex_preserving_output() {
        let paper = Paper {
            key: "A".into(),
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
            ..Paper::default()
        };
        let bib = export_bibtex(&[paper]);
        assert!(bib.starts_with("@article{A,\n  title = {A $Z\\to e^+e^-$ result},\n"));
        assert!(bib.contains("  archivePrefix = {arXiv},\n"));
        assert!(bib.ends_with("}\n"));
    }
}
