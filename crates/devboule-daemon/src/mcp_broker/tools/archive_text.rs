/// A workspace title as the approval card shows it: bounded, with the characters
/// that would break the card's own quoting turned into underscores.
pub(super) fn quote_title(title: &str) -> String {
    let title = title
        .chars()
        .take(120)
        .map(|char| {
            if char.is_control() || matches!(char, '\\' | '"' | '\'') {
                '_'
            } else {
                char
            }
        })
        .collect::<String>();
    format!("\"{title}\"")
}
