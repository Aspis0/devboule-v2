//! Shared character tables for untrusted text: line breaks and invisible formatting.
//!
//! One table per fact, used by every door that stores a name and later renders
//! one (device names, session titles, envelope headers, the roster boundary),
//! so a string one door accepts is not refused at another. The tables live in
//! the protocol crate because the session-name rule (`messages.rs`) is
//! validated here; the daemon calls the same functions rather than keeping a
//! second copy.

/// Mandatory line breaks: render as a new line even where `\n` is absent, so headers flatten them.
pub fn is_mandatory_line_break(character: char) -> bool {
    matches!(
        character,
        '\r' | '\n' | '\u{b}' | '\u{c}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// Zero-width and bidi formatting that makes a name render as something other than what it holds.
pub fn is_invisible_format(character: char) -> bool {
    matches!(
        character,
        '\u{00ad}' | '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}' | '\u{feff}'
    )
}

/// The first character of `name` that must never ride a display name — control,
/// invisible formatting or a mandatory line break — named for a caller to put
/// in its own sentence. `None` when every character is plain.
///
/// One rule for every surface that stores a name and later renders one (a
/// device name, a session title), so a string one door accepts is not
/// refused at another.
pub fn unsafe_character(name: &str) -> Option<&'static str> {
    for character in name.chars() {
        if character.is_control() {
            return Some("a control character");
        }
        if is_invisible_format(character) {
            return Some("an invisible formatting character");
        }
        if is_mandatory_line_break(character) {
            return Some("a line break character");
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{is_invisible_format, is_mandatory_line_break};

    #[test]
    fn the_line_break_set_is_the_mandatory_seven() {
        for character in [
            '\r', '\n', '\u{b}', '\u{c}', '\u{85}', '\u{2028}', '\u{2029}',
        ] {
            assert!(
                is_mandatory_line_break(character),
                "{character:?} must break"
            );
        }
        for character in [
            'a', ' ', '\u{7}', '\u{1b}', '\u{200b}', '\u{202e}', '\u{feff}',
        ] {
            assert!(
                !is_mandatory_line_break(character),
                "{character:?} must not break"
            );
        }
    }

    #[test]
    fn the_invisible_set_covers_marks_zero_widths_and_overrides() {
        for character in [
            '\u{ad}', '\u{61c}', '\u{200b}', '\u{200e}', '\u{200f}', '\u{202e}', '\u{2066}',
            '\u{feff}',
        ] {
            assert!(is_invisible_format(character), "{character:?} must hide");
        }
        for character in ['a', ' ', '\n', '\u{2028}', '!'] {
            assert!(
                !is_invisible_format(character),
                "{character:?} must not hide"
            );
        }
    }
}
