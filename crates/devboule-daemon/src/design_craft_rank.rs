//! Lexical skill ranking: BM25 over field-weighted term frequencies.
//!
//! One responsibility: order the craft sections for a query exactly as the
//! app's ranker does — same tokenization, same weights, same weak-query
//! concession — so the matcher's head agrees slug for slug.

use std::collections::{HashMap, HashSet};

use super::design_craft::{Manifest, SkillSection};

fn tokenize(text: &str, stopwords: &HashSet<&str>, min_len: usize) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_ascii_lowercase() && !c.is_ascii_digit())
        .filter(|token| token.len() >= min_len && !stopwords.contains(*token))
        .map(str::to_string)
        .collect()
}

struct WeightedDoc {
    tf: HashMap<String, f64>,
    length: f64,
}

/// The full ranking for one query, best first, with the weak-query
/// concession: no useful tokens, or a best score under the threshold, hands
/// back the input order untouched — the input order is the priority order,
/// so refusing to rank means keeping it. Ties keep input order too.
pub(crate) fn rank_slugs_for_query(
    query: &str,
    index: &[SkillSection],
    manifest: &Manifest,
) -> (Vec<String>, bool) {
    let input_order: Vec<String> = index.iter().map(|s| s.slug.clone()).collect();
    let stopwords: HashSet<&str> = manifest.stopwords.iter().map(|s| s.as_str()).collect();
    let query_tokens = tokenize(query, &stopwords, manifest.min_token_length);
    if index.is_empty() || query_tokens.is_empty() {
        return (input_order, true);
    }
    let mut docs = Vec::with_capacity(index.len());
    let mut document_frequency: HashMap<String, usize> = HashMap::new();
    let mut total_length = 0.0;
    for section in index {
        let mut tf: HashMap<String, f64> = HashMap::new();
        let mut length = 0.0;
        let fields = [
            (&section.title, manifest.weights.title),
            (&section.description, manifest.weights.description),
            (&section.body, manifest.weights.body),
        ];
        for (text, weight) in fields {
            for token in tokenize(text, &stopwords, manifest.min_token_length) {
                *tf.entry(token).or_insert(0.0) += weight;
                length += weight;
            }
        }
        total_length += length;
        for token in tf.keys() {
            *document_frequency.entry(token.clone()).or_insert(0) += 1;
        }
        docs.push(WeightedDoc { tf, length });
    }
    let count = docs.len() as f64;
    let average = total_length / count;
    let mut distinct: Vec<String> = Vec::new();
    for token in query_tokens {
        if !distinct.contains(&token) {
            distinct.push(token);
        }
    }
    let scores: Vec<f64> = docs
        .iter()
        .map(|doc| {
            let mut score = 0.0;
            for token in &distinct {
                let tf = match doc.tf.get(token) {
                    Some(tf) => *tf,
                    None => continue,
                };
                let frequency = *document_frequency.get(token).unwrap_or(&0) as f64;
                let idf = (1.0 + (count - frequency + 0.5) / (frequency + 0.5)).ln();
                score += idf
                    * ((tf * (manifest.bm25.k1 + 1.0))
                        / (tf
                            + manifest.bm25.k1
                                * (1.0 - manifest.bm25.b
                                    + manifest.bm25.b * (doc.length / average))));
            }
            score
        })
        .collect();
    let best = scores.iter().cloned().fold(0.0, f64::max);
    if best < manifest.bm25.fallback_threshold {
        return (input_order, true);
    }
    let mut ranked: Vec<(f64, usize, &str)> = index
        .iter()
        .enumerate()
        .map(|(position, section)| (scores[position], position, section.slug.as_str()))
        .collect();
    ranked.sort_by(|left, right| {
        right
            .0
            .partial_cmp(&left.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(left.1.cmp(&right.1))
    });
    (
        ranked
            .into_iter()
            .map(|(_, _, slug)| slug.to_string())
            .collect(),
        false,
    )
}
