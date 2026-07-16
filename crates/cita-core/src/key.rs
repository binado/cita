use crate::PaperRecord;
use unicode_normalization::UnicodeNormalization;

pub fn validate_key(key: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("citation key must not be empty".into());
    }
    // Keys appear both in BibTeX entries and as quoted TOML table keys, so
    // reject anything that breaks either syntax.
    if key
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, ',' | '{' | '}' | '"' | '\\'))
    {
        return Err(format!(
            "invalid citation key `{key}`: whitespace, commas, braces, quotes, and backslashes are not allowed"
        ));
    }
    Ok(())
}

pub fn fallback_key(record: &PaperRecord) -> String {
    let owner = record
        .authors
        .first()
        .map(|author| author.split(',').next().unwrap_or(author))
        .or_else(|| record.collaborations.first().map(String::as_str))
        .unwrap_or("paper");
    let year = record
        .year
        .map_or_else(|| "nd".into(), |year| year.to_string());
    let title = record
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
        let record = PaperRecord {
            title: "Über gauge fields".into(),
            authors: vec!["García, Ana".into()],
            year: Some(2024),
            ..PaperRecord::default()
        };
        assert_eq!(fallback_key(&record), "Garcia2024Uber");
    }

    #[test]
    fn rejects_quotes_and_backslashes() {
        assert!(validate_key(r#"Aad"2012"#).is_err());
        assert!(validate_key(r"Aad\2012").is_err());
        assert!(validate_key("Aad:2012tfa").is_ok());
    }
}
