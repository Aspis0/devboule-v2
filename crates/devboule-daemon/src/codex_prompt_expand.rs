//! The argument substitution Paseo applies to a custom prompt's body before
//! Codex sees it, and the tokenizer that splits the arguments it substitutes.
//!
//! Translated from
//! `paseo-src/packages/server/src/server/agent/providers/codex-app-server-agent.ts`:
//! `expandCodexCustomPrompt` :763-808, `escapeRegExp` :763-765,
//! `tokenizeCommandArgs` :576-616 and `decodeEscapedChar` :552-556.
//!
//! This is the half of the command surface that has nothing to do with the
//! app-server: a string in, a string out. It runs on the body a
//! `prompts:<name>` command carries — Codex's own prompt expansion, which the
//! app-server does not perform for a text input.

use std::collections::BTreeMap;

/// The name of the placeholder that hides a literal `$$` from the substitution
/// passes, exactly as Paseo names it (:779).
const DOLLAR_PLACEHOLDER: &str = "__CODEX_DOLLAR_PLACEHOLDER__";

/// `expandCodexCustomPrompt` :763-808: `$$` is a literal dollar, `$ARGUMENTS`
/// is the whole argument string, `$1`-`$9` are the positional tokens, and a
/// `<name>=<value>` token fills `$<name>`.
pub(crate) fn expand_prompt(template: &str, args: &str) -> String {
    let trimmed = args.trim();
    let tokens = if trimmed.is_empty() {
        Vec::new()
    } else {
        tokenize(trimmed)
    };
    let mut named: BTreeMap<String, String> = BTreeMap::new();
    let mut positional: Vec<String> = Vec::new();
    for token in &tokens {
        // Paseo's `idx > 0` (:784): a token that starts with `=` is positional,
        // and the first `=` splits the name from the value.
        match token.find('=') {
            Some(index) if index > 0 => {
                named.insert(token[..index].to_string(), token[index + 1..].to_string());
            }
            _ => positional.push(token.clone()),
        }
    }

    // `$$` is hidden first and restored last, so a dollar written literally
    // cannot be read as a placeholder by the passes below.
    let mut out = template.replace("$$", DOLLAR_PLACEHOLDER);
    out = out.replace("$ARGUMENTS", trimmed);
    for position in 1..=9 {
        let value = positional.get(position - 1).cloned().unwrap_or_default();
        out = out.replace(&format!("${position}"), &value);
    }
    // Longest name first (Paseo's `sort((a, b) => b.length - a.length)` :796),
    // so `$branch_name` survives a `$branch` token.
    let mut names: Vec<&String> = named.keys().collect();
    names.sort_by_key(|name| std::cmp::Reverse(name.len()));
    for name in names {
        out = replace_dollar_word(&out, name, &named[name]);
    }
    out.replace(DOLLAR_PLACEHOLDER, "$")
}

/// Paseo's named-token regex (:800-802): a dollar literal, the escaped name,
/// then a word boundary. Every `$<name>` whose following character is on the
/// other side of that boundary from the name's last character is replaced.
/// JavaScript's boundary test is ASCII-only without the `u` flag, so "word
/// character" here is `[A-Za-z0-9_]` — which is what the name itself is built
/// from, and why both ends are compared rather than only the character after
/// the needle.
fn replace_dollar_word(text: &str, name: &str, value: &str) -> String {
    let needle = format!("${name}");
    // A name whose last character is not a word character has no boundary to
    // its right unless the next character IS one — Paseo's regex says so, and
    // a name like `dir.` reaches here from a `dir.=value` token.
    let name_ends_on_word = name.chars().next_back().is_some_and(is_word_character);
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find(&needle) {
        out.push_str(&rest[..index]);
        let after = &rest[index + needle.len()..];
        let boundary = match after.chars().next() {
            Some(next) => name_ends_on_word != is_word_character(next),
            None => name_ends_on_word,
        };
        out.push_str(if boundary { value } else { &needle });
        rest = after;
    }
    out.push_str(rest);
    out
}

fn is_word_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

/// `tokenizeCommandArgs` :576-616: whitespace splits, `'…'` and `"…"` group,
/// and inside a quote a backslash escapes the quote, another backslash, `n`
/// and `t`.
fn tokenize(args: &str) -> Vec<String> {
    let characters: Vec<char> = args.chars().collect();
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if let Some(opened) = quote {
            if character == opened {
                quote = None;
                index += 1;
                continue;
            }
            // The escape covers the open quote, a backslash, `n` and `t`, and
            // needs a character after the backslash; anything else — a
            // backslash that ends the string included — stays text.
            if character == '\\' && index + 1 < characters.len() {
                let next = characters[index + 1];
                if next == opened || next == '\\' || next == 'n' || next == 't' {
                    current.push(match next {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                    index += 2;
                    continue;
                }
            }
            current.push(character);
            index += 1;
            continue;
        }
        if character == '\'' || character == '"' {
            quote = Some(character);
            index += 1;
            continue;
        }
        if character.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            index += 1;
            continue;
        }
        current.push(character);
        index += 1;
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::expand_prompt;

    /// Every expectation here came from Paseo's own JavaScript run over the
    /// same inputs (`expandCodexCustomPrompt` :763-808, `tokenizeCommandArgs`
    /// :576-616). Where the port is surprising, the surprise is Paseo's, and it
    /// is pinned rather than smoothed over.
    #[test]
    fn the_whole_substitution_table_in_one_case() {
        assert_eq!(
            expand_prompt(
                "$$ | $ARGUMENTS |\n$1 $2 $9 |\n$branch/$name |\n$branch_name alone |\n$10\n",
                "branch=main name=\"a b\" one two"
            ),
            "$ | branch=main name=\"a b\" one two |\none two  |\nmain/a b |\n\
             $branch_name alone |\none0\n",
            "$$ is a literal dollar, $ARGUMENTS is the whole argument string, a \
             missing $9 is empty, a quoted value is one token, a longer name is not \
             eaten by a shorter one, and $10 reads as $1 plus a zero because the \
             positional passes run in order"
        );
    }

    #[test]
    fn a_prompt_body_keeps_its_own_spacing_and_gains_empty_values() {
        assert_eq!(
            expand_prompt("On $1: $ARGUMENTS\n", "stage"),
            "On stage: stage\n",
            "the body is sent whole, trailing newline included (:656)"
        );
        assert_eq!(
            expand_prompt("On $1: $ARGUMENTS\n", ""),
            "On : \n",
            "with no arguments every slot is the empty string (:776, :791)"
        );
    }

    #[test]
    fn quotes_group_and_the_four_escapes_work_inside_them() {
        // The four escapes are the open quote, a backslash, `n` and `t`. A
        // backslash before anything else stays text, and whitespace inside a
        // quote does not split the token.
        assert_eq!(
            expand_prompt("x $1 y", r#""a\nb""#),
            "x a\nb y",
            "\\n inside a quote is a newline (:589-593)"
        );
        assert_eq!(
            expand_prompt("x $1 y", r#"'it\'s'"#),
            "x it's y",
            "an escaped quote stays inside the quoted token"
        );
        assert_eq!(
            expand_prompt("x $1 y", r#""C:\\path""#),
            r"x C:\path y",
            "an escaped backslash becomes one backslash"
        );
        assert_eq!(
            expand_prompt("x $1 y", r#""C:\path""#),
            r"x C:\path y",
            "a backslash before an unexpected character is kept, both of them, so \
             the two inputs above arrive at the same text"
        );
        assert_eq!(
            expand_prompt("x $1|$2 y", r#""unclosed rest"#),
            "x unclosed rest| y",
            "a quote that never closes swallows the whitespace after it"
        );
    }

    #[test]
    fn a_leading_equals_sign_is_an_argument_not_a_name() {
        // Paseo's `idx > 0`: a token that opens with `=` has no name to speak of.
        assert_eq!(expand_prompt("$1", "=x"), "=x");
        assert_eq!(
            expand_prompt("$name!", "name="),
            "!",
            "an empty value is still a value, and the name was taken"
        );
    }

    #[test]
    fn a_name_written_with_its_dollar_is_no_shortcut() {
        // The key is whatever sits before the first `=`, dollar included, so a
        // `$name=value` token looks for `$$name` in the body and finds nothing.
        // Paseo's regex does the same; this case keeps the port from quietly
        // normalising the difference away.
        assert_eq!(expand_prompt("$name", "$name=main"), "$name");
    }
}
