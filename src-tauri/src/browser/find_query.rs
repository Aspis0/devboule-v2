//! What a question about a page means before any node is read: the kind of
//! control its words name ("search box", "button") and the words left over,
//! which describe the control itself.
//!
//! An agent asks for "the search box" long before it knows the page's own name
//! for it, and the page's name may be nothing at all: the role is the part of
//! the question every page answers the same way.

use super::view::FIELD_ROLES;

/// Words that name a kind of control, and the roles that kind covers.
const ROLE_WORDS: [(&str, &[&str]); 13] = [
    ("button", &["button"]),
    ("btn", &["button"]),
    ("link", &["link"]),
    ("checkbox", &["checkbox", "switch"]),
    ("radio", &["radio"]),
    ("tab", &["tab"]),
    ("dropdown", &["combobox", "listbox"]),
    ("select", &["combobox", "listbox"]),
    ("textbox", &FIELD_ROLES),
    ("searchbox", &FIELD_ROLES),
    ("input", &FIELD_ROLES),
    ("field", &FIELD_ROLES),
    ("box", &FIELD_ROLES),
];

/// Words that name a landmark, and the role the page gives it.
const PLACES: [(&str, &str); 6] = [
    ("header", "banner"),
    ("banner", "banner"),
    ("footer", "contentinfo"),
    ("sidebar", "complementary"),
    ("nav", "navigation"),
    ("navigation", "navigation"),
];

/// Words that carry nothing a control is called by.
const ARTICLES: [&str; 3] = ["the", "a", "an"];

/// Words that join a control to a place in a sentence ("link in the footer").
const FILLER: [&str; 8] = ["in", "on", "at", "of", "for", "to", "with", "from"];

/// Other words that say where to look. A query that has one has already said
/// which part of the page it means, so none is ranked ahead of another.
const AREA_WORDS: [&str; 7] = ["main", "content", "menu", "row", "list", "dialog", "modal"];

/// One question, taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    /// The question as written, lowercased and trimmed.
    pub phrase: String,
    /// The roles its role words name, empty when it names none.
    pub roles: Vec<&'static str>,
    /// The landmarks it says to look in ("footer" is `contentinfo`).
    pub places: Vec<&'static str>,
    /// What is left once the words for a kind of control and a place are out:
    /// what the control itself is called.
    pub needle: String,
    pub words: Vec<String>,
}

impl Query {
    pub fn parse(text: &str) -> Query {
        let phrase = text.trim().to_lowercase();
        // Ways to say one thing, written as two words and read as one.
        let normalized = phrase
            .replace("search bar", "search box")
            .replace("text box", "textbox")
            .replace("check box", "checkbox")
            .replace("top bar", "nav")
            .replace("top menu", "nav")
            .replace("menu bar", "nav")
            .replace("nav bar", "nav")
            .replace("navbar", "nav");
        let written: Vec<&str> = normalized.split_whitespace().collect();
        let meant: Vec<&str> = written
            .iter()
            .copied()
            .filter(|token| !ARTICLES.contains(token))
            .collect();
        let tokens = if meant.is_empty() { written } else { meant };
        let mut roles: Vec<&'static str> = Vec::new();
        let mut places: Vec<&'static str> = Vec::new();
        let mut place_words: Vec<String> = Vec::new();
        let mut words: Vec<String> = Vec::new();
        for token in tokens {
            if let Some(named) = roles_of(token) {
                for role in named {
                    if !roles.contains(role) {
                        roles.push(role);
                    }
                }
            } else if let Some((_, landmark)) = PLACES.iter().find(|(word, _)| *word == token) {
                if !places.contains(landmark) {
                    places.push(landmark);
                }
                place_words.push(token.to_owned());
                // "in" before a place joins the sentence; it is not part of a name.
                while words
                    .last()
                    .is_some_and(|word| FILLER.contains(&word.as_str()))
                {
                    words.pop();
                }
            } else {
                words.push(token.to_owned());
            }
        }
        // A question made only of words that name a place is about a control
        // called that, not about the place.
        if words.is_empty() && roles.is_empty() {
            places.clear();
            words = place_words;
        }
        Query {
            phrase,
            roles,
            places,
            needle: words.join(" "),
            words,
        }
    }

    /// Whether the question is about something a person types into, which is
    /// when the markup of a field is worth reading.
    pub fn asks_for_fields(&self) -> bool {
        self.roles.iter().any(|role| FIELD_ROLES.contains(role))
    }

    /// Whether the question says which part of the page it means.
    pub fn names_a_place(&self) -> bool {
        !self.places.is_empty()
            || self
                .phrase
                .split_whitespace()
                .any(|word| AREA_WORDS.contains(&word))
    }
}

/// The roles one word names, in the singular or the plural.
fn roles_of(token: &str) -> Option<&'static [&'static str]> {
    let singular = token.strip_suffix('s').unwrap_or(token);
    ROLE_WORDS
        .iter()
        .find(|(word, _)| *word == token || *word == singular)
        .map(|(_, roles)| *roles)
}

#[cfg(test)]
#[path = "find_query_tests.rs"]
mod tests;
