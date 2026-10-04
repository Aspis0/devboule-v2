//! `find`: which node on this page the words in the question mean.
//!
//! Scoring, not search: no index is built and nothing is kept between calls,
//! so the same view and the same query always give the same twenty answers in
//! the same order. A model in the loop here would cost a step and a guess;
//! what an agent sends is usually close to an accessible name ("the search
//! box", "Sign in"), so the name carries the most and the text above it
//! breaks the ties.
//!
//! A kind of control ("button") and a part of the page ("footer") only boost:
//! a node of that kind or in that place ranks ahead of the rest, and nothing is
//! dropped for lacking them. A page whose top bar is a table and not a
//! `navigation` still answers "the new link in the top bar" by the name.

use super::find_query::Query;
use super::view::{View, ViewNode};
use super::view_line::clip;

/// The most matches one query answers with. A caller that wants more narrows
/// the query: twenty is already a page of reading, and a hundred would be a
/// page of guessing.
pub const MAX_MATCHES: usize = 20;

/// How many nodes from the top of the page count as the top of the page.
const EARLY_NODES: usize = 30;

/// What a match is said to be in when no row and no landmark names it. A
/// control at the top of the page with nothing around it still has a place,
/// and "in " on its own is not one.
const TOP_OF_PAGE: &str = "at top of page";

/// How much of a nearby label or description a match repeats.
const DETAIL_MAX: usize = 60;

/// One node the query might mean, as the contract names it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub reference: String,
    pub role: String,
    pub name: String,
    pub context: String,
    /// What a node with no name says about itself, so the caller can tell it
    /// picked the right one: its markup, the label near it, how the page
    /// described it. Empty for a node that has a name.
    pub detail: String,
}

/// How one node ranks. Field order is the order of importance: the kind and the
/// place the question asked for come before how well the words fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Rank {
    kind: bool,
    place: bool,
    score: u32,
}

/// The matches for one question, best first, and in document order inside a
/// rank so that the control at the top of the page comes before the same one
/// repeated down it. Nodes the query says nothing about are not matches.
pub fn rank(view: &View, query: &Query) -> Vec<Match> {
    if query.phrase.is_empty() {
        return Vec::new();
    }
    let mut ranked: Vec<(Rank, usize)> = view
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(position, node)| Some((score(node, position, query)?, position)))
        .collect();
    ranked.sort_by(|(one, at), (other, later)| other.cmp(one).then(at.cmp(later)));
    ranked
        .into_iter()
        .take(MAX_MATCHES)
        .map(|(_, position)| {
            let node = &view.nodes[position];
            Match {
                reference: node.ref_text(),
                role: node.role.clone(),
                name: node.name.clone(),
                context: where_it_is(node, position),
                detail: if node.name.is_empty() {
                    describe(node)
                } else {
                    String::new()
                },
            }
        })
        .collect()
}

/// Where a match is, as the contract names it back. Its own surroundings when
/// a row or a landmark names them, and otherwise how near the top of the page
/// it is — which is what tells two controls that share a name apart on a page
/// whose landmarks are all unnamed and whose rows are all one table.
fn where_it_is(node: &ViewNode, position: usize) -> String {
    match (node.context.is_empty(), position < EARLY_NODES) {
        (true, true) => TOP_OF_PAGE.to_owned(),
        _ => node.context.clone(),
    }
}

/// What a node with no name says about itself.
pub fn describe(node: &ViewNode) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !node.hints.is_empty() {
        parts.push(node.hints.clone());
    } else if !node.placeholder.is_empty() {
        parts.push(format!(
            "placeholder=\"{}\"",
            clip(&node.placeholder, DETAIL_MAX)
        ));
    }
    if !node.nearby.is_empty() {
        parts.push(format!("near \"{}\"", clip(&node.nearby, DETAIL_MAX)));
    }
    if !node.description.is_empty() {
        parts.push(format!(
            "described as \"{}\"",
            clip(&node.description, DETAIL_MAX)
        ));
    }
    parts.join(", ")
}

/// How well one node answers the query, or None when it says nothing about it.
/// Every field is read, so a control with no name can still be found by its
/// value, its markup, its label or the words above it.
///
/// The name is a tier and everything else is the tie-break inside it: the
/// surroundings are worth at most 59, so they can order two nodes whose own
/// names match the same way and can never lift one past a better name. What a
/// node of the asked-for kind says about itself ranks between a name and its
/// surroundings, because it is the node's own word for what it is.
fn score(node: &ViewNode, position: usize, query: &Query) -> Option<Rank> {
    let name = node.name.to_lowercase();
    let needle = query.needle.as_str();
    let tokens = &query.words;
    let kind = query.roles.contains(&node.role.as_str());
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
    } else if !kind {
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
    // Words around a node whose own name is already the question say nothing
    // more about it, and would put every repeat of it ahead of the first.
    if named < 100 && (contains(&node.nearby, needle) || contains(&node.context, needle)) {
        around += 10;
    }
    let mut score = tier * 100 + around;
    if score == 0 {
        return None;
    }
    // Where a person looks first: the page's navigation and header, and its top.
    if !query.names_a_place() && matches!(node.landmark.as_str(), "navigation" | "banner") {
        score += 8;
    }
    if position < EARLY_NODES {
        score += 6;
    }
    Some(Rank {
        kind,
        place: query.places.contains(&node.landmark.as_str()),
        score,
    })
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
