//! `find`: which node on this page the words in the question mean.
//!
//! Scoring, not search: no index is built and nothing is kept between calls,
//! so the same view and the same query always give the same twenty answers in
//! the same order. A model in the loop here would cost a step and a guess;
//! what an agent sends is usually close to an accessible name ("the search
//! box", "Sign in"), so the name carries the most and the text above it
//! breaks the ties.

use super::view::View;
use super::view::ViewNode;

/// The most matches one query answers with. A caller that wants more narrows
/// the query: twenty is already a page of reading, and a hundred would be a
/// page of guessing.
pub const MAX_MATCHES: usize = 20;

/// One node the query might mean, as the contract names it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub reference: String,
    pub role: String,
    pub name: String,
    pub context: String,
}

/// The matches for one query, best first. Nodes the query says nothing about
/// are not matches.
pub fn find(view: &View, query: &str) -> Vec<Match> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let tokens: Vec<&str> = needle.split_whitespace().collect();
    let mut scored: Vec<(usize, u32)> = Vec::new();
    for (position, node) in view.nodes.iter().enumerate() {
        let score = score(node, &needle, &tokens);
        if score > 0 {
            scored.push((position, score));
        }
    }
    // Best score first, and document order inside a score, so the answer is a
    // list a reader can work down rather than a set to sort again.
    scored.sort_by(|(one, first), (other, second)| second.cmp(first).then(one.cmp(other)));
    scored
        .into_iter()
        .take(MAX_MATCHES)
        .map(|(position, _)| {
            let node = &view.nodes[position];
            Match {
                reference: node.ref_text(),
                role: node.role.clone(),
                name: node.name.clone(),
                context: node.context.clone(),
            }
        })
        .collect()
}

/// How well one node answers the query. Every field is read, so a control with
/// no name can still be found by its value, its label or the words above it.
///
/// The name is a tier and everything else is the tie-break inside it, which is
/// what "the name carries the most, the text above it breaks the ties" has to
/// mean: the surroundings are worth at most 75, so they can order two nodes
/// whose own names match the same way and can never lift one past a better
/// name.
fn score(node: &ViewNode, needle: &str, tokens: &[&str]) -> u32 {
    let role = node.role.to_lowercase();
    let name = node.name.to_lowercase();
    let named = if name == needle {
        100
    } else if name.contains(needle) {
        60
    } else if !tokens.is_empty() && tokens.iter().all(|token| name.contains(token)) {
        40
    } else {
        0
    };
    let mut around = 0;
    if tokens.contains(&role.as_str()) {
        around += 30;
    }
    if contains(&node.value, needle) {
        around += 20;
    }
    if contains(&node.description, needle) || contains(&node.placeholder, needle) {
        around += 15;
    }
    if contains(&node.nearby, needle) || contains(&node.context, needle) {
        around += 10;
    }
    named * 100 + around
}

fn contains(field: &str, needle: &str) -> bool {
    !field.is_empty() && field.to_lowercase().contains(needle)
}

#[cfg(test)]
#[path = "find_tests.rs"]
mod tests;
