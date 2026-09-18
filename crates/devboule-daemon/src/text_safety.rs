//! Shared character tables for untrusted text: line breaks and invisible formatting.
//!
//! One table per fact, used by the envelope headers, the roster boundary and
//! the pairing-name check, so the three paths agree by construction.

/// Mandatory line breaks: render as a new line even where `\n` is absent, so headers flatten them.
#[cfg(any(feature = "server", test))]
pub(crate) fn is_mandatory_line_break(character: char) -> bool {
    matches!(
        character,
        '\r' | '\n' | '\u{b}' | '\u{c}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// Zero-width and bidi formatting that makes a name render as something other than what it holds.
pub(crate) fn is_invisible_format(character: char) -> bool {
    matches!(
        character,
        '\u{00ad}' | '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}' | '\u{feff}'
    )
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
