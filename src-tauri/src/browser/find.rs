//! `find`: which node on this page the words in the question mean.
//!
//! Scoring, not search: no index is built and nothing is kept between calls,
//! so the same view and the same query always give the same twenty answers in
//! the same order. A model in the loop here would cost a step and a guess;
//! what an agent sends is usually close to an accessible name ("the search
//! box", "Sign in"), so the name carries the most and the text above it
//! breaks the ties.

use super::find_query::Query;
use super::view::{View, ViewNode};

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

/// The matches for one question, best first. Nodes the query says nothing
/// about are not matches.
///
/// A question that names a kind of control, or a part of the page, is answered
/// from that kind in that part first: "search box" is the fields, and a button
/// called "Search" is not one of them. Only when nothing there fits is the
/// reading widened, so "sign in button" still finds the link a page styled as
/// one, and "footer help" still finds a Help the page put somewhere else.
pub fn rank(view: &View, query: &Query) -> Vec<Match> {
    if query.phrase.is_empty() {
        return Vec::new();
    }
    let mut scored = Vec::new();
    for (of_the_kind, in_the_place) in [(true, true), (true, false), (false, true), (false, false)]
    {
        // A pass that narrows by something the question never said is one a
        // wider pass already is.
        if (of_the_kind && query.roles.is_empty()) || (in_the_place && query.places.is_empty()) {
            continue;
        }
        scored = scores(view, query, of_the_kind, in_the_place);
        if !scored.is_empty() {
            break;
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

/// Every node the query says something about, with where it sits in the view.
fn scores(view: &View, query: &Query, of_the_kind: bool, in_the_place: bool) -> Vec<(usize, u32)> {
    view.nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| !of_the_kind || query.roles.contains(&node.role.as_str()))
        .filter(|(_, node)| !in_the_place || query.places.contains(&node.landmark.as_str()))
        .filter_map(|(position, node)| {
            let score = score(node, query, of_the_kind);
            (score > 0).then_some((position, score))
        })
        .collect()
}

/// How well one node answers the query. Every field is read, so a control with
/// no name can still be found by its value, its markup, its label or the words
/// above it.
///
/// The name is a tier and everything else is the tie-break inside it, which is
/// what "the name carries the most, the text above it breaks the ties" has to
/// mean: the surroundings are worth at most 80, so they can order two nodes
/// whose own names match the same way and can never lift one past a better
/// name. What the node says about itself ranks between a name and its
/// surroundings, because it is the node's own word for what it is.
fn score(node: &ViewNode, query: &Query, of_the_kind: bool) -> u32 {
    let name = node.name.to_lowercase();
    let needle = query.needle.as_str();
    let tokens = &query.words;
    let named = if needle.is_empty() {
        0
    } else if name == needle {
        100
    } else if name.contains(needle) {
        60
    } else if tokens.iter().all(|token| name.contains(token.as_str())) {
        40
    } else {
        0
    };
    let tier = if named > 0 {
        named
    } else if !of_the_kind {
        0
    } else if tokens.is_empty() {
        // The kind is the whole question: every control of it answers.
        20
    } else if all_in(tokens, &own_words(node, &name)) {
        35
    } else if all_in(
        tokens,
        &format!("{} {}", node.context, node.nearby).to_lowercase(),
    ) {
        25
    } else {
        0
    };
    let mut around = 0;
    if contains(&node.value, needle) {
        around += 20;
    }
    if contains(&node.description, needle)
        || contains(&node.placeholder, needle)
        || contains(&node.hints, needle)
    {
        around += 15;
    }
    if contains(&node.nearby, needle) || contains(&node.context, needle) {
        around += 10;
    }
    let signal = tier * 100 + around;
    if signal == 0 {
        return 0;
    }
    // What neither says anything about the words: being the kind asked for,
    // and being in the part of the page a person looks at first.
    if query.roles.contains(&node.role.as_str()) {
        around += 30;
    }
    if !query.names_a_place() && matches!(node.landmark.as_str(), "navigation" | "banner") {
        around += 5;
    }
    tier * 100 + around
}

/// What a node says about itself, lowercased: its name, its role, what the
/// markup of a field adds and what the page described it with.
fn own_words(node: &ViewNode, name: &str) -> String {
    format!(
        "{name} {} {} {} {}",
        node.role, node.placeholder, node.hints, node.description
    )
    .to_lowercase()
}

fn all_in(tokens: &[String], haystack: &str) -> bool {
    tokens.iter().all(|token| haystack.contains(token.as_str()))
}

fn contains(field: &str, needle: &str) -> bool {
    !needle.is_empty() && !field.is_empty() && field.to_lowercase().contains(needle)
}

#[cfg(test)]
#[path = "find_tests.rs"]
mod tests;
