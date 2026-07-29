//! Splitting a set of query terms into requests INSPIRE will accept.

/// Records per search. A property of the query endpoint, not of any one
/// operation, so resolve and refresh use the same bound.
pub const MAX_BATCH_RECORDS: usize = 100;

/// Encoded length of the `q` value, in bytes.
pub const MAX_ENCODED_QUERY: usize = 6 * 1024;

/// Split terms into batches that satisfy both bounds.
///
/// Batching is what makes the design's arithmetic work: a three-hundred-record
/// project refreshes in three requests instead of three hundred, which is the
/// difference between seconds and minutes of pacing on exactly the operations
/// where a user is waiting.
pub fn batch(terms: &[String]) -> Vec<Vec<String>> {
    let mut batches: Vec<Vec<String>> = Vec::new();
    for term in terms {
        let needs_new = batches.last().is_none_or(|batch| {
            batch.len() == MAX_BATCH_RECORDS || encoded_len(batch, Some(term)) > MAX_ENCODED_QUERY
        });
        if needs_new {
            batches.push(Vec::new());
        }
        batches
            .last_mut()
            .expect("a batch exists")
            .push(term.clone());
    }
    batches
}

/// Join terms into one INSPIRE query.
pub fn query(terms: &[String]) -> String {
    terms.join(" or ")
}

/// The encoded size of `q=<query>` if `extra` were appended.
fn encoded_len(batch: &[String], extra: Option<&String>) -> usize {
    let mut terms = batch.to_vec();
    if let Some(term) = extra {
        terms.push(term.clone());
    }
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("q", &query(&terms));
    serializer.finish().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(count: usize) -> Vec<String> {
        (1..=count)
            .map(|id| format!("control_number:{id}"))
            .collect()
    }

    #[test]
    fn splits_at_the_record_limit() {
        let all = terms(205);
        let batches = batch(&all);
        assert_eq!(
            batches.iter().map(Vec::len).collect::<Vec<_>>(),
            [100, 100, 5]
        );
        // Nothing is lost or reordered.
        assert_eq!(batches.concat(), all);
    }

    #[test]
    fn splits_before_the_encoded_query_limit() {
        // Long terms hit the size bound well before the count bound.
        let long = (0..60)
            .map(|index| format!("doi:10.1000/{}{index}", "x".repeat(200)))
            .collect::<Vec<_>>();
        let batches = batch(&long);
        assert!(batches.len() > 1, "the size bound did not bind");
        for batch in &batches {
            assert!(encoded_len(batch, None) <= MAX_ENCODED_QUERY);
            assert!(!batch.is_empty());
        }
        assert_eq!(batches.concat(), long);
    }

    #[test]
    fn an_empty_request_makes_no_batches() {
        assert!(batch(&[]).is_empty());
    }

    #[test]
    fn a_query_joins_terms_with_or() {
        assert_eq!(
            query(&["arxiv:1207.7214".into(), "doi:10.1/x".into()]),
            "arxiv:1207.7214 or doi:10.1/x"
        );
    }
}
