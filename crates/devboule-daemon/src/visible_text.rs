//! Untrusted text as a person or a model reads it: nothing it cannot see.
//!
//! One responsibility: render a string a third party chose so that what is read
//! is what is there. [`visible_text`] is the card's rendering for a person;
//! [`escape_for_model`] is the lighter one for text a model reads, which leaves
//! blank runs and the module's own markers alone. Invisible formatting and control characters
//! become visible escapes, and a long run of blank characters — spaces and the
//! glyphs that draw as spaces — becomes a count, so neither can push the real
//! content out of sight. The marker characters themselves are escaped too, so
//! no input can forge one. The text that is sent is never touched; only this
//! rendering is.

use devboule_protocol::{is_invisible_format, is_mandatory_line_break};

/// The longest run of blank characters shown as they are.
const BLANKS_SHOWN: usize = 3;
/// A rendering longer than this (characters) or taller (lines) is long enough
/// for its end to need repeating where it cannot be missed.
const LONG_AFTER_CHARS: usize = 400;
const LONG_AFTER_LINES: usize = 8;
/// How much of a long message's end is repeated.
const TAIL_SHOWN: usize = 120;
/// The characters this module writes its own markers with: input that holds one
/// is escaped like any other, so a marker is always this module's.
const MARKER_CHARACTERS: [char; 3] = ['⟨', '⟩', '␠'];

/// Characters that draw as a space without being White_Space: the Braille blank,
/// the Hangul fillers, the Khmer inherent vowels, the combining grapheme joiner
/// and the Mongolian variation selectors. Padding made of them hides text as
/// well as spaces do.
fn is_blank_glyph(character: char) -> bool {
    matches!(
        character,
        '\u{034f}' | '\u{115f}' | '\u{1160}' | '\u{17b4}' | '\u{17b5}' | '\u{180b}'
            ..='\u{180d}' | '\u{180f}' | '\u{2800}' | '\u{3164}' | '\u{ffa0}'
    )
}

fn is_blank(character: char) -> bool {
    (character.is_whitespace() || is_blank_glyph(character)) && !is_mandatory_line_break(character)
}

/// `text` with every character a reader could miss made visible: format and
/// control characters as `⟨U+202E⟩`, and a run of more than three blank
/// characters as `␠×N`. Line breaks stay as they are when `keep_line_breaks`
/// (the caller marks each line), and are escaped otherwise.
pub(crate) fn visible_text(text: &str, keep_line_breaks: bool) -> String {
    let mut shown = String::with_capacity(text.len());
    let mut blanks = String::new();
    for character in text.chars() {
        if is_blank(character) {
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
        } else if is_invisible_format(character)
            || character.is_control()
            || MARKER_CHARACTERS.contains(&character)
        {
            shown.push_str(&escaped(character));
        } else {
            shown.push(character);
        }
    }
    flush_blanks(&mut shown, &mut blanks);
    shown
}

/// `text` as a model should read it: format and control characters spelled out
/// as `⟨U+202E⟩` (the tag block and the bidi controls are the ones a model reads
/// and a person does not), line breaks and tabs kept, nothing else changed — a
/// run of spaces is indentation here, not padding, and a `⟨` is a `⟨`.
pub(crate) fn escape_for_model(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    for character in text.chars() {
        let hidden = is_invisible_format(character)
            || (character.is_control() && !matches!(character, '\n' | '\t' | '\r'));
        if hidden {
            shown.push_str(&escaped(character));
        } else {
            shown.push(character);
        }
    }
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

/// The end of an already-rendered message, when it is long enough that the end
/// may be out of view: the last [`TAIL_SHOWN`] characters, never starting in
/// the middle of an escape. `None` for a message that fits.
pub(crate) fn long_message_tail(rendered: &str) -> Option<String> {
    let characters: Vec<char> = rendered.chars().collect();
    let lines = rendered.lines().count();
    if characters.len() <= LONG_AFTER_CHARS && lines <= LONG_AFTER_LINES {
        return None;
    }
    let mut start = characters.len().saturating_sub(TAIL_SHOWN);
    // A cut that lands inside `⟨U+…⟩` moves back to the escape's opening.
    let inside_escape = |at: usize| {
        let before = &characters[..at];
        before.iter().rposition(|c| *c == '⟨') > before.iter().rposition(|c| *c == '⟩')
    };
    while start > 0 && inside_escape(start) {
        start -= 1;
    }
    Some(characters[start..].iter().collect())
}

#[cfg(test)]
mod tests {
    use super::{escape_for_model, long_message_tail, visible_text};

    /// A model reads tag characters, bidi controls and escape sequences that a
    /// person does not see: they are spelled out, and indentation, tabs, line
    /// breaks and the module's own marker characters are left exactly as they
    /// are.
    #[test]
    fn model_text_spells_out_what_is_hidden_and_changes_nothing_else() {
        assert_eq!(
            escape_for_model("a\u{e0041}b\u{202e}c\u{1b}[0m"),
            "a⟨U+E0041⟩b⟨U+202E⟩c⟨U+001B⟩[0m"
        );
        let code = "fn main() {\n\t    let x = ⟨1⟩; // ␠\n}";
        assert_eq!(escape_for_model(code), code);
    }

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

    /// Padding made of glyphs that draw as spaces counts as blanks, whatever
    /// its spelling, and the run is counted across the different kinds.
    #[test]
    fn blank_glyphs_are_blanks_for_the_run_marker() {
        for filler in [
            '\u{2800}', '\u{3164}', '\u{115f}', '\u{1160}', '\u{ffa0}', '\u{17b4}', '\u{34f}',
        ] {
            let text = format!("start{}end", filler.to_string().repeat(3000));
            assert_eq!(visible_text(&text, true), "start␠×3000end", "{filler:?}");
        }
        let mixed = format!(
            "a{}{}{} \u{2000}\u{3000}b",
            "\u{2800}", "\u{3164}", "\u{a0}"
        );
        assert_eq!(
            visible_text(&mixed, true),
            "a␠×6b",
            "kinds of blank are one run"
        );
        assert_eq!(
            visible_text("a\u{2800}b", true),
            "a\u{2800}b",
            "one is not a run"
        );
    }

    /// The markers are this module's alone: input holding one is escaped, so
    /// no message can pass itself off as an annotation.
    #[test]
    fn input_that_looks_like_a_marker_is_escaped() {
        assert_eq!(visible_text("⟨U+202E⟩", true), "⟨U+27E8⟩U+202E⟨U+27E9⟩");
        assert_eq!(visible_text("␠×2000", true), "⟨U+2420⟩×2000");
    }

    #[test]
    fn only_a_long_rendering_gets_a_tail_and_the_tail_is_its_end() {
        assert_eq!(long_message_tail("short"), None);
        assert_eq!(long_message_tail(&"x".repeat(400)), None);
        let long = format!("{}THE END", "x".repeat(500));
        let tail = long_message_tail(&long).expect("a long message has a tail");
        assert!(
            tail.ends_with("THE END") && tail.chars().count() == 120,
            "{tail}"
        );
        let tall = "line\n".repeat(9);
        assert!(long_message_tail(&tall).is_some(), "nine lines is tall");
    }

    #[test]
    fn a_tail_never_starts_inside_an_escape() {
        let long = format!("{}⟨U+202E⟩{}", "x".repeat(500), "y".repeat(115));
        let tail = long_message_tail(&long).expect("long");
        assert!(tail.starts_with("⟨U+202E⟩"), "{tail}");
    }
}
