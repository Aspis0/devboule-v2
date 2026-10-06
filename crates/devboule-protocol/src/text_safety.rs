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

/// Formatting that makes text render as something other than what it holds:
/// the Unicode format category (Cf) — soft hyphen, zero-width and bidi controls,
/// the deprecated 206A-206F set, interlinear annotations, the script-specific
/// number and shorthand controls — and the tag block, which most fonts draw as
/// nothing and models read as text.
pub fn is_invisible_format(character: char) -> bool {
    matches!(
        character,
        '\u{00ad}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061c}'
            | '\u{06dd}'
            | '\u{070f}'
            | '\u{08e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0000}'..='\u{e007f}'
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

    /// Each class a message can hide behind, one member at its edges: the
    /// tag block (whole), the deprecated format set, annotation marks and the
    /// script-specific controls; and neighbours that must stay visible text.
    #[test]
    fn the_invisible_set_covers_the_tag_block_and_the_rest_of_the_format_category() {
        for character in [
            '\u{e0000}',
            '\u{e0001}',
            '\u{e0041}',
            '\u{e007f}',
            '\u{206a}',
            '\u{206f}',
            '\u{fff9}',
            '\u{fffb}',
            '\u{600}',
            '\u{605}',
            '\u{6dd}',
            '\u{70f}',
            '\u{8e2}',
            '\u{180e}',
            '\u{110bd}',
            '\u{1bca0}',
            '\u{1d173}',
            '\u{13430}',
        ] {
            assert!(is_invisible_format(character), "{character:?} must hide");
        }
        for character in [
            '\u{e0080}',
            '\u{2065}',
            '\u{2070}',
            '\u{fffc}',
            '\u{606}',
            '\u{fe0f}',
            '\u{3000}',
        ] {
            assert!(
                !is_invisible_format(character),
                "{character:?} is not format"
            );
        }
    }
}
