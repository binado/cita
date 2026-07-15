use crate::ResolvedPaper;
use unicode_normalization::UnicodeNormalization;

pub fn validate_key(key: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("citation key must not be empty".into());
    }
    if key
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, ',' | '{' | '}'))
    {
        return Err(format!(
            "invalid citation key `{key}`: whitespace, commas, and braces are not allowed"
        ));
    }
    Ok(())
}

pub fn fallback_key(paper: &ResolvedPaper) -> String {
    let owner = paper
        .authors
        .first()
        .map(|author| author.split(',').next().unwrap_or(author))
        .or_else(|| paper.collaborations.first().map(String::as_str))
        .unwrap_or("paper");
    let year = paper
        .year
        .map_or_else(|| "nd".into(), |year| year.to_string());
    let title = paper
        .title
        .split_whitespace()
        .find(|word| word.chars().any(char::is_alphanumeric))
        .unwrap_or("paper");
    format!("{}{}{}", ascii_word(owner), year, ascii_word(title))
}

fn ascii_word(value: &str) -> String {
    let value: String = value.nfkd().filter(|c| c.is_ascii_alphanumeric()).collect();
    if value.is_empty() {
        "paper".into()
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_ascii_fallbacks() {
        let paper = ResolvedPaper {
            title: "Über gauge fields".into(),
            authors: vec!["García, Ana".into()],
            year: Some(2024),
            ..ResolvedPaper::default()
        };
        assert_eq!(fallback_key(&paper), "Garcia2024Uber");
    }
}
