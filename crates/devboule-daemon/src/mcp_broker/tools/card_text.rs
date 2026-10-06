//! Untrusted text as a card shows it: nothing a person cannot see.
//!
//! One responsibility: render a string an agent chose so that what the person
//! reads is what would be sent. Invisible formatting and control characters
//! become visible escapes, and a long run of blanks becomes a count, so neither
//! can push the real content out of sight. The text that is sent is never
//! touched; only this rendering is.

use devboule_protocol::{is_invisible_format, is_mandatory_line_break};

/// The longest run of blank characters shown as they are.
const BLANKS_SHOWN: usize = 3;

/// `text` with every character a reader could miss made visible: format and
/// control characters as `⟨U+202E⟩`, and a run of more than three blanks as
/// `␠×N`. Line breaks stay as they are when `keep_line_breaks` (the caller
/// marks each line), and are escaped otherwise.
pub(super) fn visible_text(text: &str, keep_line_breaks: bool) -> String {
    let mut shown = String::with_capacity(text.len());
    let mut blanks = String::new();
    for character in text.chars() {
        if character.is_whitespace() && !is_mandatory_line_break(character) {
            blanks.push(character);
            continue;
        }
        flush_blanks(&mut shown, &mut blanks);
        if is_mandatory_line_break(character) {
            if keep_line_breaks {
                shown.push(character);
            } else {
                shown.push_str(&escaped(character));
            }
        } else if is_invisible_format(character) || character.is_control() {
            shown.push_str(&escaped(character));
        } else {
            shown.push(character);
        }
    }
    flush_blanks(&mut shown, &mut blanks);
    shown
}

fn flush_blanks(shown: &mut String, blanks: &mut String) {
    let count = blanks.chars().count();
    if count > BLANKS_SHOWN {
        shown.push_str(&format!("␠×{count}"));
    } else {
        shown.push_str(blanks);
    }
    blanks.clear();
}

fn escaped(character: char) -> String {
    format!("⟨U+{:04X}⟩", u32::from(character))
}

#[cfg(test)]
mod tests {
    use super::visible_text;

    /// Every class that renders as nothing, or as something other than itself.
    #[test]
    fn invisible_and_control_characters_become_visible_escapes() {
        for (character, escape) in [
            ('\u{202e}', "⟨U+202E⟩"),
            ('\u{2066}', "⟨U+2066⟩"),
            ('\u{200b}', "⟨U+200B⟩"),
            ('\u{2060}', "⟨U+2060⟩"),
            ('\u{feff}', "⟨U+FEFF⟩"),
            ('\u{00ad}', "⟨U+00AD⟩"),
            ('\u{e0041}', "⟨U+E0041⟩"),
            ('\u{e007f}', "⟨U+E007F⟩"),
            ('\u{206a}', "⟨U+206A⟩"),
            ('\u{1b}', "⟨U+001B⟩"),
            ('\u{7}', "⟨U+0007⟩"),
        ] {
            let text = format!("a{character}b");
            assert_eq!(
                visible_text(&text, true),
                format!("a{escape}b"),
                "{character:?}"
            );
        }
    }

    #[test]
    fn plain_text_is_left_alone_including_short_runs_and_line_breaks() {
        let text = "hello  world\twith\u{a0}\u{a0}\u{a0}tabs\nand a second line";
        assert_eq!(visible_text(text, true), text);
        assert_eq!(visible_text("é ü 日本語 🙂", true), "é ü 日本語 🙂");
    }

    #[test]
    fn a_long_run_of_blanks_is_counted_not_dumped() {
        let text = format!("start{}end", " ".repeat(2000));
        assert_eq!(visible_text(&text, true), "start␠×2000end");
        assert_eq!(visible_text("a    b", true), "a␠×4b", "four is a run");
        assert_eq!(visible_text("a   b", true), "a   b", "three is not");
        let mixed = format!("x{}{}y", "\t".repeat(3), " ".repeat(3));
        assert_eq!(
            visible_text(&mixed, true),
            "x␠×6y",
            "tabs and spaces are one run"
        );
    }

    #[test]
    fn line_breaks_are_kept_for_a_message_and_escaped_for_a_one_line_fact() {
        assert_eq!(visible_text("a\nb", true), "a\nb");
        assert_eq!(visible_text("a\nb\u{2028}c", false), "a⟨U+000A⟩b⟨U+2028⟩c");
    }
}
