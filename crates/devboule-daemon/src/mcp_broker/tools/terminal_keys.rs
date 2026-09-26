//! The keys payload: the tokens one `keys` string may name, the bytes it
//! stands for, and how the consent card says what will be typed without
//! carrying any of it.

/// Paseo's key tokens: the names one key press is spelled with and the bytes
/// each stands for (`paseo-tools.ts`, `resolveTerminalKeyToken`).
pub(super) const KEY_TOKENS: [(&str, &str); 11] = [
    ("Enter", "\r"),
    ("Tab", "\t"),
    ("Escape", "\u{1b}"),
    ("Space", " "),
    ("BSpace", "\u{7f}"),
    ("C-c", "\u{3}"),
    ("C-d", "\u{4}"),
    ("C-z", "\u{1a}"),
    ("C-l", "\u{c}"),
    ("C-a", "\u{1}"),
    ("C-e", "\u{5}"),
];

/// The bytes one `keys` payload stands for: literal text as typed, else the
/// named key it names. An unknown name is the text it is — Paseo's resolver
/// falls through its switch the same way — so "echo hi" types itself either
/// way and only a real token is translated.
pub(super) fn resolve_keys(keys: &str, literal: bool) -> String {
    if literal {
        return keys.to_string();
    }
    KEY_TOKENS
        .iter()
        .find(|token| token.0 == keys)
        .map(|token| token.1.to_string())
        .unwrap_or_else(|| keys.to_string())
}

/// What the card says about `keys`: a named key by name, anything else by
/// length alone. The characters themselves never leave the caller — they may
/// be a password, and the card is rendered, journaled, and (for a peer's
/// session) sent to that peer.
pub(super) fn keys_preview(keys: &str, literal: bool) -> String {
    if !literal {
        if let Some(token) = KEY_TOKENS.iter().find(|token| token.0 == keys) {
            return format!("key {}", token.0);
        }
    }
    format!("{} characters, not shown", keys.chars().count())
}
